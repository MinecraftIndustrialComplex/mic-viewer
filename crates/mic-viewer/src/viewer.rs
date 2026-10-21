// SPDX-License-Identifier: MPL-2.0
//! The viewer's routes (docs/api.md §1) over a [`Backend`]: the page, `/api/*`, the stream and the
//! tiles. A project implements [`Backend`] for its sources and serves [`App`] with
//! [`crate::http::Server`].
//!
//! Every route is a GET (HEAD too); anything else is 405. Nothing here can change a simulation:
//! the backend's methods all take `&self` (VIEWER.md §5.1, VWD-11).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::aggregate;
use crate::api::{Card, EventsAnswer, Histogram, LayerMarks, Layers, MarksAnswer, Tick, WorldInfo};
use crate::assets;
use crate::http::{Handler, NO_CACHE, Reply, Request, Response, VIEWER_CSP};
use crate::scene::Scene;
use crate::sse::PING;
use crate::tiles::{Tiles, lock};

/// `/api/events` and `/api/events/histogram` queries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventQuery {
    pub frame: Option<String>,
    pub from: Tick,
    pub to: Tick,
    /// `[x0, z0, x1, z1]`, or none for the whole frame.
    pub area: Option<[f64; 4]>,
    /// Kind ids, or empty for all.
    pub kinds: Vec<String>,
    /// Rows at most (`/api/events`), buckets (`/api/events/histogram`).
    pub limit: usize,
}

impl EventQuery {
    pub fn wants(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// Rows `/api/events` returns at most.
pub const MAX_EVENT_ROWS: usize = 5_000;
/// Buckets `/api/events/histogram` returns at most.
pub const MAX_BUCKETS: usize = 1_000;

/// A project's sources: everything the routes read. All `&self`.
pub trait Backend: Send + Sync + 'static {
    /// `/api/world`.
    fn world(&self) -> WorldInfo;
    /// `/api/layers`.
    fn layers(&self) -> Arc<Layers>;
    /// The frame's marks as of `t` (None: now). None for a frame it does not know, or a past it
    /// cannot answer.
    fn scene(&self, frame: &str, t: Option<Tick>) -> Option<Arc<Scene>>;
    /// `/api/thing/<layer>/<id>`.
    fn card(&self, layer: &str, id: &str, t: Option<Tick>) -> Option<Card>;
    /// `/api/events`.
    fn events(&self, _q: &EventQuery) -> EventsAnswer {
        EventsAnswer::default()
    }
    /// `/api/events/histogram`.
    fn histogram(&self, q: &EventQuery) -> Histogram {
        Histogram { from: q.from, to: q.to, buckets: q.limit, ..Histogram::default() }
    }
    /// Archive rows newer than `cursor` (which it advances), for the stream's `events`.
    fn events_since(&self, _cursor: &mut u64) -> Vec<crate::api::EventRow> {
        Vec::new()
    }
    /// The base map `source` (the `<source>` of `/tiles/<source>/...`).
    fn tiles(&self, _source: &str) -> Option<&Tiles> {
        None
    }
    /// What wakes the stream when the backend publishes.
    fn changes(&self) -> Option<&Changes> {
        None
    }
    /// Told how many streams are open whenever that changes (a live source's lease follows it).
    fn watchers(&self, _open: usize) {}
    /// Routes of the project's own (a plug-in's scripts). GET only.
    fn extra(&self, _req: &Request) -> Option<Reply> {
        None
    }
}

/// A publication counter that streams wait on.
#[derive(Default)]
pub struct Changes {
    seq: Mutex<u64>,
    cv: Condvar,
}

impl Changes {
    pub fn new() -> Changes {
        Changes::default()
    }

    /// Something changed: wakes every stream.
    pub fn publish(&self) {
        *lock(&self.seq) += 1;
        self.cv.notify_all();
    }

    pub fn current(&self) -> u64 {
        *lock(&self.seq)
    }

