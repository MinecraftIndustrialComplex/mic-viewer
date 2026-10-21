// SPDX-License-Identifier: MPL-2.0
//! A small HTTP/1.1 server on std's listeners: GET, HEAD and (for consumers that want them) POST,
//! keep-alive, a thread per connection (at most [`MAX_CONNECTIONS`]), server-sent event streams.
//!
//! Adapted from Deep Time's workbench server (`crates/deeptime-cli/src/watch/http.rs`, NOTICE.md).
//! What changed (VIEWER.md §13): routes are a [`Handler`] instead of a hard-coded `get()`; every
//! request's `Host` is checked, not only POSTs' (a page on another site that rebinds its name to
//! 127.0.0.1 could otherwise read the answers); and the server binds and serves loopback only unless
//! [`Config::allow_tailnet`] is set. With `allow_tailnet` it also accepts Tailscale's ranges, as
//! Deep Time's server always did. A Unix socket ([`Server::start_unix`]) serves a machine where other
//! accounts have shells: only the op can enter its folder, and `ssh -L 8740:/path/to/socket`
//! forwards it.

use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::sse::SseWriter;

/// Connections served at once; more are answered 503.
pub const MAX_CONNECTIONS: usize = 64;
const MAX_HEADER_BYTES: u64 = 16 * 1024;
const MAX_BODY_BYTES: u64 = 1 << 20;
/// A keep-alive connection with no request for this long is closed.
const IDLE: Duration = Duration::from_secs(60);

/// Never cached (the page and its scripts change with the build).
pub const NO_CACHE: &str = "no-cache";
/// Never stored (API answers).
pub const NO_STORE: &str = "no-store";
/// An answer whose URL names its version.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The viewer page's Content Security Policy (docs/api.md §8): nothing from the network, no inline
/// script or style.
pub const VIEWER_CSP: &str = "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; \
                              script-src 'self'; connect-src 'self'; base-uri 'none'; \
                              frame-ancestors 'none'; form-action 'none'";

/// Loopback addresses (IPv4 127/8, `::1`, and IPv4-mapped loopback).
pub fn loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback(),
        IpAddr::V6(a) => a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// Tailscale's ranges: 100.64.0.0/10 (the CGNAT block its IPv4 addresses come from) and
/// fd7a:115c:a1e0::/48.
pub fn tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.octets()[0] == 100 && a.octets()[1] & 0xc0 == 0x40,
        IpAddr::V6(a) => {
            a.to_ipv4_mapped().is_some_and(|v4| tailnet(IpAddr::V4(v4)))
                || a.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0]
        }
    }
}

/// Loopback, or Tailscale's ranges (Deep Time's `tailnet_or_loopback`).
pub fn tailnet_or_loopback(ip: IpAddr) -> bool {
    loopback(ip) || tailnet(ip)
}

/// Whether a peer or a bound address may be served under this policy.
pub fn address_allowed(ip: IpAddr, allow_tailnet: bool) -> bool {
    loopback(ip) || (allow_tailnet && tailnet(ip))
}

/// The name in a `Host` header, without its port and brackets, lower case.
fn host_name(host: &str) -> String {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h)
    };
    name.to_ascii_lowercase()
}

/// Whether a request's `Host` is one this server answers (VIEWER.md §13): `localhost`, a loopback IP
/// literal (`127.0.0.1`, `[::1]`), on any port; with `allow_tailnet` also a tailnet IP literal, a bare
/// machine name (MagicDNS) or a `*.ts.net` name. Anything else is a foreign site's name pointed at
/// this address.
pub fn host_allowed(host: &str, allow_tailnet: bool) -> bool {
    let name = host_name(host);
    if name.is_empty() {
        return false;
    }
    if name == "localhost" {
        return true;
    }
    if let Ok(ip) = name.parse::<IpAddr>() {
        return address_allowed(ip, allow_tailnet);
    }
    allow_tailnet && (!name.contains('.') || name.ends_with(".ts.net"))
}

