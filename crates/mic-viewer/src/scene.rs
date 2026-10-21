// SPDX-License-Identifier: MPL-2.0
//! A published view of a world: each layer's marks, grouped by 512-block region with counts, so a
//! request costs the regions the viewport covers plus the marks it returns (VIEWER.md §6), never
//! the rows outside the view.
//!
//! Sources write marks into a [`SceneWriter`] (VIEWER.md's `PartWriter`: generic rows of a known
//! mark kind, so one reader serves every layer) and publish the [`Scene`] it builds. A scene is
//! immutable; a source publishes a new one when its state changes and readers hold an `Arc` to the
//! one they started with.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hasher;
use std::io::Write;
use std::sync::Arc;

use crate::api::{Mark, Tick};

/// The region edge in blocks (VIEWER.md §4.3, TP4's canonical region).
pub const REGION: f64 = 512.0;
/// A mark whose box covers more regions than this is kept on a list every query checks.
const WIDE_REGIONS: i64 = 64;

/// A region's key: `floor(x / 512)`, `floor(z / 512)`.
pub type RegionKey = (i32, i32);

pub fn region_of(x: f64, z: f64) -> RegionKey {
    ((x / REGION).floor() as i32, (z / REGION).floor() as i32)
}

/// One layer's marks in one frame. Cloning one is cheap (the marks and their index are shared), so
/// a backend can compose a new scene from an old one's layers and a few new ones.
#[derive(Clone, Debug, Default)]
pub struct LayerRows {
    /// Changes when the marks change (a hash of them), so the page refetches only what moved.
    pub version: u64,
    /// Why the layer has no marks, when it has none (`/api/marks` passes it on as `empty`).
    pub empty: Option<String>,
    rows: Arc<Rows>,
}

#[derive(Debug, Default)]
struct Rows {
    marks: Vec<Mark>,
    /// Region -> indices of the marks whose box touches it.
    by_region: HashMap<RegionKey, Vec<u32>>,
    /// Marks too big to index by region.
    wide: Vec<u32>,
    /// Marks anchored in each region (the region counts of the zoomed-out levels).
    counts: HashMap<RegionKey, u32>,
}

impl LayerRows {
    pub fn from_marks(marks: Vec<Mark>) -> LayerRows {
        let version = version_of(&marks);
        let mut rows = Rows::default();
        for (i, m) in marks.iter().enumerate() {
            let i = i as u32;
            let b = m.bbox();
            let (r0, r1) = (region_of(b[0], b[1]), region_of(b[2], b[3]));
            let span = (i64::from(r1.0) - i64::from(r0.0) + 1) * (i64::from(r1.1) - i64::from(r0.1) + 1);
            if span > WIDE_REGIONS {
                rows.wide.push(i);
            } else {
                for rx in r0.0..=r1.0 {
                    for rz in r0.1..=r1.1 {
                        rows.by_region.entry((rx, rz)).or_default().push(i);
                    }
                }
            }
            let a = m.anchor(Tick::MIN);
            *rows.counts.entry(region_of(a[0], a[1])).or_default() += 1;
        }
        rows.marks = marks;
        LayerRows { version, empty: None, rows: Arc::new(rows) }
    }

    /// No marks, and why: a module the world does not run, a part that failed to read.
    pub fn empty_because(reason: impl Into<String>) -> LayerRows {
        let reason = reason.into();
        LayerRows {
            version: version_of(&[Mark::Label { x: 0.0, z: 0.0, text: reason.clone() }]),
            empty: Some(reason),
            ..LayerRows::default()
        }
    }

    pub fn len(&self) -> usize {
        self.rows.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.marks.is_empty()
    }

    pub fn marks(&self) -> &[Mark] {
        &self.rows.marks
    }

    /// Marks anchored per region.
    pub fn region_counts(&self) -> &HashMap<RegionKey, u32> {
        &self.rows.counts
    }