    /// Waits up to `timeout` for the counter to pass `seen`; returns it.
    pub fn wait(&self, seen: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        let mut g = lock(&self.seq);
        while *g == seen {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            g = self.cv.wait_timeout(g, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
        *g
    }
}

/// The routes, as an HTTP [`Handler`].
pub struct App<B: Backend> {
    pub backend: Arc<B>,
    streams: Arc<AtomicUsize>,
}

impl<B: Backend> App<B> {
    pub fn new(backend: Arc<B>) -> App<B> {
        App { backend, streams: Arc::new(AtomicUsize::new(0)) }
    }

    /// Open streams.
    pub fn streams(&self) -> usize {
        self.streams.load(Ordering::Acquire)
    }
}

fn parse_list(s: Option<String>) -> Vec<String> {
    s.map(|v| v.split(',').map(str::trim).filter(|k| !k.is_empty()).map(str::to_owned).collect()).unwrap_or_default()
}

fn finite(req: &Request, key: &str) -> Option<f64> {
    req.num::<f64>(key).filter(|v| v.is_finite())
}

/// The `x0, z0, x1, z1` box of a query, ordered, if all four are there.
fn query_box(req: &Request) -> Option<[f64; 4]> {
    let (x0, z0, x1, z1) = (finite(req, "x0")?, finite(req, "z0")?, finite(req, "x1")?, finite(req, "z1")?);
    Some([x0.min(x1), z0.min(z1), x0.max(x1), z0.max(z1)])
}

impl<B: Backend> App<B> {
    fn marks(&self, req: &Request) -> Response {
        let Some(frame) = req.param("frame") else { return Response::error(400, "give frame") };
        let Some(b) = query_box(req) else { return Response::error(400, "give x0, z0, x1, z1 (blocks)") };
        let Some(bpp) = finite(req, "bpp").filter(|&v| v > 0.0) else {
            return Response::error(400, "give bpp (blocks per CSS pixel, above 0)");
        };
        let t = req.num::<Tick>("t");
        let Some(scene) = self.backend.scene(&frame, t) else {
            return Response::error(404, format!("no frame {frame:?} at that time"));
        };
        let descs = self.backend.layers();
        let asked = parse_list(req.param("layers"));
        // The asked layers this zoom shows, by priority (highest first), then id.
        let mut layers: Vec<(&str, i32)> = descs
            .layers
            .iter()
            .filter(|d| asked.iter().any(|a| a == &d.id) && d.min_bpp <= bpp && bpp <= d.max_bpp)
            .map(|d| (d.id.as_str(), d.priority))
            .collect();
        layers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        // Pad the box by a bin so marks just outside still draw at the edge.
        let pad = aggregate::BIN_PX * bpp;
        let b = [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad];
        let found: Vec<Vec<&crate::api::Mark>> = layers
            .iter()
            .map(|(id, _)| scene.layers.get(*id).map(|r| r.in_box(b, scene.period_x)).unwrap_or_default())
            .collect();
        let needs: Vec<usize> = found.iter().map(Vec::len).collect();
        let budget = self.backend.world().budget.max(1);
        let shares = aggregate::share(budget, &needs);
        let at = t.unwrap_or(scene.t);
        let mut out = BTreeMap::new();
        for (((id, _), marks), share) in layers.iter().zip(found).zip(shares) {
            let mut lm: LayerMarks = aggregate::reduce(marks, share, bpp, at);
            lm.version = scene.layers.get(*id).map_or(0, |r| r.version);
            out.insert((*id).to_owned(), lm);
        }
        Response::json_of(&MarksAnswer { t: at, frame, layers: out })
    }

    fn event_query(req: &Request, default_limit: usize, max: usize) -> Result<EventQuery, Response> {
        let from = req.num::<Tick>("from").unwrap_or(Tick::MIN);
        let to = req.num::<Tick>("to").unwrap_or(Tick::MAX);
        if to < from {
            return Err(Response::error(400, "to is before from"));
        }
        let limit = req.num::<usize>("limit").or(req.num::<usize>("buckets")).unwrap_or(default_limit).clamp(1, max);
        Ok(EventQuery {
            frame: req.param("frame"),
            from,
            to,
            area: query_box(req),
            kinds: parse_list(req.param("kinds")),
            limit,
        })
    }

    fn stream(&self) -> Reply {
        let backend = Arc::clone(&self.backend);
        let streams = Arc::clone(&self.streams);
        Reply::Stream(Box::new(move |w| {
            let open = streams.fetch_add(1, Ordering::AcqRel) + 1;
            backend.watchers(open);
            struct Leave<B: Backend>(Arc<B>, Arc<AtomicUsize>);
            impl<B: Backend> Drop for Leave<B> {
                fn drop(&mut self) {
                    let open = self.1.fetch_sub(1, Ordering::AcqRel) - 1;
                    self.0.watchers(open);
                }
            }
            let _leave = Leave(Arc::clone(&backend), streams);
            // What the hello describes: a publication after this point is sent as a frame.
            let mut seen = backend.changes().map(Changes::current);
            let mut cursor = 0u64;
            // Start the events where the archive is now: the stream sends what is new.
            let _ = backend.events_since(&mut cursor);
            let world = backend.world();
            w.send("hello", &serde_json::to_string(&world).unwrap_or_default())?;
            let (Some(changes), Some(mut seen)) = (backend.changes(), seen.take()) else {
                // Nothing ever changes: keep the stream (the page's sign of a live server) with pings.
                loop {
                    std::thread::sleep(PING);
                    w.ping()?;
                }
            };
            loop {
                let now = changes.wait(seen, PING);
                if now == seen {
                    w.ping()?;
                    continue;
                }
                seen = now;
                let world = backend.world();
                // Each frame's layer versions (what `/api/marks` answers with), and their xor over
                // the frames for a page that does not care which frame moved.
                let mut versions: BTreeMap<String, u64> = BTreeMap::new();
                let mut frames: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
                for f in &world.frames {
                    if let Some(s) = backend.scene(&f.id, None) {
                        let v = s.versions();
                        for (k, v) in &v {
                            *versions.entry(k.clone()).or_default() ^= v;
                        }
                        frames.insert(f.id.clone(), v);
                    }
                }
                let frame = serde_json::json!({
                    "t": world.time.tick,
                    "time": world.time,
                    "freshness": world.freshness,
                    "versions": versions,
                    "frames": frames,
                });
                let rows = backend.events_since(&mut cursor);
                let mut msgs = vec![("frame".to_owned(), frame.to_string())];
                if !rows.is_empty() {
                    msgs.push(("events".to_owned(), serde_json::json!({ "rows": rows }).to_string()));
                }
                w.send_all(msgs.iter().map(|(a, b)| (a.as_str(), b.as_str())))?;
            }
        }))
    }
}

impl<B: Backend> Handler for App<B> {
    fn handle(&self, req: &Request) -> Reply {
        if req.method != "GET" && req.method != "HEAD" {
            return Response::error(405, "the viewer is read only: GET").into();
        }
        let path = req.path.as_str();
        match path {
            "/" | "/index.html" => {
                return Response::ok("text/html; charset=utf-8", NO_CACHE, assets::PAGE.as_bytes().to_vec())
                    .with_csp(VIEWER_CSP)
                    .into();
            }
            "/api/world" => return Response::json_of(&self.backend.world()).into(),
            "/api/layers" => return Response::json_of(&*self.backend.layers()).into(),
            "/api/marks" => return self.marks(req).into(),
            "/api/events" => {
                return match Self::event_query(req, 500, MAX_EVENT_ROWS) {
                    Ok(q) => Response::json_of(&self.backend.events(&q)),
                    Err(r) => r,
                }
                .into();
            }
            "/api/events/histogram" => {
                return match Self::event_query(req, 120, MAX_BUCKETS) {
                    Ok(q) => Response::json_of(&self.backend.histogram(&q)),
                    Err(r) => r,
                }
                .into();
            }
            "/api/stream" => return self.stream(),
            _ => {}
        }
        if let Some(rest) = path.strip_prefix("/api/thing/") {
            let Some((layer, id)) = rest.split_once('/') else {
                return Response::error(404, "/api/thing/<layer>/<id>").into();
            };
            let (layer, id) = (crate::http::percent_decode(layer), crate::http::percent_decode(id));
            return match self.backend.card(&layer, &id, req.num::<Tick>("t")) {
                Some(card) => Response::json_of(&card),
                None => Response::error(404, format!("no {layer} {id} (gone, or never there)")),
            }
            .into();
        }
        if let Some(rest) = path.strip_prefix("/tiles/") {
            let Some((source, tail)) = rest.split_once('/') else { return Response::not_found().into() };
            return match self.backend.tiles(source) {
                Some(t) => t.serve(&format!("{source}/{tail}")),
                None => Response::error(404, format!("no tile source {source:?}")),
            }
            .into();
        }
        if let Some(f) = assets::file(path.trim_start_matches('/')) {
            return Response::ok(f.content_type, NO_CACHE, f.body.as_bytes().to_vec()).into();
        }
        self.backend.extra(req).unwrap_or_else(|| Response::not_found().into())
    }
}
