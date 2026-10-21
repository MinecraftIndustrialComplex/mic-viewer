// SPDX-License-Identifier: MPL-2.0
//! Draws a world's Surveyor map to one PNG, or prints a chunk's floors.
//!
//!   cargo run --release -p mic-viewer-surveyor --example render -- <world> [dimension] [--zoom Z] [--out map.png]
//!   cargo run --release -p mic-viewer-surveyor --example render -- <world> [dimension] --dump CX,CZ
//!
//! Without `--zoom` it picks the finest zoom whose picture of the main cluster fits 4,096 pixels.

use std::path::PathBuf;
use std::time::Instant;

use mic_viewer::tiles::{Rendered, TILE, encode_png};
use mic_viewer_surveyor::{MAX_ZOOM, MIN_ZOOM, Surveyor, default_max_y, dir_for};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut world = None;
    let mut dim = "minecraft:overworld".to_string();
    let mut zoom = None;
    let mut out = PathBuf::from("surveyor-map.png");
    let mut dump = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--zoom" => zoom = args.next().and_then(|z| z.parse::<i32>().ok()),
            "--out" => out = args.next().expect("--out FILE").into(),
            "--dump" => dump = args.next(),
            _ if world.is_none() => world = Some(PathBuf::from(a)),
            _ => dim = a,
        }
    }
    let world = world.expect("usage: render <world> [dimension] [--zoom Z] [--out FILE] [--dump CX,CZ]");
    let s = Surveyor::open(dir_for(&world, &dim)).expect("read the folder").expect("no Surveyor regions there");
    let s = s.with_max_y(default_max_y(&dim));
    if let Some(d) = dump {
        let (cx, cz) =
            d.split_once(',').map(|(a, b)| (a.parse::<i32>().unwrap(), b.parse::<i32>().unwrap())).expect("CX,CZ");
        let r = s.read_region(cx >> 5, cz >> 5).expect("region");
        for x in 0..16 {
            for z in 0..16 {
                let (bx, bz) = (cx * 16 + x, cz * 16 + z);
                if let Some(f) = r.floor_at(bx, bz) {
                    println!("{bx} {bz} {} {} {}", f.y, r.block_name(&f), f.water);
                }
            }
        }
        return;
    }
    let b = s.bounds();
    let edge = |z: i32| ((b[2] - b[0]).max(b[3] - b[1]) * f64::from(z).exp2()) as i64;
    let z = zoom.unwrap_or_else(|| (MIN_ZOOM..=MAX_ZOOM).rev().find(|&z| edge(z) <= 4096).unwrap_or(MIN_ZOOM));
    let span = f64::from(TILE) * f64::from(-z).exp2();
    let (x0, y0) = ((b[0] / span).floor() as i64, (b[1] / span).floor() as i64);
    let (x1, y1) = ((b[2] / span).ceil() as i64, (b[3] / span).ceil() as i64);
    let (tw, th) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let t = TILE as usize;
    let (w, h) = (tw * t, th * t);
    let mut img = vec![0u8; w * h * 4];
    let start = Instant::now();
    let (mut drawn, mut failed) = (0, 0);
    for ty in 0..th {
        for tx in 0..tw {
            match s.tile(z, x0 + tx as i64, y0 + ty as i64) {
                Ok(Rendered::Rgba(px)) => {
                    drawn += 1;
                    for row in 0..t {
                        let dst = ((ty * t + row) * w + tx * t) * 4;
                        img[dst..dst + t * 4].copy_from_slice(&px[row * t * 4..(row + 1) * t * 4]);
                    }
                }
                Ok(Rendered::Empty) => {}
                Err(e) => {
                    failed += 1;
                    eprintln!("tile {z}/{}/{}: {e:?}", x0 + tx as i64, y0 + ty as i64);
                }
            }
        }
    }
    let took = start.elapsed();
    std::fs::write(&out, encode_png(w as u32, h as u32, &img).expect("encode")).expect("write");
    let known = img.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
    println!(
        "{dim}: {} regions, bounds {b:?}, zoom {z} ({} blocks a pixel), {w}x{h} px, {drawn} tiles drawn, {failed} failed, \
         {} region parses in {:.0} ms ({:.1} ms a parse), {known} pixels drawn, cache {} MiB -> {}",
        s.regions().count(),
        f64::from(-z).exp2(),
        s.parses(),
        took.as_secs_f64() * 1e3,
        took.as_secs_f64() * 1e3 / s.parses().max(1) as f64,
        s.cached_bytes() >> 20,
        out.display()
    );
}