    /// The marks whose box meets `b` (`[x0, z0, x1, z1]`) or one of its copies a `period` east or
    /// west (0: no wrap), in their stored order.
    pub fn in_box(&self, b: [f64; 4], period: f64) -> Vec<&Mark> {
        let boxes = copies(b, period);
        let regions: i64 = boxes
            .iter()
            .map(|q| {
                let (r0, r1) = (region_of(q[0], q[1]), region_of(q[2], q[3]));
                (i64::from(r1.0) - i64::from(r0.0) + 1) * (i64::from(r1.1) - i64::from(r0.1) + 1)
            })
            .sum();
        let hit = |m: &Mark| {
            let mb = m.bbox();
            boxes.iter().any(|q| mb[0] <= q[2] && mb[2] >= q[0] && mb[1] <= q[3] && mb[3] >= q[1])
        };
        let rows = &*self.rows;
        if regions as usize >= rows.marks.len() / 4 || regions > 1 << 16 {
            // Cheaper to look at every mark than at every region.
            return rows.marks.iter().filter(|m| hit(m)).collect();
        }
        let mut idx: Vec<u32> = rows.wide.clone();
        for q in &boxes {
            let (r0, r1) = (region_of(q[0], q[1]), region_of(q[2], q[3]));
            for rx in r0.0..=r1.0 {
                for rz in r0.1..=r1.1 {
                    if let Some(v) = rows.by_region.get(&(rx, rz)) {
                        idx.extend_from_slice(v);
                    }
                }
            }
        }
        idx.sort_unstable();
        idx.dedup();
        idx.into_iter().map(|i| &rows.marks[i as usize]).filter(|m| hit(m)).collect()
    }
}

/// `b` and, on a wrapping frame, its copies one period east and west, cut to one period wide.
pub fn copies(b: [f64; 4], period: f64) -> Vec<[f64; 4]> {
    if period <= 0.0 {
        return vec![b];
    }
    if b[2] - b[0] >= period {
        return vec![[-period, b[1], period, b[3]]];
    }
    vec![b, [b[0] - period, b[1], b[2] - period, b[3]], [b[0] + period, b[1], b[2] + period, b[3]]]
}

