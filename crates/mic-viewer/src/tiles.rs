// SPDX-License-Identifier: MPL-2.0
//! Base map tiles: the [`TileSource`] trait a project implements (Deep Time's bake, rtsim's
//! HABITAT), a size-bounded cache on disk or in memory, and [`Tiles`], which answers
//! `/tiles/<layer>/<z>/<x>/<y>.png` from the two.
//!
//! The disk cache is Deep Time's (`crates/deeptime-cli/src/watch/tiles.rs`, NOTICE.md): files under
//! `<dir>/v<version>/`, written aside and renamed in, a least-recently-asked eviction to nine tenths
//! of the cap when the folder passes it, and folders of other versions removed when it is first
//! opened. The memory cache has the same bound and order; a viewer over a save uses it, so the
//! world's folder is never written (VIEWER.md §5.1).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use crate::http::{IMMUTABLE, NO_CACHE, Response};

/// Tile edge in pixels.
pub const TILE: u32 = 256;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a tile request finds.
pub enum Rendered {
    /// `TILE * TILE * 4` bytes, RGBA, rows top (north) first.
    Rgba(Vec<u8>),
    /// Nothing there: answered 204 and not cached.
    Empty,
}

/// A base map. `z` is the pyramid's zoom (docs/api.md §2.2); indices may be negative where the
/// pyramid is unbounded.
pub trait TileSource: Send + Sync + 'static {
    /// The tile's pixels. `Err` for a tile that does not exist (a layer it does not have, an index
    /// outside the pyramid): answered 404.
    fn render(&self, layer: &str, z: i32, x: i64, y: i64) -> Result<Rendered, String>;
    /// Whether the layer has no transparent pixels (it is then stored as RGB, a quarter smaller).
    fn opaque(&self, layer: &str) -> bool;
    /// Bumped whenever what a tile looks like changes; it names the cache folder.
    fn version(&self) -> u32;
}

/// RGBA8 as PNG, fast deflate (Deep Time's `encode_png`).
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> io::Result<Vec<u8>> {
    encode(w, h, rgba, png::ColorType::Rgba, 4)
}

/// RGB8 as PNG, fast deflate (Deep Time's `encode_png_rgb`).
pub fn encode_png_rgb(w: u32, h: u32, rgb: &[u8]) -> io::Result<Vec<u8>> {
    encode(w, h, rgb, png::ColorType::Rgb, 3)
}

fn encode(w: u32, h: u32, px: &[u8], color: png::ColorType, bpp: u64) -> io::Result<Vec<u8>> {
    if px.len() as u64 != u64::from(w) * u64::from(h) * bpp {
        return Err(io::Error::other(format!("{} bytes are not a {w}x{h} image of {bpp} bytes a pixel", px.len())));
    }
    let mut out = Vec::with_capacity(px.len() / 4);
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().map_err(io::Error::other)?;
    writer.write_image_data(px).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)?;
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------------------------

/// Tiles by key (`layer/z/x/y.png`), bounded in bytes.
pub struct TileCache {
    /// `<dir>/v<N>` on disk, or `None` in memory.
    root: Option<PathBuf>,
    cap: u64,
    state: Mutex<CacheState>,
}

