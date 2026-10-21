// SPDX-License-Identifier: MPL-2.0
//! Surveyor's server-side terrain summaries as a base map (mic-rtsim VIEWER.md §9, V5).
//!
//! Surveyor (in the MIC pack, both sides) records the top floor of every column of every chunk the
//! server loads, per dimension, in `<dimension>/data/surveyor/c.<rx>.<rz>.dat`. This crate reads
//! those files ([`nbt`], [`region`]), draws a region one pixel a block ([`render`]) and answers the
//! viewer's square tile pyramid from them ([`Surveyor::tile`]): zoom 0 is one block a pixel,
//! [`MIN_ZOOM`] the coarsest. It only ever opens files for reading (VWD-11).
//!
//! A region is parsed once and kept as a picture and its halvings in a cache bounded in bytes; a
//! coarse tile pastes the small halvings of every region it covers.

pub mod nbt;
pub mod region;
pub mod render;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mic_viewer::api::{PyramidDesc, TileLayerDesc};
use mic_viewer::tiles::{Rendered, TILE, TileError, TileSource};

use region::{REGION, Region};

/// The coarsest zoom: 64 blocks a pixel, a tile 16,384 blocks (32 x 32 regions) on a side. TUNABLE.
pub const MIN_ZOOM: i32 = -6;
/// The finest zoom: one block a pixel (the page stretches it further).
pub const MAX_ZOOM: i32 = 0;
/// Regions further apart than this (in regions, either axis) fall in separate clusters for
/// [`Surveyor::bounds`]: 64 regions, 32,768 blocks. TUNABLE.
pub const CLUSTER_GAP: i32 = 64;
/// The picture cache's default bound: 64 MiB, about 48 regions at one block a pixel with their
/// halvings. TUNABLE.
pub const DEFAULT_CACHE_BYTES: usize = 64 << 20;
/// A region file inflating past this is refused (the largest seen inflates to about 2 MB).
pub const MAX_INFLATED: u64 = 64 << 20;
/// Bumped when what a tile looks like changes.
pub const RENDER_VERSION: u32 = 1;
/// The Nether's floors above this are its bedrock roof; [`default_max_y`] skips them so the floor
/// below shows.
pub const NETHER_ROOF: i32 = 126;

const W: usize = REGION as usize;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where Surveyor keeps a dimension's summaries in a world folder (Minecraft 1.21.1's layout).
pub fn dir_for(world: &Path, dimension: &str) -> PathBuf {
    let base = match dimension {
        "minecraft:overworld" | "overworld" => world.to_path_buf(),
        "minecraft:the_nether" | "the_nether" => world.join("DIM-1"),
        "minecraft:the_end" | "the_end" => world.join("DIM1"),
        other => {
            let (ns, path) = other.split_once(':').unwrap_or(("minecraft", other));
            path.split('/').fold(world.join("dimensions").join(ns), |p, part| p.join(part))
        }
    };
    base.join("data").join("surveyor")
}

/// The height above which floors are skipped by default: the Nether's roof, nothing elsewhere.
pub fn default_max_y(dimension: &str) -> Option<i32> {
    matches!(dimension, "minecraft:the_nether" | "the_nether").then_some(NETHER_ROOF)
}

/// The region a file name names: `c.<rx>.<rz>.dat`.
pub fn region_of(name: &str) -> Option<(i32, i32)> {
    let (rx, rz) = name.strip_prefix("c.")?.strip_suffix(".dat")?.split_once('.')?;
    Some((rx.parse().ok()?, rz.parse().ok()?))
}

/// A picture's key: region x, region z, halvings.
type PicKey = (i32, i32, u32);
/// A region's south-row heights.
type Edges = HashMap<(i32, i32), Arc<Vec<i32>>>;

/// Pictures with the clock of their last use, least recently used first out.
struct Cache {
    map: HashMap<PicKey, (Arc<Vec<u8>>, u64)>,
    bytes: usize,
    cap: usize,
    clock: u64,
}