/// How a server listens.
#[derive(Clone, Debug)]
pub struct Config {
    /// Accept tailnet addresses and names as well as loopback (`--allow-tailnet`).
    pub allow_tailnet: bool,
    /// Thread names start with this.
    pub name: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { allow_tailnet: false, name: "mic-viewer".into() }
    }
}

/// One request (its body read when it has one of at most 1 MiB).
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub keep_alive: bool,
    pub host: String,
    pub origin: Option<String>,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Request {
    /// The first value of a query parameter, percent-decoded.
    pub fn param(&self, key: &str) -> Option<String> {
        query_param(&self.query, key).map(percent_decode)
    }

    /// A query parameter parsed as a number.
    pub fn num<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.param(key).and_then(|v| v.parse().ok())
    }
}

/// The raw value of a query parameter.
pub fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// `%xx` and `+` decoding of a query value.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An answer.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: Cow<'static, str>,
    pub cache: &'static str,
    pub body: Vec<u8>,
    /// A Content Security Policy for this answer (the viewer page sets [`VIEWER_CSP`]).
    pub csp: Option<&'static str>,
}

impl Response {
    pub fn ok(content_type: impl Into<Cow<'static, str>>, cache: &'static str, body: Vec<u8>) -> Response {
        Response { status: 200, content_type: content_type.into(), cache, body, csp: None }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Response {
        Response {
            status,
            content_type: "text/plain; charset=utf-8".into(),
            cache: NO_STORE,
            body: body.into().into_bytes(),
            csp: None,
        }
    }

    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response {
            status,
            content_type: "application/json".into(),
            cache: NO_STORE,
            body: v.to_string().into_bytes(),
            csp: None,
        }
    }

    /// A serializable value as JSON, 200.
    pub fn json_of<T: serde::Serialize>(v: &T) -> Response {
        match serde_json::to_vec(v) {
            Ok(body) => {
                Response { status: 200, content_type: "application/json".into(), cache: NO_STORE, body, csp: None }
            }
            Err(e) => Response::error(500, format!("encoding the answer: {e}")),
        }
    }

    pub fn error(status: u16, message: impl Into<String>) -> Response {
        Response::json(status, &serde_json::json!({ "error": message.into() }))
    }

    pub fn not_found() -> Response {
        Response::text(404, "not found\n")
    }

    pub fn no_content() -> Response {
        Response { status: 204, content_type: "text/plain".into(), cache: NO_STORE, body: Vec::new(), csp: None }
    }

    pub fn with_csp(mut self, csp: &'static str) -> Response {
        self.csp = Some(csp);
        self
    }

    pub fn with_cache(mut self, cache: &'static str) -> Response {
        self.cache = cache;
        self
    }
}

/// A stream's body: called with the event writer once the headers are out, returns when the page
/// goes away (a write fails) or the stream ends.
pub type StreamFn = Box<dyn FnOnce(&mut SseWriter<'_>) -> io::Result<()> + Send>;

/// What a handler answers.
pub enum Reply {
    Response(Response),
    /// Server-sent events (`text/event-stream`); the connection closes when it ends.
    Stream(StreamFn),
}

impl From<Response> for Reply {
    fn from(r: Response) -> Reply {
        Reply::Response(r)
    }
}

/// The routes. Called on the connection's thread for every request that passed the address and
/// `Host` checks.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, req: &Request) -> Reply;
}

impl<F> Handler for F
where
    F: Fn(&Request) -> Reply + Send + Sync + 'static,
{
    fn handle(&self, req: &Request) -> Reply {
        self(req)
    }
}

/// Where a server listens.
#[derive(Clone, Debug)]
pub enum Bound {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(std::path::PathBuf),
}

impl std::fmt::Display for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bound::Tcp(a) => write!(f, "http://{a}/"),
            #[cfg(unix)]
            Bound::Unix(p) => write!(f, "unix:{}", p.display()),
        }
    }
}

