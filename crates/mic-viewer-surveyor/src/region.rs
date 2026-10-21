// SPDX-License-Identifier: MPL-2.0
//! One Surveyor region file, `c.<rx>.<rz>.dat`: its palettes and the top floor of every column it
//! recorded.
//!
//! The layout (mic-rtsim VIEWER.md §9, read from Surveyor 1.2.4's `RegionSummary`, `ChunkSummary`,
//! `LayerSummary` and `uints` classes and checked against a world's region files):
//!
//! - `blocks` and `biomes`: lists of ids; `blockColors` (each block's default map colour, RGB),
//!   `biomeWater`, `biomeGrass` and `biomeFoliage`: int arrays in the same orders.
//! - `chunks`: keyed by absolute chunk coordinates `"cx,cz"`, each `{layers: {"<y>": layer}}` where
//!   `<y>` is the top of a height band that runs down to the next key.
//! - A layer: `found`, a 256-bit column mask (a long array, trailing zero words dropped; column `i`
//!   is `x = i / 16`, `z = i % 16` in the chunk), and arrays with one entry per found column in mask
//!   order: `depth` (the floor is at `y = key - depth`), `biome`, `block`, `light`, `water` (water
//!   blocks above the floor) and `glint`. Each array is stored in the narrowest form that fits (see
//!   [`uints`]).
//! - A plant, carpet or snow layer standing on the floor is recorded in its place, with its height.

use std::io;

use crate::nbt::Tag;

/// A region's edge in blocks.
pub const REGION: i32 = 512;
/// Columns in a region.
pub const COLUMNS: usize = (REGION * REGION) as usize;

/// One column's top floor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Floor {
    /// The floor block's height.
    pub y: i32,
    /// Index into [`Region::blocks`] and [`Region::block_colors`].
    pub block: u32,
    /// Index into [`Region::biomes`] and the biome colours.
    pub biome: u32,
    /// Water blocks above the floor.
    pub water: u32,
}

/// A parsed region.
#[derive(Clone, Debug)]
pub struct Region {
    pub rx: i32,
    pub rz: i32,
    pub blocks: Vec<String>,
    pub block_colors: Vec<u32>,
    pub biomes: Vec<String>,
    pub biome_water: Vec<u32>,
    pub biome_grass: Vec<u32>,
    pub biome_foliage: Vec<u32>,
    /// Chunks of this region the file held.
    pub chunks: usize,
    /// Row-major, north row first: index `(z - rz * 512) * 512 + (x - rx * 512)`.
    floors: Vec<Option<Floor>>,
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// One `uints` array as `n` values, as Surveyor's `UInts.readNbt` reads it: absent is all 0; a byte,
/// short or int is every column's value; a byte array is unsigned bytes when it holds `n`, else two
/// values a byte, high nibble first; an int array is ints when it holds `n`, else two unsigned shorts
/// an int, high half first.
pub fn uints(tag: Option<&Tag>, n: usize) -> io::Result<Vec<u32>> {
    let short = |len: usize, what: &str| {
        if len < n.div_ceil(2) { Err(invalid(format!("{what} of {len} for {n} columns"))) } else { Ok(()) }
    };
    Ok(match tag {
        None => vec![0; n],
        Some(Tag::Byte(v)) => vec![*v as u8 as u32; n],
        Some(Tag::Short(v)) => vec![*v as u16 as u32; n],
        Some(Tag::Int(v)) => vec![*v as u32; n],
        Some(Tag::ByteArray(a)) if a.len() == n => a.iter().map(|&b| b as u32).collect(),
        Some(Tag::ByteArray(a)) => {
            short(a.len(), "nibble array")?;
            (0..n).map(|i| u32::from(if i % 2 == 0 { a[i / 2] >> 4 } else { a[i / 2] & 15 })).collect()
        }
        Some(Tag::IntArray(a)) if a.len() == n => a.iter().map(|&v| v as u32).collect(),
        Some(Tag::IntArray(a)) => {
            short(a.len(), "packed short array")?;
            (0..n).map(|i| if i % 2 == 0 { a[i / 2] as u32 >> 16 } else { a[i / 2] as u32 & 0xFFFF }).collect()
        }
        Some(other) => return Err(invalid(format!("a column array of unexpected type {other:?}"))),
    })
}

/// The set bits of a `found` mask below 256, in order.
pub fn found_columns(tag: &Tag) -> io::Result<Vec<usize>> {
    let words: Vec<u64> = match tag {
        Tag::LongArray(v) => v.iter().map(|&w| w as u64).collect(),
        Tag::Long(v) => vec![*v as u64],
        other => return Err(invalid(format!("a found mask of unexpected type {other:?}"))),
    };
    Ok((0..256).filter(|&i| words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)).collect())
}

fn colors(root: &Tag, key: &str) -> Vec<u32> {
    match root.get(key) {
        Some(Tag::IntArray(v)) => v.iter().map(|&c| c as u32 & 0xFF_FFFF).collect(),
        _ => Vec::new(),
    }
}

