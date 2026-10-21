// SPDX-License-Identifier: MPL-2.0
//! The server end to end over a real socket: the `Host` check on every request (no DNS
//! rebinding), loopback-only binding, 405 for anything but GET, the page's CSP, the routes, the
//! budget, the stream.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use mic_viewer::api::{Card, Frame, LayerDesc, Layers, Mark, MarksAnswer, Tick, WorldInfo};
use mic_viewer::http::{Config, Server};
use mic_viewer::scene::{Scene, SceneWriter};
use mic_viewer::viewer::{App, Backend, Changes};

struct Tiny {
    scene: std::sync::RwLock<Arc<Scene>>,
    changes: Changes,
}

fn tiny_scene(n: usize, t: Tick) -> Scene {
    let mut w = SceneWriter::new();
    for i in 0..n {
        w.mark("dots", Mark::point(i, (i % 1000) as f64, (i / 1000) as f64));
    }
    w.mark("one", Mark::point("solo", 5.0, 5.0));
    w.finish(t, 0.0)
}

impl Backend for Tiny {
    fn world(&self) -> WorldInfo {
        WorldInfo {
            name: "tiny".into(),
            source: "demo".into(),
            frames: vec![Frame { id: "f".into(), label: "F".into(), ..Frame::default() }],
            budget: 2000,
            ..WorldInfo::default()
        }
    }
    fn layers(&self) -> Arc<Layers> {
        Arc::new(Layers {
            groups: vec![],
            layers: vec![
                LayerDesc::new("dots", "g", "Dots", "point", "#fff"),
                LayerDesc::new("one", "g", "One", "point", "#fff").priority(5),
                LayerDesc::new("far", "g", "Far only", "point", "#fff").zoom(100.0, 1e9),
            ],
        })
    }
    fn scene(&self, frame: &str, _t: Option<Tick>) -> Option<Arc<Scene>> {
        (frame == "f").then(|| Arc::clone(&self.scene.read().unwrap()))
    }
    fn card(&self, layer: &str, id: &str, _t: Option<Tick>) -> Option<Card> {
        (layer == "one" && id == "solo").then(|| Card::new(layer, id, "Solo"))
    }
    fn changes(&self) -> Option<&Changes> {
        Some(&self.changes)
    }
}

fn start() -> (Server, Arc<Tiny>) {
    let tiny =
        Arc::new(Tiny { scene: std::sync::RwLock::new(Arc::new(tiny_scene(100_000, 10))), changes: Changes::new() });
    let app = Arc::new(App::new(Arc::clone(&tiny)));
    let server = Server::start("127.0.0.1:0".parse().unwrap(), Config::default(), app).unwrap();
    (server, tiny)
}

/// One request on its own connection: (status, headers, body).
fn ask(addr: SocketAddr, method: &str, path: &str, host: &str) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head, buf[split + 4..].to_vec())
}

#[test]
fn a_forged_host_is_refused_on_every_route() {
    let (mut server, _) = start();
    let addr = server.addr().unwrap();
    for path in
        ["/", "/api/world", "/api/layers", "/map.js", "/api/marks?frame=f&x0=0&z0=0&x1=1&z1=1&bpp=1&layers=dots"]
    {
        let (st, _, body) = ask(addr, "GET", path, "evil.example.com");
        assert_eq!(st, 403, "{path}");
        assert!(String::from_utf8_lossy(&body).contains("not a local name"));
        // A tailnet name too, without --allow-tailnet.
        assert_eq!(ask(addr, "GET", path, "laptop.tail1234.ts.net:8740").0, 403, "{path}");
        assert_eq!(ask(addr, "GET", path, "127.0.0.1:8740").0, 200, "{path}");
        // ssh -L to another local port still names localhost.
        assert_eq!(ask(addr, "GET", path, "localhost:9000").0, 200, "{path}");
    }
    server.stop();
}

#[test]
fn only_gets_and_the_page_carries_its_policy() {
    let (mut server, _) = start();
    let addr = server.addr().unwrap();
    for m in ["POST", "PUT", "DELETE", "PATCH"] {
        assert_eq!(ask(addr, m, "/api/world", "localhost").0, 405, "{m}");
    }
    let (st, head, _) = ask(addr, "GET", "/", "localhost");
    assert_eq!(st, 200);
    assert!(head.contains("Content-Security-Policy: default-src 'self'"), "{head}");
    assert!(head.contains("X-Content-Type-Options: nosniff"));
    // Not no-referrer: that makes a browser send `Origin: null` on same-origin POSTs.
    assert!(head.contains("Referrer-Policy: same-origin"), "{head}");
    assert!(!head.to_ascii_lowercase().contains("access-control-allow"), "no CORS");
    let (st, head, _) = ask(addr, "HEAD", "/api/world", "localhost");
    assert_eq!(st, 200);
    assert!(head.contains("application/json"));
    assert_eq!(ask(addr, "GET", "/nope", "localhost").0, 404);
    assert_eq!(ask(addr, "GET", "/../Cargo.toml", "localhost").0, 404);
    server.stop();
}