pub struct Server {
    bound: Bound,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Server {
    /// Binds `bind` and serves `handler` on a background thread. Refuses an address that is not
    /// loopback (or tailnet, with `allow_tailnet`).
    pub fn start(bind: SocketAddr, config: Config, handler: Arc<dyn Handler>) -> io::Result<Server> {
        if !address_allowed(bind.ip(), config.allow_tailnet) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                if config.allow_tailnet {
                    format!("{} is neither loopback nor a Tailscale address", bind.ip())
                } else {
                    format!(
                        "{} is not a loopback address (reach the viewer with `ssh -L`; tailnet addresses need --allow-tailnet)",
                        bind.ip()
                    )
                },
            ));
        }
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let accept = std::thread::Builder::new()
            .name(format!("{}-accept", config.name))
            .spawn(move || accept_tcp(listener, config, handler, stopping))?;
        Ok(Server { bound: Bound::Tcp(addr), stop, accept: Some(accept) })
    }

    /// Serves on a Unix socket at `path` (removed first if it is a stale socket). The folder's
    /// permissions decide who may connect; the `Host` check still applies.
    #[cfg(unix)]
    pub fn start_unix(path: &std::path::Path, config: Config, handler: Arc<dyn Handler>) -> io::Result<Server> {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_socket() {
                std::fs::remove_file(path)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", path.display()),
                ));
            }
        }
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let accept = std::thread::Builder::new()
            .name(format!("{}-accept", config.name))
            .spawn(move || accept_unix(listener, config, handler, stopping))?;
        Ok(Server { bound: Bound::Unix(path.to_path_buf()), stop, accept: Some(accept) })
    }

    pub fn bound(&self) -> &Bound {
        &self.bound
    }

    /// The TCP address (None for a Unix socket).
    pub fn addr(&self) -> Option<SocketAddr> {
        match &self.bound {
            Bound::Tcp(a) => Some(*a),
            #[cfg(unix)]
            Bound::Unix(_) => None,
        }
    }

    /// Stops accepting (connections already open finish on their own).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the blocking accept.
        match &self.bound {
            Bound::Tcp(a) => {
                let _ = TcpStream::connect_timeout(a, Duration::from_secs(1));
            }
            #[cfg(unix)]
            Bound::Unix(p) => {
                let _ = std::os::unix::net::UnixStream::connect(p);
            }
        }
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        #[cfg(unix)]
        if let Bound::Unix(p) = &self.bound {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Serves until the process ends.
    pub fn join(mut self) {
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

/// A connection: TCP or a Unix socket.
trait Conn: Read + Write + Send + Sized + 'static {
    fn try_clone_conn(&self) -> io::Result<Self>;
    fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()>;
    fn shutdown_both(&self);
}

impl Conn for TcpStream {
    fn try_clone_conn(&self) -> io::Result<Self> {
        self.try_clone()
    }
    fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(read)?;
        self.set_write_timeout(write)
    }
    fn shutdown_both(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

#[cfg(unix)]
impl Conn for std::os::unix::net::UnixStream {
    fn try_clone_conn(&self) -> io::Result<Self> {
        self.try_clone()
    }
    fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(read)?;
        self.set_write_timeout(write)
    }
    fn shutdown_both(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

fn accept_tcp(listener: TcpListener, config: Config, handler: Arc<dyn Handler>, stop: Arc<AtomicBool>) {
    let active = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = conn else { continue };
        let peer_ok = stream.peer_addr().is_ok_and(|p| address_allowed(p.ip(), config.allow_tailnet));
        let _ = stream.set_nodelay(true);
        spawn_conn(stream, peer_ok, &config, &handler, &active, &stop);
    }
}

#[cfg(unix)]
fn accept_unix(
    listener: std::os::unix::net::UnixListener,
    config: Config,
    handler: Arc<dyn Handler>,
    stop: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = conn else { continue };
        spawn_conn(stream, true, &config, &handler, &active, &stop);
    }
}

fn spawn_conn<C: Conn>(
    mut stream: C,
    peer_ok: bool,
    config: &Config,
    handler: &Arc<dyn Handler>,
    active: &Arc<AtomicUsize>,
    stop: &Arc<AtomicBool>,
) {
    if active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
        active.fetch_sub(1, Ordering::AcqRel);
        let _ = stream.set_timeouts(None, Some(Duration::from_secs(5)));
        let _ = respond(&mut stream, &Response::text(503, "too many connections\n"), false, false);
        return;
    }
    let (handler, done, stop, allow_tailnet) =
        (Arc::clone(handler), Arc::clone(active), Arc::clone(stop), config.allow_tailnet);
    let spawned = std::thread::Builder::new().name(format!("{}-conn", config.name)).spawn(move || {
        let _ = handle(stream, peer_ok, allow_tailnet, handler.as_ref(), &stop);
        done.fetch_sub(1, Ordering::AcqRel);
    });
    if spawned.is_err() {
        active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

/// `Referrer-Policy: same-origin`, not `no-referrer`: under `no-referrer` a browser sends
/// `Origin: null` on the page's own POSTs, and [`post_refusal`] would refuse every one of them.
fn respond<W: Write>(out: &mut W, r: &Response, head: bool, keep_alive: bool) -> io::Result<()> {
    let mut msg = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: same-origin\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len(),
        r.cache,
    );
    if let Some(csp) = r.csp {
        msg.push_str(&format!("Content-Security-Policy: {csp}\r\n"));
    }
    msg.push_str(if keep_alive { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    let mut msg = msg.into_bytes();
    if !head {
        msg.extend_from_slice(&r.body);
    }
    out.write_all(&msg)?;
    out.flush()
}

/// One request. `None` at a clean end of the connection.
fn read_request<R: Read>(r: &mut BufReader<R>) -> io::Result<Option<Request>> {
    let mut limited = (&mut *r).take(MAX_HEADER_BYTES);
    let mut line = String::new();
    loop {
        line.clear();
        if limited.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad request line"));
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let mut keep_alive = version == "HTTP/1.1";
    let (mut content_length, mut host, mut origin, mut content_type) = (0u64, String::new(), None, String::new());
    loop {
        line.clear();
        if limited.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "headers cut short or too long"));
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let v = v.trim();
            let lower = v.to_ascii_lowercase();
            match k.trim().to_ascii_lowercase().as_str() {
                "connection" if lower.contains("close") => keep_alive = false,
                "connection" if lower.contains("keep-alive") => keep_alive = true,
                "content-length" => content_length = v.parse().unwrap_or(u64::MAX),
                "host" => host = v.to_owned(),
                "origin" => origin = Some(v.to_owned()),
                "content-type" => content_type = lower,
                _ => {}
            }
        }
    }
    if content_length > MAX_BODY_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the request body is over 1 MiB"));
    }
    let mut body = Vec::new();
    if content_length > 0 {
        (&mut *r).take(content_length).read_to_end(&mut body)?;
    }
    let target = target.split('#').next().unwrap_or("/");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (target.to_owned(), String::new()),
    };
    Ok(Some(Request { method, path, query, keep_alive, host, origin, content_type, body }))
}