#[derive(Default)]
struct CacheState {
    scanned: bool,
    /// key -> (bytes, last asked).
    files: HashMap<String, (u64, u64)>,
    /// The tiles themselves, for a memory cache.
    mem: HashMap<String, Arc<Vec<u8>>>,
    bytes: u64,
    clock: u64,
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl TileCache {
    /// A cache in `<dir>/v<version>`.
    pub fn on_disk(dir: &Path, version: u32, cap: u64) -> TileCache {
        TileCache { root: Some(dir.join(format!("v{version}"))), cap, state: Mutex::new(CacheState::default()) }
    }

    /// A cache that writes nothing to disk.
    pub fn in_memory(cap: u64) -> TileCache {
        TileCache { root: None, cap, state: Mutex::new(CacheState { scanned: true, ..CacheState::default() }) }
    }

    /// Reads the folder once: what is on disk counts against the cap, the oldest files (by
    /// modification time) are the first to go, and folders of other versions are removed.
    fn scan(&self, s: &mut CacheState) {
        if s.scanned {
            return;
        }
        s.scanned = true;
        let Some(root) = &self.root else { return };
        if let Some(parent) = root.parent()
            && let Ok(rd) = std::fs::read_dir(parent)
        {
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                // Only version folders: the cache's parent may hold other things.
                if e.path() != *root && name.starts_with('v') && name[1..].bytes().all(|b| b.is_ascii_digit()) && name.len() > 1 {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        fn go(dir: &Path, rel: &str, out: &mut Vec<(SystemTime, String, u64)>, depth: u32) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let path = e.path();
                let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                if path.is_dir() && depth < 4 {
                    go(&path, &rel_name, out, depth + 1);
                } else if let Ok(md) = e.metadata()
                    && md.is_file()
                    && rel_name.ends_with(".png")
                {
                    out.push((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), rel_name, md.len()));
                } else if rel_name.ends_with(".tmp") {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let mut found = Vec::new();
        go(root, "", &mut found, 0);
        found.sort();
        for (_, key, size) in found {
            s.clock += 1;
            s.bytes += size;
            s.files.insert(key, (size, s.clock));
        }
    }

    /// The tile `key` if it is cached.
    pub fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        {
            let mut s = lock(&self.state);
            self.scan(&mut s);
            s.clock += 1;
            let clock = s.clock;
            s.files.get_mut(key)?.1 = clock;
            if self.root.is_none() {
                return s.mem.get(key).cloned();
            }
        }
        let root = self.root.as_ref()?;
        match std::fs::read(root.join(key)) {
            Ok(b) => Some(Arc::new(b)),
            Err(_) => {
                // Gone behind our back: forget it.
                let mut s = lock(&self.state);
                if let Some((size, _)) = s.files.remove(key) {
                    s.bytes -= size;
                }
                None
            }
        }
    }

    /// Stores a tile and evicts the least recently asked while over the cap.
    pub fn put(&self, key: &str, data: Arc<Vec<u8>>) {
        if let Some(root) = &self.root {
            let path = root.join(key);
            if let Some(dir) = path.parent()
                && std::fs::create_dir_all(dir).is_err()
            {
                return;
            }
            let tmp = path.with_extension(format!("png.{}.tmp", TMP_SEQ.fetch_add(1, Ordering::Relaxed)));
            if std::fs::write(&tmp, &*data).is_err() || std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
                return;
            }
        }
        let mut s = lock(&self.state);
        self.scan(&mut s);
        s.clock += 1;
        let clock = s.clock;
        let size = data.len() as u64;
        if let Some((old, _)) = s.files.insert(key.to_owned(), (size, clock)) {
            s.bytes -= old;
        }
        if self.root.is_none() {
            s.mem.insert(key.to_owned(), data);
        }
        s.bytes += size;
        if s.bytes > self.cap {
            // Down to nine tenths, so the next tile does not evict again.
            let target = self.cap / 10 * 9;
            let mut order: Vec<(u64, String)> =
                s.files.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| (v.1, k.clone())).collect();
            order.sort_unstable();
            for (_, k) in order {
                if s.bytes <= target {
                    break;
                }
                if let Some((size, _)) = s.files.remove(&k) {
                    s.bytes -= size;
                    s.mem.remove(&k);
                    if let Some(root) = &self.root {
                        let _ = std::fs::remove_file(root.join(&k));
                    }
                }
            }
        }
    }

    /// (bytes, tiles, cap).
    pub fn stats(&self) -> (u64, usize, u64) {
        let mut s = lock(&self.state);
        self.scan(&mut s);
        (s.bytes, s.files.len(), self.cap)
    }
}

// ---------------------------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------------------------

/// A tile source with its cache.
pub struct Tiles {
    pub source: Arc<dyn TileSource>,
    pub cache: TileCache,
    /// Tiles never change while the source lives (Deep Time's finished runs): answered immutable.
    /// Otherwise the page revalidates (`no-cache`), and the URL is expected to carry a version.
    pub immutable: bool,
}

/// `<layer>/<z>/<x>/<y>.png`, with signed indices. Nothing else parses, so a path cannot leave the
/// cache folder.
pub fn parse_tile_path(rest: &str) -> Option<(&str, i32, i64, i64)> {
    let mut it = rest.split('/');
    let layer = it.next()?;
    if layer.is_empty() || !layer.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return None;
    }
    let z = it.next()?.parse().ok()?;
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.strip_suffix(".png")?.parse().ok()?;
    it.next().is_none().then_some((layer, z, x, y))
}

impl Tiles {
    pub fn new(source: Arc<dyn TileSource>, cache: TileCache, immutable: bool) -> Tiles {
        Tiles { source, cache, immutable }
    }

