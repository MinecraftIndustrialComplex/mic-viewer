// SPDX-License-Identifier: MPL-2.0
//! The mark budget (VIEWER.md §6): a request returns at most [`crate::api::MARK_BUDGET`] marks, shared among the
//! asked layers; a layer with more marks in view than its share is binned on a 32 px screen grid and
//! answered with clusters.
//!
//! **Sharing.** Water filling: every layer is offered an equal part of what is left; a layer that
//! needs less takes only what it needs and the rest is offered again to the others. What cannot be
//! split evenly goes to the higher priorities first. So no visible layer is starved by a busier one,
//! and a quiet layer always shows all of its marks.
//!
//! **Binning.** Points and legs (at their place at time `t`) fall into cells of 32 screen pixels;
//! a cell with one mark keeps the mark, a cell with more becomes a `cluster` with the count per
//! legend entry. If that is still over the share, the cells double until it fits. An edge, path or
//! area smaller than a bin is a dot at that zoom and bins the same way. Larger shapes and cells
//! cannot be binned into a bubble: over the share, the largest are kept (a long road matters more
//! at a distance than a short one), and `shown` says how many of `total`.

use std::collections::BTreeMap;

use crate::api::{LayerMarks, Mark, Tick};

/// The screen grid for binning, CSS pixels.
pub const BIN_PX: f64 = 32.0;

/// Splits `budget` among layers that each have `need` marks in view, in priority order (highest
/// first: `needs` is given in that order). Returns each layer's share.
pub fn share(budget: usize, needs: &[usize]) -> Vec<usize> {
    let mut out = vec![0usize; needs.len()];
    let mut open: Vec<usize> = (0..needs.len()).filter(|&i| needs[i] > 0).collect();
    let mut left = budget;
    while !open.is_empty() && left > 0 {
        let fair = left / open.len();
        let (small, big): (Vec<usize>, Vec<usize>) = open.iter().partition(|&&i| needs[i] - out[i] <= fair.max(1));
        if small.is_empty() || fair == 0 {
            // Everyone wants more than the fair part: each takes it, the remainder by priority.
            let mut rem = left - fair * open.len();
            for &i in &open {
                out[i] += fair;
                if rem > 0 {
                    out[i] += 1;
                    rem -= 1;
                }
            }
            break;
        }
        for &i in &small {
            left -= needs[i] - out[i];
            out[i] = needs[i];
        }
        open = big;
    }
    out
}

/// Reduces one layer's marks in view to at most `share` (see the module docs). `bpp` is blocks per
/// CSS pixel; `t` places legs.
pub fn reduce(marks: Vec<&Mark>, share: usize, bpp: f64, t: Tick) -> LayerMarks {
    let total = marks.len() as u64;
    if marks.len() <= share {
        return LayerMarks {
            version: 0,
            total,
            shown: total,
            clustered: false,
            marks: marks.into_iter().cloned().collect(),
        };
    }
    if share == 0 {
        return LayerMarks { version: 0, total, shown: 0, clustered: false, marks: Vec::new() };
    }
    // A shape smaller than a bin is a dot at this zoom: it bins like a point.
    let bin_blocks = BIN_PX * bpp;
    let (movable, fixed): (Vec<&Mark>, Vec<&Mark>) = marks.into_iter().partition(|m| {
        matches!(m, Mark::Point { .. } | Mark::Leg { .. } | Mark::Cluster { .. } | Mark::Label { .. }) || {
            let b = m.bbox();
            !matches!(m, Mark::Cell { .. }) && (b[2] - b[0]).max(b[3] - b[1]) < bin_blocks
        }
    });
    // Fixed shapes get their part of the share by count, the largest first.
    let fixed_share =
        if movable.is_empty() { share } else { ((share as f64) * fixed.len() as f64 / total as f64).floor() as usize };
    let mut fixed = fixed;
    let mut out_marks: Vec<Mark> = Vec::new();
    let mut shown = 0u64;
    if fixed.len() > fixed_share {
        fixed.sort_by(|a, b| size(b).total_cmp(&size(a)));
        fixed.truncate(fixed_share);
    }
    shown += fixed.len() as u64;
    out_marks.extend(fixed.into_iter().cloned());
    let movable_share = share - out_marks.len();
    let mut clustered = false;
    if movable.len() <= movable_share {
        shown += movable.len() as u64;
        out_marks.extend(movable.into_iter().cloned());
    } else if movable_share > 0 {
        clustered = true;
        let mut cell = BIN_PX * bpp.max(1e-6);
        loop {
            let bins = bin(&movable, cell, t);
            if bins.len() <= movable_share {
                for b in bins.into_values() {
                    if b.members.len() == 1 {
                        out_marks.push(b.members[0].clone());
                        shown += 1;
                    } else {
                        let n = b.members.iter().map(|m| weight(m)).sum::<u64>();
                        let mut by: BTreeMap<String, u64> = BTreeMap::new();
                        for m in &b.members {
                            match m {
                                Mark::Cluster { by: inner, .. } => {
                                    for (k, v) in inner {
                                        *by.entry(k.clone()).or_default() += v;
                                    }
                                }
                                _ => {
                                    if let Some(ci) = m.legend_index() {
                                        *by.entry(ci.to_string()).or_default() += 1;
                                    }
                                }
                            }
                        }
                        let k = b.members.len() as f64;
                        out_marks.push(Mark::Cluster { x: b.sx / k, z: b.sz / k, n, by });
                    }
                }
                break;
            }
            cell *= 2.0;
        }
    }
    LayerMarks { version: 0, total, shown, clustered, marks: out_marks }
}

