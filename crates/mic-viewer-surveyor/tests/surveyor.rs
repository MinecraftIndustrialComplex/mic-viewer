// SPDX-License-Identifier: MPL-2.0
//! Surveyor region files written the way Surveyor 1.2.4 writes them (`UInts.fromUInts`), read back
//! and drawn as tiles.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use mic_viewer::tiles::{Rendered, TILE, TileError};
use mic_viewer_surveyor::nbt::{Tag, write};
use mic_viewer_surveyor::region::Region;
use mic_viewer_surveyor::render::{GRASS, PLANT};
use mic_viewer_surveyor::{MIN_ZOOM, Surveyor, dir_for};

/// Surveyor's `UInts.fromUInts(values, 0)` as NBT: absent when all 0, a scalar when all equal,
/// else nibbles (high first), bytes, packed shorts (high first) or ints, the narrowest that fits.
fn encode(v: &[u32]) -> Option<Tag> {
    if v.iter().all(|&x| x == v[0]) {
        return match v[0] {
            0 => None,
            x if x <= 255 => Some(Tag::Byte(x as u8 as i8)),
            x if x <= 65535 => Some(Tag::Short(x as u16 as i16)),
            x => Some(Tag::Int(x as i32)),
        };
    }
    let max = *v.iter().max().unwrap();
    Some(if max <= 15 {
        let mut b = vec![0u8; v.len().div_ceil(2)];
        for (i, &x) in v.iter().enumerate() {
            b[i / 2] |= if i % 2 == 0 { (x as u8) << 4 } else { x as u8 };
        }
        Tag::ByteArray(b)
    } else if max <= 255 {
        Tag::ByteArray(v.iter().map(|&x| x as u8).collect())
    } else if max <= 65535 {
        let mut p = vec![0i32; v.len().div_ceil(2)];
        for (i, &x) in v.iter().enumerate() {
            p[i / 2] |= if i % 2 == 0 { (x << 16) as i32 } else { x as i32 };
        }
        Tag::IntArray(p)
    } else {
        Tag::IntArray(v.iter().map(|&x| x as i32).collect())
    })
}

/// One recorded column: index in the chunk (`x * 16 + z`), depth below the band's top, block,
/// biome, water.
#[derive(Clone, Copy)]
struct Col(usize, u32, u32, u32, u32);

fn layer(cols: &[Col]) -> Tag {
    let mut cols = cols.to_vec();
    cols.sort_by_key(|c| c.0);
    let mut words = vec![0i64; 4];
    for c in &cols {
        words[c.0 / 64] |= 1 << (c.0 % 64);
    }
    while words.last() == Some(&0) {
        words.pop();
    }
    let mut out = vec![("found".to_string(), Tag::LongArray(words))];
    for key in ["depth", "biome", "block", "light", "water"] {
        let field = |c: &Col| match key {
            "depth" => c.1,
            "biome" => c.3,
            "block" => c.2,
            "light" => 15,
            _ => c.4,
        };
        if let Some(t) = encode(&cols.iter().map(field).collect::<Vec<_>>()) {
            out.push((key.to_string(), t));
        }
    }
    Tag::Compound(out)
}

const BLOCKS: [(&str, u32); 4] = [
    ("minecraft:stone", 0x70_7070),
    ("minecraft:grass_block", GRASS),
    ("minecraft:oak_leaves", PLANT),
    ("minecraft:sand", 0xF7_E9A3),
];
const BIOME_GRASS: u32 = 0x91_BD59;

/// A chunk by absolute coordinates with its (band top, layer) list.
type Chunk = ((i32, i32), Vec<(i32, Tag)>);

/// A region file.
fn region_file(chunks: &[Chunk]) -> Tag {
    let chunks = chunks
        .iter()
        .map(|((cx, cz), layers)| {
            let layers = layers.iter().map(|(y, l)| (y.to_string(), l.clone())).collect();
            (format!("{cx},{cz}"), Tag::Compound(vec![("layers".into(), Tag::Compound(layers))]))
        })
        .collect();
    Tag::Compound(vec![
        (
            "biomes".into(),
            Tag::List(vec![Tag::String("minecraft:plains".into()), Tag::String("minecraft:ocean".into())]),
        ),
        ("blocks".into(), Tag::List(BLOCKS.iter().map(|b| Tag::String(b.0.into())).collect())),
        ("biomeWater".into(), Tag::IntArray(vec![0x3F_76E4, 0x10_20F0])),
        ("biomeFoliage".into(), Tag::IntArray(vec![0x77_AB2F, 0x71_A74D])),
        ("biomeGrass".into(), Tag::IntArray(vec![BIOME_GRASS as i32, 0x8E_B971])),
        ("blockColors".into(), Tag::IntArray(BLOCKS.iter().map(|b| b.1 as i32).collect())),
        ("chunks".into(), Tag::Compound(chunks)),
    ])
}