fn handle<C: Conn>(
    stream: C,
    peer_ok: bool,
    allow_tailnet: bool,
    handler: &dyn Handler,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_timeouts(Some(IDLE), Some(Duration::from_secs(30)))?;
    let mut out = stream;
    if !peer_ok {
        return respond(
            &mut out,
            &Response::text(
                403,
                if allow_tailnet {
                    "Forbidden: this viewer serves loopback and the tailnet only\n"
                } else {
                    "Forbidden: this viewer serves loopback only (use ssh -L)\n"
                },
            ),
            false,
            false,
        );
    }
    let mut reader = BufReader::new(out.try_clone_conn()?);
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let req = match read_request(&mut reader) {
            Ok(Some(req)) => req,
            Ok(None) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                return respond(&mut out, &Response::text(400, format!("{e}\n")), false, false);
            }
            Err(e) => return Err(e),
        };
        if !host_allowed(&req.host, allow_tailnet) {
            let msg = format!("Forbidden: not a local name: {:?}\n", req.host);
            return respond(&mut out, &Response::text(403, msg), false, false);
        }
        let head = req.method == "HEAD";
        match handler.handle(&req) {
            Reply::Response(resp) => {
                respond(&mut out, &resp, head, req.keep_alive)?;
                if !req.keep_alive {
                    return Ok(());
                }
            }
            Reply::Stream(body) => {
                out.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
                      X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\nretry: 2000\n\n",
                )?;
                out.flush()?;
                if head {
                    return Ok(());
                }
                // A stream may wait longer than the idle timeout between events.
                out.set_timeouts(None, Some(Duration::from_secs(30)))?;
                let result = {
                    let mut w = SseWriter::new(&mut out);
                    body(&mut w)
                };
                out.shutdown_both();
                return result;
            }
        }
    }
}