#[test]
fn a_public_address_is_refused_at_start() {
    let app = Arc::new(App::new(Arc::new(Tiny {
        scene: std::sync::RwLock::new(Arc::new(tiny_scene(1, 0))),
        changes: Changes::new(),
    })));
    for bind in ["0.0.0.0:0", "[::]:0", "192.168.1.2:0", "100.100.1.1:0"] {
        let err = Server::start(bind.parse().unwrap(), Config::default(), app.clone()).err().expect(bind);
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{bind}");
    }
    let err = Server::start("0.0.0.0:0".parse().unwrap(), Config { allow_tailnet: true, ..Config::default() }, app)
        .err()
        .unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn marks_stay_within_the_budget_and_cards_answer() {
    let (mut server, _) = start();
    let addr = server.addr().unwrap();
    // The whole tiny world (100,000 dots) at 1 block a pixel.
    let (st, _, body) = ask(
        addr,
        "GET",
        "/api/marks?frame=f&x0=-10&z0=-10&x1=1010&z1=110&bpp=1&layers=dots,one,far,unknown",
        "localhost",
    );
    assert_eq!(st, 200);
    let a: MarksAnswer = serde_json::from_slice(&body).unwrap();
    let total: usize = a.layers.values().map(|l| l.marks.len()).sum();
    assert!(total <= 2000, "{total}");
    assert!(a.layers["dots"].clustered);
    assert_eq!(a.layers["dots"].total, 100_000);
    assert_eq!(a.layers["one"].marks.len(), 1, "a quiet layer shows all");
    assert!(!a.layers.contains_key("far"), "out of its zoom range");
    assert!(!a.layers.contains_key("unknown"));
    assert!(a.layers["dots"].version > 0);
    // A small box shows the dots themselves.
    let (_, _, body) = ask(addr, "GET", "/api/marks?frame=f&x0=0&z0=0&x1=10&z1=2&bpp=0.1&layers=dots", "localhost");
    let a: MarksAnswer = serde_json::from_slice(&body).unwrap();
    assert!(!a.layers["dots"].clustered);
    assert!(a.layers["dots"].marks.iter().all(|m| matches!(m, Mark::Point { .. })));
    // Bad queries.
    assert_eq!(ask(addr, "GET", "/api/marks?frame=f&x0=0&z0=0&x1=10&bpp=1&layers=dots", "localhost").0, 400);
    assert_eq!(ask(addr, "GET", "/api/marks?frame=f&x0=0&z0=0&x1=10&z1=1&bpp=0&layers=dots", "localhost").0, 400);
    assert_eq!(ask(addr, "GET", "/api/marks?frame=g&x0=0&z0=0&x1=10&z1=1&bpp=1&layers=dots", "localhost").0, 404);
    // Cards.
    let (st, _, body) = ask(addr, "GET", "/api/thing/one/solo", "localhost");
    assert_eq!(st, 200);
    assert_eq!(serde_json::from_slice::<Card>(&body).unwrap().title, "Solo");
    assert_eq!(ask(addr, "GET", "/api/thing/one/gone", "localhost").0, 404);
    assert_eq!(ask(addr, "GET", "/tiles/none/0/0/0.png", "localhost").0, 404);
    server.stop();
}

#[test]
fn the_stream_says_hello_then_each_publication() {
    let (mut server, tiny) = start();
    let addr = server.addr().unwrap();
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET /api/stream HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut r = BufReader::new(s);
    let mut events = Vec::new();
    let mut line = String::new();
    let mut published = false;
    while events.len() < 2 {
        line.clear();
        assert!(r.read_line(&mut line).unwrap() > 0, "stream ended");
        if let Some(e) = line.strip_prefix("event: ") {
            events.push(e.trim().to_owned());
            if !published {
                published = true;
                *tiny.scene.write().unwrap() = Arc::new(tiny_scene(10, 20));
                tiny.changes.publish();
            }
        }
    }
    assert_eq!(events, ["hello", "frame"]);
    line.clear();
    r.read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim().strip_prefix("data: ").unwrap()).unwrap();
    assert!(v["versions"]["dots"].as_u64().is_some());
    assert!(v["frames"]["f"]["dots"].as_u64().is_some());
    server.stop();
}

/// A save viewer's backend: it never publishes, has a smaller budget and a layer empty for a reason.
struct Still;