    /// Answers `rest` = `<layer>/<z>/<x>/<y>.png`.
    pub fn serve(&self, rest: &str) -> Response {
        let Some((layer, z, x, y)) = parse_tile_path(rest) else {
            return Response::error(404, "tiles are <layer>/<z>/<x>/<y>.png");
        };
        let key = format!("{layer}/{z}/{x}/{y}.png");
        let cache = if self.immutable { IMMUTABLE } else { NO_CACHE };
        if let Some(bytes) = self.cache.get(&key) {
            return Response::ok("image/png", cache, (*bytes).clone());
        }
        let rgba = match self.source.render(layer, z, x, y) {
            Ok(Rendered::Rgba(px)) => px,
            Ok(Rendered::Empty) => return Response::no_content(),
            Err(e) => return Response::error(404, e),
        };
        let encoded = if self.source.opaque(layer) {
            let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            encode_png_rgb(TILE, TILE, &rgb)
        } else {
            encode_png(TILE, TILE, &rgba)
        };
        match encoded {
            Ok(b) => {
                let b = Arc::new(b);
                self.cache.put(&key, Arc::clone(&b));
                Response::ok("image/png", cache, Arc::unwrap_or_clone(b))
            }
            Err(e) => Response::error(500, format!("tile {key}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mic-viewer-tiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tile_paths_parse_and_nothing_else_does() {
        assert_eq!(parse_tile_path("sat/3/5/2.png"), Some(("sat", 3, 5, 2)));
        assert_eq!(parse_tile_path("habitat/-6/-3/2.png"), Some(("habitat", -6, -3, 2)));
        for bad in [
            "sat/3/5/2",
            "sat/3/5/2.jpg",
            "sat/3/5.png",
            "sat/3/5/2/9.png",
            "sat/3/../2.png",
            "../3/5/2.png",
            "/3/5/2.png",
            "sat/a/5/2.png",
            "s.t/3/5/2.png",
        ] {
            assert_eq!(parse_tile_path(bad), None, "{bad}");
        }
    }

    fn walk_bytes(dir: &Path) -> u64 {
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let md = e.metadata().unwrap();
            n += if md.is_dir() { walk_bytes(&e.path()) } else { md.len() };
        }
        n
    }

    fn exercise(cache: &TileCache) -> (u64, usize) {
        let blob = Arc::new(vec![7u8; 100]);
        for k in 0..14 {
            cache.put(&format!("sat/0/{k}/0.png"), Arc::clone(&blob));
            // Ask for tile 0 all along, so it is never the oldest.
            assert!(cache.get("sat/0/0/0.png").is_some());
        }
        let (bytes, files, cap) = cache.stats();
        assert_eq!(cap, 1000);
        assert!(bytes <= 1000, "{bytes}");
        assert!((8..=10).contains(&files), "{files}");
        assert!(cache.get("sat/0/0/0.png").is_some(), "the favourite stays");
        assert!(cache.get("sat/0/1/0.png").is_none(), "the oldest went");
        (bytes, files)
    }

    #[test]
    fn the_disk_cache_stays_under_its_cap_and_drops_the_least_recently_asked() {
        let run = tmp("cap");
        let dir = run.join("tiles");
        let cache = TileCache::on_disk(&dir, 3, 1000);
        let (bytes, _) = exercise(&cache);
        assert_eq!(walk_bytes(&dir), bytes, "the counts match the disk");
        // A new cache over the same folder counts what is there, and drops other versions only.
        std::fs::create_dir_all(dir.join("v0/sat")).unwrap();
        std::fs::write(dir.join("v0/sat/old.png"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        let again = TileCache::on_disk(&dir, 3, 1000);
        assert_eq!(again.stats().0, bytes);
        assert!(!dir.join("v0").exists());
        assert!(dir.join("notes").exists());
        std::fs::remove_dir_all(&run).unwrap();
    }

    #[test]
    fn the_memory_cache_has_the_same_bound_and_writes_nothing() {
        let cache = TileCache::in_memory(1000);
        exercise(&cache);
    }

    struct Checker;
    impl TileSource for Checker {
        fn render(&self, layer: &str, z: i32, x: i64, y: i64) -> Result<Rendered, String> {
            match layer {
                "check" if z <= 0 => {
                    let v = if (x + y).rem_euclid(2) == 0 { 255 } else { 0 };
                    Ok(Rendered::Rgba(vec![v; (TILE * TILE * 4) as usize]))
                }
                "check" => Err(format!("no zoom {z}")),
                "empty" => Ok(Rendered::Empty),
                _ => Err(format!("no layer {layer}")),
            }
        }
        fn opaque(&self, layer: &str) -> bool {
            layer == "check"
        }
        fn version(&self) -> u32 {
            1
        }
    }

    #[test]
    fn serving_renders_once_then_reads_the_cache() {
        let tiles = Tiles::new(Arc::new(Checker), TileCache::in_memory(1 << 20), false);
        let a = tiles.serve("check/-2/-1/3.png");
        assert_eq!(a.status, 200);
        assert_eq!(&a.body[1..4], b"PNG");
        assert_eq!(a.cache, NO_CACHE);
        assert_eq!(tiles.cache.stats().1, 1);
        let b = tiles.serve("check/-2/-1/3.png");
        assert_eq!(a.body, b.body);
        assert_eq!(tiles.serve("empty/0/0/0.png").status, 204);
        assert_eq!(tiles.serve("check/1/0/0.png").status, 404);
        assert_eq!(tiles.serve("nope/0/0/0.png").status, 404);
        assert_eq!(tiles.serve("check/0/0").status, 404);
        let png = png::Decoder::new(std::io::Cursor::new(a.body)).read_info().unwrap();
        assert_eq!(png.info().color_type, png::ColorType::Rgb);
        assert_eq!((png.info().width, png.info().height), (TILE, TILE));
    }
}