/// A hash of the marks, as an integer JavaScript holds exactly (53 bits).
pub fn version_of(marks: &[Mark]) -> u64 {
    struct H(std::collections::hash_map::DefaultHasher);
    impl Write for H {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.write(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut h = H(std::collections::hash_map::DefaultHasher::new());
    let _ = serde_json::to_writer(&mut h, marks);
    h.0.write_usize(marks.len());
    h.0.finish() & ((1 << 53) - 1)
}

/// A frame's layers at one time.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// The game tick the marks are as of.
    pub t: Tick,
    /// The frame's east-west wrap period (0: none).
    pub period_x: f64,
    pub layers: HashMap<String, LayerRows>,
}

impl Scene {
    pub fn versions(&self) -> BTreeMap<String, u64> {
        self.layers.iter().map(|(k, v)| (k.clone(), v.version)).collect()
    }
}

/// Collects marks by layer and builds a [`Scene`].
#[derive(Default)]
pub struct SceneWriter {
    layers: BTreeMap<String, Vec<Mark>>,
    empty: BTreeMap<String, String>,
}

impl SceneWriter {
    pub fn new() -> SceneWriter {
        SceneWriter::default()
    }

    pub fn mark(&mut self, layer: &str, m: Mark) {
        match self.layers.get_mut(layer) {
            Some(v) => v.push(m),
            None => {
                self.layers.insert(layer.to_owned(), vec![m]);
            }
        }
    }

    /// Declares a layer with no marks (so it has a version and answers empty, not unknown).
    pub fn layer(&mut self, layer: &str) {
        self.layers.entry(layer.to_owned()).or_default();
    }

    /// Declares a layer with no marks and says why (the page shows the reason in its layer list).
    /// A mark for the layer later wins: a layer with marks has no reason to be empty.
    pub fn empty(&mut self, layer: &str, reason: impl Into<String>) {
        self.layer(layer);
        self.empty.insert(layer.to_owned(), reason.into());
    }

    pub fn finish(mut self, t: Tick, period_x: f64) -> Scene {
        let layers = self
            .layers
            .into_iter()
            .map(|(k, v)| {
                let rows = match self.empty.remove(&k) {
                    Some(why) if v.is_empty() => LayerRows::empty_because(why),
                    _ => LayerRows::from_marks(v),
                };
                (k, rows)
            })
            .collect();
        Scene { t, period_x, layers }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_floor_toward_minus_infinity() {
        assert_eq!(region_of(0.0, 0.0), (0, 0));
        assert_eq!(region_of(511.9, -0.1), (0, -1));
        assert_eq!(region_of(-512.0, 1024.0), (-1, 2));
    }

    #[test]
    fn box_queries_find_what_touches_and_only_that() {
        let mut w = SceneWriter::new();
        for i in 0..100 {
            w.mark("p", Mark::point(i, f64::from(i) * 100.0, 0.0));
        }
        // A wide road from far west to far east, and an area straddling regions.
        w.mark("p", Mark::edge([-100_000.0, 50.0], [100_000.0, 50.0], 2.0));
        w.mark("p", Mark::disc(Some("d".into()), 1000.0, 600.0, 200.0));
        let s = w.finish(0, 0.0);
        let rows = &s.layers["p"];
        assert_eq!(rows.len(), 102);
        let got = rows.in_box([950.0, -10.0, 1250.0, 60.0], 0.0);
        let ids: Vec<_> = got.iter().filter_map(|m| m.id()).collect();
        assert_eq!(ids, ["10", "11", "12"]);
        assert_eq!(got.len(), 4, "the three points and the wide edge");
        let got = rows.in_box([1100.0, 700.0, 1150.0, 750.0], 0.0);
        assert_eq!(got.len(), 1, "the disc's box");
        // Every point is counted once in its anchor region.
        assert_eq!(rows.region_counts().values().map(|&c| c as usize).sum::<usize>(), 102);
        // The whole world at once takes the scan path and agrees.
        assert_eq!(rows.in_box([-1e9, -1e9, 1e9, 1e9], 0.0).len(), 102);
    }

    #[test]
    fn wrapped_frames_see_across_the_seam() {
        let mut w = SceneWriter::new();
        w.mark("p", Mark::point("east", 990.0, 0.0));
        w.mark("p", Mark::point("west", -990.0, 0.0));
        let s = w.finish(0, 2000.0);
        let rows = &s.layers["p"];
        // A view just east of the seam (x 980 to 1020) sees the west point's copy at 1010.
        let got: Vec<_> = rows.in_box([980.0, -5.0, 1020.0, 5.0], 2000.0).iter().filter_map(|m| m.id()).collect();
        assert_eq!(got, ["east", "west"]);
        let got: Vec<_> = rows.in_box([980.0, -5.0, 1020.0, 5.0], 0.0).iter().filter_map(|m| m.id()).collect();
        assert_eq!(got, ["east"]);
    }

    #[test]
    fn versions_follow_the_content() {
        let a = vec![Mark::point(1, 0.0, 0.0)];
        let b = vec![Mark::point(1, 0.0, 1.0)];
        assert_eq!(version_of(&a), version_of(&a.clone()));
        assert_ne!(version_of(&a), version_of(&b));
        assert!(version_of(&b) < 1 << 53);
    }

    #[test]
    fn an_empty_layer_says_why_until_it_has_marks() {
        let mut w = SceneWriter::new();
        w.empty("dragons", "no dragons module in this save");
        w.empty("ships", "the ships part failed to read");
        w.mark("ships", Mark::point("s", 0.0, 0.0));
        w.layer("herds");
        let s = w.finish(0, 0.0);
        assert_eq!(s.layers["dragons"].empty.as_deref(), Some("no dragons module in this save"));
        assert!(s.layers["dragons"].is_empty());
        assert_eq!(s.layers["ships"].empty, None, "a mark wins over the reason");
        assert_eq!(s.layers["ships"].len(), 1);
        assert_eq!(s.layers["herds"].empty, None);
        // A new reason is a new version: the page refetches and shows it.
        let other = LayerRows::empty_because("another reason");
        assert_ne!(other.version, s.layers["dragons"].version);
        assert_ne!(s.layers["herds"].version, s.layers["dragons"].version);
    }
}