impl Backend for Still {
    fn world(&self) -> WorldInfo {
        WorldInfo {
            name: "still".into(),
            source: "save".into(),
            frames: vec![Frame { id: "f".into(), label: "F".into(), ..Frame::default() }],
            budget: 300,
            ..WorldInfo::default()
        }
    }
    fn budget(&self) -> usize {
        300
    }
    fn layers(&self) -> Arc<Layers> {
        Arc::new(Layers {
            groups: vec![],
            layers: vec![
                LayerDesc::new("dots", "g", "Dots", "point", "#fff"),
                LayerDesc::new("ships", "g", "Ships", "point", "#fff"),
            ],
        })
    }
    fn scene(&self, frame: &str, _t: Option<Tick>) -> Option<Arc<Scene>> {
        let mut w = SceneWriter::new();
        for i in 0..10_000 {
            w.mark("dots", Mark::point(i, (i % 100) as f64, (i / 100) as f64));
        }
        w.empty("ships", "no ships module in this save");
        (frame == "f").then(|| Arc::new(w.finish(5, 0.0)))
    }
    fn card(&self, _layer: &str, _id: &str, _t: Option<Tick>) -> Option<Card> {
        None
    }
}

#[test]
fn a_backend_that_never_publishes_has_no_stream_and_keeps_its_budget() {
    let app = Arc::new(App::new(Arc::new(Still)));
    let mut server = Server::start("127.0.0.1:0".parse().unwrap(), Config::default(), app).unwrap();
    let addr = server.addr().unwrap();
    // 204: EventSource stops, and no connection thread waits on pings.
    let (st, _, body) = ask(addr, "GET", "/api/stream", "localhost");
    assert_eq!(st, 204);
    assert!(body.is_empty());
    let (st, _, body) =
        ask(addr, "GET", "/api/marks?frame=f&x0=0&z0=0&x1=100&z1=100&bpp=1&layers=dots,ships", "localhost");
    assert_eq!(st, 200);
    let a: MarksAnswer = serde_json::from_slice(&body).unwrap();
    let total: usize = a.layers.values().map(|l| l.marks.len()).sum();
    assert!(total <= 300, "the backend's own budget: {total}");
    assert!(a.layers["dots"].clustered);
    assert_eq!(a.layers["dots"].empty, None);
    assert_eq!(a.layers["ships"].empty.as_deref(), Some("no ships module in this save"));
    assert_eq!(a.layers["ships"].total, 0);
    server.stop();
}

/// A backend whose cards panic (a bug, a page that fails a check deep inside).
struct Panicky;

impl Backend for Panicky {
    fn world(&self) -> WorldInfo {
        WorldInfo { name: "panicky".into(), source: "save".into(), budget: 2000, ..WorldInfo::default() }
    }
    fn layers(&self) -> Arc<Layers> {
        Arc::new(Layers::default())
    }
    fn scene(&self, _frame: &str, _t: Option<Tick>) -> Option<Arc<Scene>> {
        None
    }
    fn card(&self, _layer: &str, _id: &str, _t: Option<Tick>) -> Option<Card> {
        panic!("a card that cannot be built")
    }
}

#[test]
fn a_panicking_request_answers_500_and_frees_its_connection() {
    let app = Arc::new(App::new(Arc::new(Panicky)));
    let mut server = Server::start("127.0.0.1:0".parse().unwrap(), Config::default(), app).unwrap();
    let addr = server.addr().unwrap();
    // More panics than connection slots: each one's slot comes back.
    for _ in 0..mic_viewer::http::MAX_CONNECTIONS + 6 {
        assert_eq!(ask(addr, "GET", "/api/thing/a/b", "localhost").0, 500);
    }
    assert_eq!(ask(addr, "GET", "/api/world", "localhost").0, 200);
    server.stop();
}

#[cfg(unix)]
#[test]
fn a_unix_socket_is_its_owners_and_is_never_taken_over() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("mic-viewer-sock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("viewer.sock");
    let app = || Arc::new(App::new(Arc::new(Still))) as Arc<dyn mic_viewer::http::Handler>;
    let mut first = Server::start_unix(&path, Config::default(), app()).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "only the owner connects");
    // A second server on the same path is refused while the first answers.
    let second = Server::start_unix(&path, Config::default(), app());
    assert_eq!(second.err().map(|e| e.kind()), Some(std::io::ErrorKind::AddrInUse));
    let mut c = std::os::unix::net::UnixStream::connect(&path).unwrap();
    write!(c, "GET /api/world HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "the first still serves: {out}");
    first.stop();
    // A stale socket (its server gone without removing it) is replaced.
    let stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(stale);
    let mut again = Server::start_unix(&path, Config::default(), app()).unwrap();
    again.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