impl Cache {
    fn get(&mut self, k: PicKey) -> Option<Arc<Vec<u8>>> {
        self.clock += 1;
        let clock = self.clock;
        self.map.get_mut(&k).map(|e| {
            e.1 = clock;
            e.0.clone()
        })
    }

    fn put(&mut self, k: PicKey, v: Arc<Vec<u8>>) {
        self.clock += 1;
        self.bytes += v.len();
        if let Some((old, _)) = self.map.insert(k, (v, self.clock)) {
            self.bytes -= old.len();
        }
        if self.bytes > self.cap {
            let mut order: Vec<_> = self.map.iter().map(|(k, e)| (e.1, *k)).collect();
            order.sort_unstable();
            for (_, k) in order {
                if self.bytes <= self.cap / 10 * 9 {
                    break;
                }
                if let Some((v, _)) = self.map.remove(&k) {
                    self.bytes -= v.len();
                }
            }
        }
    }
}

/// One dimension's Surveyor summaries.
pub struct Surveyor {
    dir: PathBuf,
    max_y: Option<i32>,
    regions: BTreeSet<(i32, i32)>,
    cache: Mutex<Cache>,
    /// South-row heights by region, for the relief of the region south of each.
    edges: Mutex<Edges>,
    parses: AtomicU64,
}

impl Surveyor {
    /// The summaries in `dir` (see [`dir_for`]); `None` when the folder is absent or holds no region
    /// file (the viewer then falls back to HABITAT).
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Option<Surveyor>> {
        let dir = dir.into();
        let entries = match fs::read_dir(&dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            r => r?,
        };
        let mut regions = BTreeSet::new();
        for e in entries {
            if let Some(r) = e?.file_name().to_str().and_then(region_of) {
                regions.insert(r);
            }
        }
        if regions.is_empty() {
            return Ok(None);
        }
        Ok(Some(Surveyor {
            dir,
            max_y: None,
            regions,
            cache: Mutex::new(Cache { map: HashMap::new(), bytes: 0, cap: DEFAULT_CACHE_BYTES, clock: 0 }),
            edges: Mutex::new(HashMap::new()),
            parses: AtomicU64::new(0),
        }))
    }

    /// Skip floors above `max_y` (see [`default_max_y`]).
    pub fn with_max_y(mut self, max_y: Option<i32>) -> Self {
        self.max_y = max_y;
        self
    }

    /// Bound the picture cache.
    pub fn with_cache_bytes(self, cap: usize) -> Self {
        lock(&self.cache).cap = cap;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The regions with a file, in order.
    pub fn regions(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.regions.iter().copied()
    }

    /// Region files parsed so far (each parse reads and inflates one file).
    pub fn parses(&self) -> u64 {
        self.parses.load(Ordering::Relaxed)
    }

    /// Bytes the picture cache holds.
    pub fn cached_bytes(&self) -> usize {
        lock(&self.cache).bytes
    }

    /// One region, read and parsed now (no cache).
    pub fn read_region(&self, rx: i32, rz: i32) -> io::Result<Region> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(format!("c.{rx}.{rz}.dat"));
        let root = nbt::read_gzip(io::BufReader::new(fs::File::open(&path)?), MAX_INFLATED)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        let r = Region::from_nbt(&root, rx, rz, self.max_y)?;
        lock(&self.edges).insert((rx, rz), Arc::new(render::south_edge(&r)));
        Ok(r)
    }

