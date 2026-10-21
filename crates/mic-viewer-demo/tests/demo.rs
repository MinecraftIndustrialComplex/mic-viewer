// SPDX-License-Identifier: MPL-2.0
//! The demo world through the viewer's routes: every layer draws, cards answer, and a world of
//! 100,000 districts stays within the budget at every zoom.

use std::sync::Arc;

use mic_viewer::api::{Card, EventsAnswer, Histogram, Layers, MarksAnswer, WorldInfo};
use mic_viewer::http::{Handler, Reply, Request};
use mic_viewer::viewer::{App, Backend};
use mic_viewer_demo::{Clock, Demo, HALF, LAYER_IDS, OVERWORLD, RING, START, Size};

fn get(app: &App<Demo>, path: &str) -> (u16, Vec<u8>) {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let req = Request {
        method: "GET".into(),
        path: path.into(),
        query: query.into(),
        host: "localhost".into(),
        ..Request::default()
    };
    match app.handle(&req) {
        Reply::Response(r) => (r.status, r.body),
        Reply::Stream(_) => (200, Vec::new()),
    }
}

fn marks(app: &App<Demo>, frame: &str, b: [f64; 4], bpp: f64, layers: &str) -> MarksAnswer {
    let (st, body) = get(
        app,
        &format!("/api/marks?frame={frame}&x0={}&z0={}&x1={}&z1={}&bpp={bpp}&layers={layers}", b[0], b[1], b[2], b[3]),
    );
    assert_eq!(st, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn every_layer_draws_and_every_card_answers() {
    let demo = Arc::new(Demo::new(7, Size::default(), false));
    let app = App::new(Arc::clone(&demo));
    let (st, body) = get(&app, "/api/world");
    assert_eq!(st, 200);
    let world: WorldInfo = serde_json::from_slice(&body).unwrap();
    assert_eq!(world.frames.len(), 2);
    let layers: Layers = serde_json::from_slice(&get(&app, "/api/layers").1).unwrap();
    assert_eq!(layers.layers.len(), LAYER_IDS.len());
    let all = LAYER_IDS.join(",");
    // Up close (1 block a pixel) around the first village, and the whole world.
    let v = &demo.world.villages[0];
    let near = marks(&app, OVERWORLD, [v.x - 600.0, v.z - 400.0, v.x + 600.0, v.z + 400.0], 1.0, &all);
    let far = marks(&app, OVERWORLD, [-HALF, -HALF, HALF, HALF], 12.0, &all);
    for d in &layers.layers {
        let in_near = near.layers.get(&d.id).is_some_and(|l| !l.marks.is_empty());
        let in_far = far.layers.get(&d.id).is_some_and(|l| !l.marks.is_empty());
        assert!(in_near || in_far, "{} draws nothing", d.id);
    }
    assert!(far.layers.values().map(|l| l.marks.len()).sum::<usize>() <= 2000);
    // Cards for a thing of each layer that has them.
    for (layer, id) in [
        ("settle.villages", "0"),
        ("travel.caravans", "3"),
        ("travel.roads", "0"),
        ("ecology.herds", &demo.world.herds[0].id.to_string()[..]),
        ("ecology.lairs", "1"),
        ("conflict.factions", "2"),
        ("industry.districts", "5"),
    ] {
        let (st, body) = get(&app, &format!("/api/thing/{layer}/{id}"));
        assert_eq!(st, 200, "{layer} {id}");
        let c: Card = serde_json::from_slice(&body).unwrap();
        assert!(!c.title.is_empty());
    }
    let (fa, fb, _) = demo.world.feuds[0];
    assert_eq!(get(&app, &format!("/api/thing/conflict.feuds/{fa}-{fb}")).0, 200);
    // The ring wraps: a view across its seam sees the caravan cut in two pieces.
    let ring = marks(&app, RING, [1900.0, -500.0, 2200.0, 500.0], 1.0, "travel.caravans,settle.villages");
    assert_eq!(ring.layers["travel.caravans"].marks.len(), 1);
    // Events and the histogram.
    let ev: EventsAnswer = serde_json::from_slice(&get(&app, "/api/events?limit=50&kinds=conflict.raid").1).unwrap();
    assert!(!ev.rows.is_empty() && ev.rows.len() <= 50);
    assert!(ev.rows.iter().all(|r| r.kind == "conflict.raid"));
    let h: Histogram = serde_json::from_slice(&get(&app, "/api/events/histogram?buckets=60").1).unwrap();
    assert_eq!(h.buckets, 60);
    assert!(h.series.values().all(|s| s.len() == 60));
    assert!(h.series.values().flatten().sum::<u64>() > 100);
    // A tile.
    let (st, png) = get(&app, "/tiles/terrain/-5/-1/-1.png");
    assert_eq!(st, 200);
    assert_eq!(&png[1..4], b"PNG");
    assert_eq!(get(&app, "/tiles/terrain/-5/9/9.png").0, 204, "outside the world");
}

#[test]
fn a_hundred_thousand_districts_stay_within_the_budget_at_every_zoom() {
    let size = Size { villages: 2000, districts: 100_000, caravans: 20_000, days_of_events: 2 };
    let demo = Arc::new(Demo::new(11, size, false));
    let app = App::new(Arc::clone(&demo));
    let all = LAYER_IDS.join(",");
    for bpp in [0.5, 2.0, 8.0, 32.0, 128.0] {
        let (w, h) = (1600.0 * bpp, 1000.0 * bpp);
        let started = std::time::Instant::now();
        let a = marks(&app, OVERWORLD, [-w / 2.0, -h / 2.0, w / 2.0, h / 2.0], bpp, &all);
        let took = started.elapsed();
        let n: usize = a.layers.values().map(|l| l.marks.len()).sum();
        assert!(n <= 2000, "{n} marks at {bpp} bpp");
        let d = &a.layers["industry.districts"];
        assert!(d.shown <= d.total);
        eprintln!("bpp {bpp}: {n} marks, districts {} of {} in view, {took:?}", d.shown, d.total);
    }
    let scene = demo.scene(OVERWORLD, None).unwrap();
    assert_eq!(scene.layers["industry.districts"].len(), 100_000);
}

#[test]
fn a_stepped_clock_publishes_the_same_world_every_run_and_events_keep_their_frame() {
    let size = Size { villages: 60, districts: 40, caravans: 80, days_of_events: 2 };
    let clock = Clock::Stepped { ticks: 2000, every_ms: 1000 };
    let run = || {
        let demo = Arc::new(Demo::with_clock(7, size, clock));
        let app = App::new(Arc::clone(&demo));
        let mut cursor = 0;
        assert!(demo.events_since(&mut cursor).is_empty(), "nothing live before the first publish");
        let mut rows = Vec::new();
        let mut bodies = Vec::new();
        for _ in 0..6 {
            demo.publish();
            rows.extend(demo.events_since(&mut cursor));
            bodies.push(
                get(
                    &app,
                    &format!(
                        "/api/marks?frame={OVERWORLD}&x0=-6144&z0=-6144&x1=6144&z1=6144&bpp=12&layers=travel.caravans"
                    ),
                )
                .1,
            );
        }
        (demo, app, rows, bodies)
    };
    let (demo, app, rows, bodies) = run();
    let (_, _, rows2, bodies2) = run();
    assert_eq!(demo.now(), START + 6 * 2000);
    assert_eq!(rows, rows2, "the same live events on every run");
    assert_eq!(bodies, bodies2, "the same caravans on every run");
    assert_ne!(bodies[0], bodies[5], "the caravans move");
    assert!(
        rows.iter().any(|r| r.frame.is_none()) && rows.iter().any(|r| r.frame.as_deref() == Some(RING)),
        "both frames"
    );
    assert!(rows.windows(2).all(|w| w[0].t <= w[1].t));
    let world: WorldInfo = serde_json::from_slice(&get(&app, "/api/world").1).unwrap();
    assert!(world.time.running);
    assert_eq!((world.time.tick, world.time.tps, world.freshness.cycle_ms), (START + 12_000, 2000.0, Some(1000)));
    // Each frame's events are its own.
    let since = START;
    let ring: EventsAnswer =
        serde_json::from_slice(&get(&app, &format!("/api/events?frame={RING}&from={since}&limit=500")).1).unwrap();
    let over: EventsAnswer =
        serde_json::from_slice(&get(&app, &format!("/api/events?frame={OVERWORLD}&from={since}&limit=500")).1).unwrap();
    assert!(!ring.rows.is_empty() && ring.rows.iter().all(|r| r.frame.as_deref() == Some(RING)));
    assert!(!over.rows.is_empty() && over.rows.iter().all(|r| r.frame.is_none()));
    assert_eq!(ring.rows.len() + over.rows.len(), rows.len());
}