fn names(root: &Tag, key: &str) -> Vec<String> {
    root.get(key).map(|l| l.items().iter().map(|t| t.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

impl Region {
    /// The region from a file's root compound. Floors above `max_y` are skipped, so the next band's
    /// floor shows (the Nether's bedrock roof, for one).
    pub fn from_nbt(root: &Tag, rx: i32, rz: i32, max_y: Option<i32>) -> io::Result<Region> {
        let mut floors = vec![None; COLUMNS];
        let mut chunks = 0;
        let mut bands: Vec<(i32, &Tag)> = Vec::new();
        for (key, chunk) in root.get("chunks").map(Tag::entries).unwrap_or_default() {
            let Some((cx, cz)) =
                key.split_once(',').and_then(|(a, b)| Some((a.parse::<i32>().ok()?, b.parse::<i32>().ok()?)))
            else {
                return Err(invalid(format!("chunk key {key:?}")));
            };
            // A chunk filed under another region is not this one's to draw.
            if cx >> 5 != rx || cz >> 5 != rz {
                continue;
            }
            chunks += 1;
            bands.clear();
            for (k, layer) in chunk.get("layers").map(Tag::entries).unwrap_or_default() {
                let y = k.parse::<i32>().map_err(|_| invalid(format!("layer key {k:?}")))?;
                bands.push((y, layer));
            }
            // The highest band first: its floor is the one seen from above.
            bands.sort_by_key(|b| std::cmp::Reverse(b.0));
            let (ox, oz) = ((cx & 31) as usize * 16, (cz & 31) as usize * 16);
            for &(top, layer) in &bands {
                let Some(found) = layer.get("found") else { continue };
                let cols = found_columns(found)?;
                let n = cols.len();
                let depth = uints(layer.get("depth"), n)?;
                let block = uints(layer.get("block"), n)?;
                let biome = uints(layer.get("biome"), n)?;
                let water = uints(layer.get("water"), n)?;
                for (j, &i) in cols.iter().enumerate() {
                    let y = top - depth[j] as i32;
                    if max_y.is_some_and(|m| y > m) {
                        continue;
                    }
                    let slot = &mut floors[(oz + i % 16) * REGION as usize + ox + i / 16];
                    if slot.is_none() {
                        *slot = Some(Floor { y, block: block[j], biome: biome[j], water: water[j] });
                    }
                }
            }
        }
        Ok(Region {
            rx,
            rz,
            blocks: names(root, "blocks"),
            block_colors: colors(root, "blockColors"),
            biomes: names(root, "biomes"),
            biome_water: colors(root, "biomeWater"),
            biome_grass: colors(root, "biomeGrass"),
            biome_foliage: colors(root, "biomeFoliage"),
            chunks,
            floors,
        })
    }

    /// The top floor at a block inside the region (`lx`, `lz` in 0..512), if Surveyor recorded one.
    pub fn floor(&self, lx: usize, lz: usize) -> Option<Floor> {
        self.floors[lz * REGION as usize + lx]
    }

    /// The floor at absolute block coordinates, if inside this region and recorded.
    pub fn floor_at(&self, x: i32, z: i32) -> Option<Floor> {
        let (lx, lz) = (x - self.rx * REGION, z - self.rz * REGION);
        if !(0..REGION).contains(&lx) || !(0..REGION).contains(&lz) {
            return None;
        }
        self.floor(lx as usize, lz as usize)
    }

    /// Columns with a floor.
    pub fn known(&self) -> usize {
        self.floors.iter().filter(|f| f.is_some()).count()
    }

    /// The block id of a floor.
    pub fn block_name(&self, f: &Floor) -> &str {
        self.blocks.get(f.block as usize).map_or("", String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uints_reads_every_form_surveyor_writes() {
        assert_eq!(uints(None, 3).unwrap(), [0, 0, 0]);
        assert_eq!(uints(Some(&Tag::Byte(-56)), 2).unwrap(), [200, 200]);
        assert_eq!(uints(Some(&Tag::Short(-1)), 1).unwrap(), [65535]);
        assert_eq!(uints(Some(&Tag::Int(70000)), 2).unwrap(), [70000, 70000]);
        // Bytes: unsigned.
        assert_eq!(uints(Some(&Tag::ByteArray(vec![0, 176, 255])), 3).unwrap(), [0, 176, 255]);
        // Nibbles: high first (UNibbleArray.get), an odd count leaving the last low nibble unused.
        assert_eq!(uints(Some(&Tag::ByteArray(vec![0x12, 0xF0])), 3).unwrap(), [1, 2, 15]);
        assert_eq!(uints(Some(&Tag::ByteArray(vec![0x12, 0x34])), 4).unwrap(), [1, 2, 3, 4]);
        // Ints, and shorts packed high first (UShortArray.ofPacked).
        assert_eq!(uints(Some(&Tag::IntArray(vec![1, -1, 3])), 3).unwrap(), [1, u32::MAX, 3]);
        assert_eq!(uints(Some(&Tag::IntArray(vec![(300 << 16) | 400, 500 << 16])), 3).unwrap(), [300, 400, 500]);
        // Too short for the count.
        assert!(uints(Some(&Tag::ByteArray(vec![1])), 4).is_err());
        assert!(uints(Some(&Tag::IntArray(vec![1])), 3).is_err());
        assert!(uints(Some(&Tag::String("x".into())), 1).is_err());
    }

    #[test]
    fn found_masks_read_little_end_first_with_trailing_words_dropped() {
        assert_eq!(found_columns(&Tag::LongArray(vec![0b101])).unwrap(), [0, 2]);
        assert_eq!(found_columns(&Tag::LongArray(vec![0, i64::MIN])).unwrap(), [127]);
        assert_eq!(found_columns(&Tag::LongArray(vec![-1, -1, -1, -1])).unwrap().len(), 256);
        assert_eq!(found_columns(&Tag::LongArray(vec![])).unwrap().len(), 0);
        assert!(found_columns(&Tag::Int(1)).is_err());
    }
}