/// What a mark counts for in a cluster (a cluster from a source counts its own `n`).
fn weight(m: &Mark) -> u64 {
    match m {
        Mark::Cluster { n, .. } => *n,
        _ => 1,
    }
}

/// A fixed shape's size, for keeping the largest: its box's diagonal, times a line's width.
fn size(m: &Mark) -> f64 {
    let b = m.bbox();
    let d = (b[2] - b[0]).hypot(b[3] - b[1]);
    match m {
        Mark::Edge { w, .. } | Mark::Path { w, .. } => d * w.unwrap_or(1.0).max(0.1),
        Mark::Cell { w, h, .. } => d.max(f64::from(*w) * f64::from(*h)),
        _ => d,
    }
}

struct Bin<'a> {
    members: Vec<&'a Mark>,
    sx: f64,
    sz: f64,
}

/// Bins by cell, in a stable order (cell key).
fn bin<'a>(marks: &[&'a Mark], cell: f64, t: Tick) -> BTreeMap<(i64, i64), Bin<'a>> {
    let mut bins: BTreeMap<(i64, i64), Bin<'a>> = BTreeMap::new();
    for m in marks {
        let [x, z] = m.anchor(t);
        let key = ((x / cell).floor() as i64, (z / cell).floor() as i64);
        let b = bins.entry(key).or_insert_with(|| Bin { members: Vec::new(), sx: 0.0, sz: 0.0 });
        b.members.push(m);
        b.sx += x;
        b.sz += z;
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_never_pass_the_budget_and_quiet_layers_get_all() {
        assert_eq!(share(2000, &[10, 50_000, 30]), vec![10, 1960, 30]);
        assert_eq!(share(2000, &[50_000, 50_000]), vec![1000, 1000]);
        assert_eq!(share(2000, &[50_000, 50_000, 50_000]), vec![667, 667, 666]);
        assert_eq!(share(2000, &[0, 0]), vec![0, 0]);
        assert_eq!(share(2000, &[1500, 1500]), vec![1000, 1000]);
        assert_eq!(share(2000, &[900, 1500]), vec![900, 1100]);
        assert_eq!(share(3, &[5, 5, 5, 5, 5]), vec![1, 1, 1, 0, 0]);
        for needs in [[1usize, 2, 3], [7000, 1, 7000], [2000, 2000, 2000]] {
            let s = share(2000, &needs);
            assert!(s.iter().sum::<usize>() <= 2000);
            assert!(s.iter().zip(&needs).all(|(a, b)| a <= b));
        }
    }

    #[test]
    fn a_crowded_layer_clusters_within_its_share() {
        // 10,000 points on a 100 x 100 grid, 10 blocks apart, two legend entries.
        let marks: Vec<Mark> = (0..10_000)
            .map(|i| Mark::point(i, f64::from(i % 100) * 10.0, f64::from(i / 100) * 10.0).with_ci((i % 2) as u16))
            .collect();
        let refs: Vec<&Mark> = marks.iter().collect();
        // At 2 blocks a pixel a cell is 64 blocks: about 16 x 16 cells.
        let out = reduce(refs.clone(), 500, 2.0, 0);
        assert!(out.clustered);
        assert!(out.marks.len() <= 500, "{}", out.marks.len());
        let n: u64 = out
            .marks
            .iter()
            .map(|m| match m {
                Mark::Cluster { n, by, .. } => {
                    assert_eq!(by.values().sum::<u64>(), *n);
                    *n
                }
                _ => 1,
            })
            .sum();
        assert_eq!(n, 10_000, "every mark is in a cluster or shown");
        // A tiny share doubles the cells until it fits.
        let out = reduce(refs.clone(), 3, 2.0, 0);
        assert!(out.marks.len() <= 3);
        // Within its share, nothing changes.
        let out = reduce(refs[..20].to_vec(), 500, 2.0, 0);
        assert!(!out.clustered);
        assert_eq!(out.marks.len(), 20);
    }

    #[test]
    fn small_shapes_bin_like_points() {
        // 5,000 district boxes of 32 blocks; at 8 blocks a pixel a bin is 256 blocks.
        let marks: Vec<Mark> = (0..5000)
            .map(|i| {
                let (x, z) = (f64::from(i % 100) * 40.0, f64::from(i / 100) * 40.0);
                Mark::rect(Some(i.to_string()), [x, z, x + 32.0, z + 32.0])
            })
            .collect();
        let out = reduce(marks.iter().collect(), 100, 8.0, 0);
        assert!(out.clustered);
        assert!(out.marks.len() <= 100);
        let n: u64 = out.marks.iter().map(|m| if let Mark::Cluster { n, .. } = m { *n } else { 1 }).sum();
        assert_eq!(n, 5000);
    }

    #[test]
    fn fixed_shapes_keep_the_largest() {
        let marks: Vec<Mark> = (1..=10).map(|i| Mark::edge([0.0, 0.0], [f64::from(i) * 100.0, 0.0], 1.0)).collect();
        let out = reduce(marks.iter().collect(), 3, 1.0, 0);
        assert_eq!(out.shown, 3);
        assert_eq!(out.total, 10);
        let lens: Vec<f64> = out.marks.iter().map(|m| m.bbox()[2]).collect();
        assert_eq!(lens, [1000.0, 900.0, 800.0]);
    }
}