    /// The block rectangle `[x0, z0, x1, z1]` of the largest cluster of regions (by region count;
    /// regions at most [`CLUSTER_GAP`] apart join), so a few far-off regions (a ship's sub-level, a
    /// lone trip) do not set the map's extent.
    pub fn bounds(&self) -> [f64; 4] {
        let all: Vec<(i32, i32)> = self.regions().collect();
        let cell = |r: (i32, i32)| (r.0.div_euclid(CLUSTER_GAP), r.1.div_euclid(CLUSTER_GAP));
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, &r) in all.iter().enumerate() {
            grid.entry(cell(r)).or_default().push(i);
        }
        let mut seen = vec![false; all.len()];
        let mut best: (usize, [i32; 4]) = (0, [0; 4]);
        for start in 0..all.len() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let (mut n, mut bb) = (0, [all[start].0, all[start].1, all[start].0, all[start].1]);
            let mut queue = VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                let r = all[i];
                n += 1;
                bb = [bb[0].min(r.0), bb[1].min(r.1), bb[2].max(r.0), bb[3].max(r.1)];
                let c = cell(r);
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        for &j in grid.get(&(c.0 + dx, c.1 + dz)).map_or(&[][..], Vec::as_slice) {
                            let o = all[j];
                            if !seen[j] && (o.0 - r.0).abs() <= CLUSTER_GAP && (o.1 - r.1).abs() <= CLUSTER_GAP {
                                seen[j] = true;
                                queue.push_back(j);
                            }
                        }
                    }
                }
            }
            if n > best.0 {
                best = (n, bb);
            }
        }
        let b = best.1;
        [b[0], b[1], b[2] + 1, b[3] + 1].map(|v| f64::from(v) * f64::from(REGION))
    }

    /// The base map's descriptor: the square pyramid, [`Surveyor::bounds`], on by default.
    pub fn layer(&self, id: &str, label: &str, url: &str) -> TileLayerDesc {
        TileLayerDesc {
            id: id.into(),
            label: label.into(),
            url: url.into(),
            pyramid: PyramidDesc { kind: "square".into(), min_zoom: MIN_ZOOM, max_zoom: MAX_ZOOM },
            bounds: self.bounds(),
            overlay: false,
            attribution: "Surveyor terrain summaries".into(),
            default_on: Some(true),
            alpha: None,
        }
    }

    /// A region's picture halved `level` times (edge `512 >> level`), from the cache or drawn now
    /// (drawing caches every halving).
    pub fn picture(&self, rx: i32, rz: i32, level: u32) -> io::Result<Arc<Vec<u8>>> {
        if let Some(p) = lock(&self.cache).get((rx, rz, level)) {
            return Ok(p);
        }
        let r = self.read_region(rx, rz)?;
        let north = if self.regions.contains(&(rx, rz - 1)) {
            let known = lock(&self.edges).get(&(rx, rz - 1)).cloned();
            match known {
                Some(e) => Some(e),
                // Parsed only for its south row; an unreadable neighbour just leaves the top row flat.
                None => self.read_region(rx, rz - 1).ok().and_then(|_| lock(&self.edges).get(&(rx, rz - 1)).cloned()),
            }
        } else {
            None
        };
        let mut level_img = Arc::new(render::region_rgba(&r, north.as_deref().map(Vec::as_slice)));
        let mut levels = vec![level_img.clone()];
        let mut w = W;
        while w > 1 {
            level_img = Arc::new(render::halve(&level_img, w));
            w /= 2;
            levels.push(level_img.clone());
        }
        let want = levels.get(level as usize).cloned().ok_or_else(|| io::Error::other(format!("level {level}")))?;
        let mut cache = lock(&self.cache);
        for (l, img) in levels.into_iter().enumerate() {
            cache.put((rx, rz, l as u32), img);
        }
        Ok(want)
    }

    /// Tile `(z, x, y)` of the square pyramid (docs/api.md §2.2): `TILE` pixels a side,
    /// `TILE * 2^-z` blocks, its north-west corner at block `(x, y) * TILE * 2^-z`.
    pub fn tile(&self, z: i32, x: i64, y: i64) -> Result<Rendered, TileError> {
        if !(MIN_ZOOM..=MAX_ZOOM).contains(&z) {
            return Err(TileError::Missing(format!("zoom {z} is outside {MIN_ZOOM}..={MAX_ZOOM}")));
        }
        let t = TILE as usize;
        let mut out = vec![0u8; t * t * 4];
        let mut drew = false;
        let mut failed = None;
        // Regions a tile spans on a side, and the pixels each gives it (zoom 0: half a region each).
        let level = (-z) as u32;
        let (n, px) = if z == 0 { (1i64, W) } else { (1i64 << (level - 1), W >> level) };
        let (rx0, rz0) = if z == 0 { (x.div_euclid(2), y.div_euclid(2)) } else { (x * n, y * n) };
        let (ox, oz) = if z == 0 { (x.rem_euclid(2) as usize * t, y.rem_euclid(2) as usize * t) } else { (0, 0) };
        for rz in rz0..rz0 + n {
            for rx in rx0..rx0 + n {
                let (Ok(rx), Ok(rz)) = (i32::try_from(rx), i32::try_from(rz)) else { continue };
                if !self.regions.contains(&(rx, rz)) {
                    continue;
                }
                let pic = match self.picture(rx, rz, level) {
                    Ok(p) => p,
                    Err(e) => {
                        failed.get_or_insert(e.to_string());
                        continue;
                    }
                };
                // Where this region's pixels land in the tile, and which of them.
                let (dx, dz) = (((rx as i64 - rx0) as usize) * px, ((rz as i64 - rz0) as usize) * px);
                let span = px.min(t);
                for row in 0..span {
                    let src = ((oz + row) * px + ox) * 4;
                    let dst = ((dz + row) * t + dx) * 4;
                    out[dst..dst + span * 4].copy_from_slice(&pic[src..src + span * 4]);
                }
                drew = true;
            }
        }
        if drew && out.as_chunks::<4>().0.iter().any(|p| p[3] > 0) {
            Ok(Rendered::Rgba(out))
        } else if let Some(e) = failed {
            Err(TileError::Failed(e))
        } else {
            Ok(Rendered::Empty)
        }
    }
}