/// The POST guard Deep Time's workbench uses (JSON only, same origin, an allowed name): a
/// cross-site form cannot send `application/json` without a CORS preflight, which this server never
/// grants.
pub fn post_refusal(req: &Request, allow_tailnet: bool) -> Option<Response> {
    if !req.content_type.starts_with("application/json") {
        return Some(Response::error(415, "POST application/json"));
    }
    if !host_allowed(&req.host, allow_tailnet) {
        return Some(Response::error(403, format!("not a local name: {}", req.host)));
    }
    if let Some(o) = &req.origin {
        let from = o.split_once("://").map_or(o.as_str(), |(_, rest)| rest);
        if from != req.host {
            return Some(Response::error(403, format!("cross-origin request from {o}")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        for ok in ["127.0.0.1", "127.3.2.1", "::1", "::ffff:127.0.0.1"] {
            assert!(address_allowed(ok.parse().unwrap(), false), "{ok}");
        }
        for tail in
            ["100.64.0.1", "100.96.0.7", "100.127.255.254", "fd7a:115c:a1e0::7", "::ffff:100.96.0.7"]
        {
            assert!(!address_allowed(tail.parse().unwrap(), false), "{tail} without the flag");
            assert!(address_allowed(tail.parse().unwrap(), true), "{tail} with the flag");
        }
        for no in [
            "0.0.0.0",
            "::",
            "10.0.0.217",
            "192.168.1.20",
            "100.63.255.255",
            "100.128.0.1",
            "8.8.8.8",
            "fd7a:115c:a1e1::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!address_allowed(no.parse().unwrap(), true), "{no}");
        }
    }

    #[test]
    fn hosts() {
        for ok in ["localhost", "localhost:8740", "127.0.0.1:8740", "127.0.0.1:9000", "[::1]:8740", "LOCALHOST:1"] {
            assert!(host_allowed(ok, false), "{ok}");
        }
        for tail in [
            "laptop:8642",
            "viewer.tail0000.ts.net:8642",
            "100.64.0.1:8642",
            "[fd7a:115c:a1e0::1]:8642",
        ] {
            assert!(!host_allowed(tail, false), "{tail} without the flag");
            assert!(host_allowed(tail, true), "{tail} with the flag");
        }
        for no in [
            "evil.example.com",
            "evil.example.com:8740",
            "",
            "192.168.1.4:8740",
            "localhost.evil.com",
            "127.0.0.1.nip.io:8740",
        ] {
            assert!(!host_allowed(no, true), "{no}");
        }
    }

    #[test]
    fn query_decoding() {
        assert_eq!(percent_decode("a%20b+c"), "a b c");
        assert_eq!(percent_decode("settle.villages%2Ctravel.edges"), "settle.villages,travel.edges");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("minecraft%3Aoverworld"), "minecraft:overworld");
        let r = Request { query: "frame=minecraft%3Aoverworld&x0=-12&bpp=2.5".into(), ..Default::default() };
        assert_eq!(r.param("frame").as_deref(), Some("minecraft:overworld"));
        assert_eq!(r.num::<i64>("x0"), Some(-12));
        assert_eq!(r.num::<f64>("bpp"), Some(2.5));
        assert_eq!(r.num::<f64>("missing"), None);
    }
}
