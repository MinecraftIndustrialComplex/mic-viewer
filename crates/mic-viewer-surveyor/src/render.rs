// SPDX-License-Identifier: MPL-2.0
//! A region as a picture, one pixel a block: the floor block's map colour (grass and leaves in their
//! biome's colours), relief from the height step to the north, water over the floor by its depth.
//! Columns Surveyor never recorded are transparent, so the base map under this one shows through.

use crate::region::{Floor, REGION, Region};

/// `MapColor.GRASS` (grass blocks): drawn in the biome's grass colour.
pub const GRASS: u32 = 0x7F_B238;
/// `MapColor.PLANT` (leaves, short grass, ferns): drawn in the biome's foliage colour, darkened.
pub const PLANT: u32 = 0x00_7C00;
/// Vanilla's default water colour, for a biome the palette lacks.
pub const WATER: u32 = 0x3F_76E4;
/// No height known.
pub const UNKNOWN: i32 = i32::MIN;

const W: usize = REGION as usize;

fn rgb(c: u32) -> [f32; 3] {
    [(c >> 16 & 255) as f32, (c >> 8 & 255) as f32, (c & 255) as f32]
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| v * k)
}

fn mix(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k]
}

/// A floor's colour before relief and water. A dedicated server has no colour maps loaded, so
/// Surveyor records 0 for the grass and foliage colour of every biome without an override (as in
/// a dedicated server's save); those keep the plain map colour.
pub fn floor_color(r: &Region, f: &Floor) -> [f32; 3] {
    let c = r.block_colors.get(f.block as usize).copied().unwrap_or(0x7F_7F7F);
    let tint = |colors: &[u32]| colors.get(f.biome as usize).copied().filter(|&t| t != 0);
    match c {
        GRASS => rgb(tint(&r.biome_grass).unwrap_or(c)),
        PLANT => tint(&r.biome_foliage).map_or(rgb(c), |t| scale(rgb(t), 0.8)),
        _ => rgb(c),
    }
}

/// The heights of a region's south row (z = 511 inside it), [`UNKNOWN`] where no floor: the relief
/// of the region south of it reads them.
pub fn south_edge(r: &Region) -> Vec<i32> {
    (0..W).map(|x| r.floor(x, W - 1).map_or(UNKNOWN, |f| f.y)).collect()
}

/// The region as RGBA, 512 x 512, north row first. `north` is the south edge of the region north of
/// it ([`south_edge`]), if that one exists; without it the top row is drawn flat.
pub fn region_rgba(r: &Region, north: Option<&[i32]>) -> Vec<u8> {
    let mut out = vec![0u8; W * W * 4];
    for z in 0..W {
        for x in 0..W {
            let Some(f) = r.floor(x, z) else { continue };
            let base = floor_color(r, &f);
            let c = if f.water > 0 {
                let water = rgb(r.biome_water.get(f.biome as usize).copied().unwrap_or(WATER));
                let w = f.water as f32;
                scale(mix(base, water, (0.5 + 0.05 * w).min(0.9)), 1.0 - 0.35 * w.min(24.0) / 24.0)
            } else {
                let up =
                    if z > 0 { r.floor(x, z - 1).map_or(UNKNOWN, |n| n.y) } else { north.map_or(UNKNOWN, |n| n[x]) };
                let step = if up == UNKNOWN { 0.0 } else { (f.y - up).clamp(-5, 5) as f32 };
                scale(base, 0.9 + 0.045 * step)
            };
            let o = (z * W + x) * 4;
            for (i, v) in c.iter().enumerate() {
                out[o + i] = v.round().clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

/// An RGBA image of edge `w` at half the edge: each pixel the alpha-weighted mean of four, so a
/// half-recorded block of four is drawn half transparent in the colour of the recorded ones.
pub fn halve(src: &[u8], w: usize) -> Vec<u8> {
    let h = w / 2;
    let mut out = vec![0u8; h * h * 4];
    for y in 0..h {
        for x in 0..h {
            let mut sum = [0u32; 3];
            let mut alpha = 0u32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((2 * y + dy) * w + 2 * x + dx) * 4;
                let a = src[i + 3] as u32;
                for c in 0..3 {
                    sum[c] += src[i + c] as u32 * a;
                }
                alpha += a;
            }
            let o = (y * h + x) * 4;
            for c in 0..3 {
                out[o + c] = (sum[c] + alpha / 2).checked_div(alpha).unwrap_or(0) as u8;
            }
            out[o + 3] = ((alpha + 2) / 4) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halving_weights_by_alpha() {
        // A 2 x 2 image: two opaque pixels, red and blue, and two transparent ones.
        let src = [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 255, 255, 9, 9, 9, 0];
        assert_eq!(halve(&src, 2), [128, 0, 128, 128]);
        assert_eq!(halve(&[0; 16], 2), [0, 0, 0, 0]);
        assert_eq!(halve(&[10, 20, 30, 255].repeat(4), 2), [10, 20, 30, 255]);
    }
}