impl TileSource for Surveyor {
    fn render(&self, _layer: &str, z: i32, x: i64, y: i64) -> Result<Rendered, TileError> {
        self.tile(z, x, y)
    }

    fn opaque(&self, _layer: &str) -> bool {
        false
    }

    fn version(&self) -> u32 {
        RENDER_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_folders_follow_minecraft_1_21_1() {
        let w = Path::new("/w");
        assert_eq!(dir_for(w, "minecraft:overworld"), Path::new("/w/data/surveyor"));
        assert_eq!(dir_for(w, "minecraft:the_nether"), Path::new("/w/DIM-1/data/surveyor"));
        assert_eq!(dir_for(w, "minecraft:the_end"), Path::new("/w/DIM1/data/surveyor"));
        assert_eq!(dir_for(w, "iceandfire:dread_land"), Path::new("/w/dimensions/iceandfire/dread_land/data/surveyor"));
        assert_eq!(dir_for(w, "a:b/c"), Path::new("/w/dimensions/a/b/c/data/surveyor"));
        assert_eq!(default_max_y("minecraft:the_nether"), Some(NETHER_ROOF));
        assert_eq!(default_max_y("minecraft:overworld"), None);
    }

    #[test]
    fn region_file_names() {
        assert_eq!(region_of("c.0.0.dat"), Some((0, 0)));
        assert_eq!(region_of("c.-3.40002.dat"), Some((-3, 40002)));
        assert_eq!(region_of("s.0.0.dat"), None);
        assert_eq!(region_of("c.0.dat"), None);
        assert_eq!(region_of("c.a.0.dat"), None);
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_down_to_nine_tenths() {
        let mut c = Cache { map: HashMap::new(), bytes: 0, cap: 100, clock: 0 };
        for i in 0..10 {
            c.put((i, 0, 0), Arc::new(vec![0; 10]));
        }
        assert_eq!(c.bytes, 100);
        assert!(c.get((0, 0, 0)).is_some());
        c.put((10, 0, 0), Arc::new(vec![0; 10]));
        assert!(c.bytes <= 90);
        assert!(c.get((0, 0, 0)).is_some(), "used recently, kept");
        assert!(c.get((1, 0, 0)).is_none(), "oldest, dropped");
        c.put((0, 0, 0), Arc::new(vec![0; 5]));
        assert_eq!(c.bytes, c.map.values().map(|e| e.0.len()).sum::<usize>());
    }
}