struct World(PathBuf);

impl World {
    fn new(name: &str) -> World {
        let p = std::env::temp_dir().join(format!("mic-viewer-surveyor-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(dir_for(&p, "minecraft:overworld")).unwrap();
        World(p)
    }

    fn put(&self, rx: i32, rz: i32, t: &Tag) {
        fs::write(dir_for(&self.0, "minecraft:overworld").join(format!("c.{rx}.{rz}.dat")), write::root_gzip(t))
            .unwrap();
    }

    fn open(&self) -> Surveyor {
        Surveyor::open(dir_for(&self.0, "minecraft:overworld")).unwrap().expect("regions")
    }

    /// Every file's bytes and modification time.
    fn snapshot(&self) -> BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
        fn walk(p: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)>) {
            for e in fs::read_dir(p).unwrap() {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                if m.is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.insert(e.path(), (fs::read(e.path()).unwrap(), m.modified().unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.0, &mut out);
        out
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rgba(r: Result<Rendered, TileError>) -> Vec<u8> {
    match r {
        Ok(Rendered::Rgba(px)) => px,
        Ok(Rendered::Empty) => panic!("empty tile"),
        Err(e) => panic!("{e:?}"),
    }
}

fn pixel(px: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * TILE as usize + x) * 4;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn columns_are_x_major_and_the_floor_is_the_band_top_minus_depth() {
    // Chunk (3, -2): column 1 is x 0, z 1; column 16 is x 1, z 0; column 255 is x 15, z 15.
    let t = region_file(&[(
        (3, -2),
        vec![(256, layer(&[Col(1, 180, 1, 0, 0), Col(16, 170, 0, 0, 0), Col(255, 300, 3, 1, 4)]))],
    )]);
    let r = Region::from_nbt(&t, 0, -1, None).unwrap();
    assert_eq!(r.chunks, 1);
    assert_eq!(r.known(), 3);
    let f = r.floor_at(48, -32 + 1).expect("x 0, z 1");
    assert_eq!((f.y, r.block_name(&f)), (76, "minecraft:grass_block"));
    let f = r.floor_at(49, -32).expect("x 1, z 0");
    assert_eq!((f.y, r.block_name(&f)), (86, "minecraft:stone"));
    let f = r.floor_at(48 + 15, -32 + 15).expect("x 15, z 15");
    // Depth 300 needs packed shorts; water 4 rides along.
    assert_eq!((f.y, f.water, f.biome, r.block_name(&f)), (-44, 4, 1, "minecraft:sand"));
    assert!(r.floor_at(48, -32).is_none());
    assert!(r.floor_at(0, 0).is_none(), "outside the region");
}

#[test]
fn the_highest_band_wins_and_max_y_reaches_under_a_roof() {
    let t = region_file(&[((0, 0), vec![(61, layer(&[Col(0, 10, 3, 0, 0)])), (319, layer(&[Col(0, 5, 0, 0, 0)]))])]);
    let top = Region::from_nbt(&t, 0, 0, None).unwrap();
    assert_eq!(top.floor_at(0, 0).unwrap().y, 314);
    let under = Region::from_nbt(&t, 0, 0, Some(200)).unwrap();
    assert_eq!(under.floor_at(0, 0).unwrap().y, 51);
    assert_eq!(under.block_name(&under.floor_at(0, 0).unwrap()), "minecraft:sand");
}

#[test]
fn every_encoding_surveyor_writes_reads_back() {
    // 256 columns with values that force each form: all equal, nibbles, bytes, shorts, ints.
    for name in ["single", "nibbles", "bytes", "shorts", "ints"] {
        let f = |i: usize| match name {
            "single" => 7u32,
            "nibbles" => (i % 16) as u32,
            "bytes" => (i % 200) as u32,
            "shorts" => (i * 250) as u32,
            _ => 70_000 + i as u32,
        };
        let cols: Vec<Col> = (0..256).map(|i| Col(i, f(i), (i % 4) as u32, 0, 0)).collect();
        let t = region_file(&[((0, 0), vec![(100_000, layer(&cols))])]);
        let r = Region::from_nbt(&t, 0, 0, None).unwrap();
        for i in 0..256 {
            let fl = r.floor_at((i / 16) as i32, (i % 16) as i32).unwrap();
            assert_eq!(fl.y, 100_000 - f(i) as i32, "{name} column {i}");
            assert_eq!(fl.block, (i % 4) as u32, "{name} column {i}");
        }
    }
}

#[test]
fn tiles_draw_one_block_a_pixel_and_coarser_zooms_paste_halvings() {
    let w = World::new("tiles");
    // Region (0, 0): chunk (0, 0) all grass at y 70; chunk (17, 0) (block x 272..288, in the
    // second zoom-0 tile) sand under 3 water.
    let grass: Vec<Col> = (0..256).map(|i| Col(i, 186, 1, 0, 0)).collect();
    let wet: Vec<Col> = (0..256).map(|i| Col(i, 196, 3, 1, 3)).collect();
    w.put(0, 0, &region_file(&[((0, 0), vec![(256, layer(&grass))]), ((17, 0), vec![(256, layer(&wet))])]));
    let s = w.open();
    let before = w.snapshot();

    let t0 = rgba(s.tile(0, 0, 0));
    let p = pixel(&t0, 5, 5);
    assert_eq!(p[3], 255);
    // Flat grass, no relief step: the biome's grass colour at 0.9.
    let g = [(BIOME_GRASS >> 16) & 255, (BIOME_GRASS >> 8) & 255, BIOME_GRASS & 255]
        .map(|c| (c as f32 * 0.9).round() as u8);
    assert_eq!(&p[..3], &g);
    assert_eq!(pixel(&t0, 20, 5)[3], 0, "never recorded: transparent");
    assert_eq!(s.parses(), 1);

    // The other half of the region comes from the cache.
    let t1 = rgba(s.tile(0, 1, 0));
    let p = pixel(&t1, 272 - 256 + 3, 3);
    assert_eq!(p[3], 255);
    assert!(p[2] > p[0], "water is blue: {p:?}");
    assert_eq!(s.parses(), 1);

    // No region there.
    assert!(matches!(s.tile(0, 4, 4), Ok(Rendered::Empty)));
    assert!(matches!(s.tile(0, 0, 1), Ok(Rendered::Empty)), "the region's southern half is all unrecorded");
    // Outside the pyramid.
    assert!(matches!(s.tile(1, 0, 0), Err(TileError::Missing(_))));
    assert!(matches!(s.tile(MIN_ZOOM - 1, 0, 0), Err(TileError::Missing(_))));

    // Zoom -1: the region fills the tile at half scale; the grass chunk is 8 x 8 pixels.
    let c = rgba(s.tile(-1, 0, 0));
    assert_eq!(&pixel(&c, 3, 3)[..3], &g);
    assert_eq!(pixel(&c, 9, 3)[3], 0);
    // The coarsest zoom: the region is 8 pixels in the tile's corner, its first pixel a sixteenth recorded.
    let c = rgba(s.tile(MIN_ZOOM, 0, 0));
    let a = pixel(&c, 0, 0)[3];
    assert!(a > 0 && a < 255, "{a}");
    assert_eq!(pixel(&c, 8, 0)[3], 0);
    assert_eq!(s.parses(), 1, "every halving was cached with the picture");
    // Negative tile indices: region (0, 0) is not in tile (-1, -1) of any zoom.
    assert!(matches!(s.tile(MIN_ZOOM, -1, -1), Ok(Rendered::Empty)));

    assert_eq!(w.snapshot(), before, "the world's folder is never written");
}

#[test]
fn biomes_without_a_tint_keep_the_map_colour() {
    // Biome 1 records grass colour 0x8EB971 and foliage 0x71A74D; a server without colour maps
    // records 0, and the plain map colour stands.
    let t = region_file(&[((0, 0), vec![(256, layer(&[Col(0, 186, 1, 1, 0), Col(1, 186, 2, 1, 0)]))])]);
    let mut r = Region::from_nbt(&t, 0, 0, None).unwrap();
    let f = r.floor_at(0, 0).unwrap();
    assert_eq!(mic_viewer_surveyor::render::floor_color(&r, &f), [142.0, 185.0, 113.0]);
    let leaves = r.floor_at(0, 1).unwrap();
    assert_eq!(mic_viewer_surveyor::render::floor_color(&r, &leaves).map(|v| v.round()), [90.0, 134.0, 62.0]);
    r.biome_grass = vec![0, 0];
    r.biome_foliage = vec![0, 0];
    assert_eq!(mic_viewer_surveyor::render::floor_color(&r, &f), [127.0, 178.0, 56.0]);
    assert_eq!(mic_viewer_surveyor::render::floor_color(&r, &leaves), [0.0, 124.0, 0.0]);
}

#[test]
fn relief_reads_the_region_to_the_north_across_the_seam() {
    let w = World::new("relief");
    // Region (0, -1)'s south row (chunk (0, -1), z 15) at y 90; region (0, 0)'s north row at y 70:
    // lower than its northern neighbour, so darker than flat ground.
    let north: Vec<Col> = (0..256).map(|i| Col(i, 166, 0, 0, 0)).collect();
    let south: Vec<Col> = (0..256).map(|i| Col(i, 186, 0, 0, 0)).collect();
    w.put(0, -1, &region_file(&[((0, -1), vec![(256, layer(&north))])]));
    w.put(0, 0, &region_file(&[((0, 0), vec![(256, layer(&south))])]));
    let s = w.open();
    let t = rgba(s.tile(0, 0, 0));
    let stone = (0x70 as f32 * 0.9).round() as u8;
    assert!(pixel(&t, 3, 0)[0] < stone, "the step down from the north darkens: {:?}", pixel(&t, 3, 0));
    assert_eq!(pixel(&t, 3, 1)[0], stone, "flat inside the chunk");
    assert_eq!(s.parses(), 2, "the northern region was read for its south row");
    let _ = s.tile(0, 0, -1);
    assert_eq!(s.parses(), 3, "drawing it reads it again, once");
}

#[test]
fn bounds_follow_the_largest_cluster() {
    let w = World::new("bounds");
    let one = region_file(&[]);
    for (rx, rz) in [(0, 0), (1, 0), (-2, 3), (40002, 40002)] {
        w.put(rx, rz, &one);
    }
    let s = w.open();
    assert_eq!(s.regions().count(), 4);
    assert_eq!(s.bounds(), [-1024.0, 0.0, 1024.0, 2048.0]);
    let l = s.layer("surveyor", "Terrain (Surveyor)", "tiles/surveyor/{z}/{x}/{y}.png");
    assert_eq!((l.pyramid.kind.as_str(), l.pyramid.min_zoom, l.pyramid.max_zoom), ("square", MIN_ZOOM, 0));
    assert_eq!(l.default_on, Some(true));
    // An empty region file draws nothing.
    assert!(matches!(s.tile(0, 0, 0), Ok(Rendered::Empty)));
}

#[test]
fn no_surveyor_folder_or_no_region_file_is_none() {
    let w = World::new("none");
    assert!(Surveyor::open(w.0.join("nope")).unwrap().is_none());
    assert!(Surveyor::open(dir_for(&w.0, "minecraft:overworld")).unwrap().is_none());
    fs::write(dir_for(&w.0, "minecraft:overworld").join("s.0.0.dat"), b"structures").unwrap();
    assert!(Surveyor::open(dir_for(&w.0, "minecraft:overworld")).unwrap().is_none());
}

#[test]
fn a_corrupt_region_fails_its_tile_and_spares_the_others() {
    let w = World::new("corrupt");
    let grass: Vec<Col> = (0..256).map(|i| Col(i, 186, 1, 0, 0)).collect();
    w.put(0, 0, &region_file(&[((0, 0), vec![(256, layer(&grass))])]));
    fs::write(dir_for(&w.0, "minecraft:overworld").join("c.1.0.dat"), b"not gzip").unwrap();
    let s = w.open();
    assert!(matches!(s.tile(0, 2, 0), Err(TileError::Failed(_))));
    // The coarse tile over both still draws region (0, 0).
    assert_eq!(pixel(&rgba(s.tile(-2, 0, 0)), 0, 0)[3], 255);
}
