// SPDX-License-Identifier: MPL-2.0
//! `cargo run --example create-census -- <world>`: what mic-viewer-create reads from a world's
//! `create_tracks.dat`, and a check of every placed carriage against the carriage entity's own
//! bogey anchors (they must lie on the carriage as drawn). Read only.

use std::path::PathBuf;

use mic_viewer_create::rail::Rail;
use mic_viewer_create::tags::{self, uuid_text};
use mic_viewer_surveyor::nbt::{self, Tag};

fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
}

fn main() -> std::io::Result<()> {
    let world: PathBuf = std::env::args_os().nth(1).expect("usage: create-census <world>").into();
    let Some(rail) = Rail::open(&world)? else {
        println!("no data/create_tracks.dat");
        return Ok(());
    };
    let edges: usize = rail.graphs.iter().map(|g| g.edges.len()).sum();
    let turns: usize = rail.graphs.iter().map(|g| g.edges.iter().filter(|e| e.turn.is_some()).count()).sum();
    let strokes: usize = rail.graphs.iter().map(|g| g.strokes().len()).sum();
    let length: f64 = rail.graphs.iter().map(|g| g.length).sum();
    println!(
        "{} graphs, {} edges ({} turns) in {} strokes, {:.0} blocks; {} stations, {} signals, {} trains",
        rail.graphs.len(),
        edges,
        turns,
        strokes,
        length,
        rail.stations.len(),
        rail.signals.len(),
        rail.trains.len()
    );
    // Turns end where their nodes are.
    let mut worst_end: f64 = 0.0;
    for g in &rail.graphs {
        for e in &g.edges {
            if e.turn.is_some() {
                for (t, n) in [(0.0, e.a), (1.0, e.b)] {
                    let p = e.at(&g.nodes, t);
                    let q = g.nodes[n as usize].p;
                    worst_end = worst_end.max(((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt());
                }
            }
        }
    }
    println!("turn ends off their nodes by at most {worst_end:.3} blocks");
    // Carriages against their anchors.
    let root = nbt::read_gzip(std::fs::read(world.join("data/create_tracks.dat"))?.as_slice(), 512 << 20)?;
    let data = root.get("data").unwrap_or(&root);
    let (mut checked, mut worst, mut worst_name) = (0, 0.0f64, String::new());
    for t in data.get("Trains").map(Tag::items).unwrap_or_default() {
        let Some(id) = t.get("Id").and_then(tags::uuid) else { continue };
        let Some(train) = rail.trains.iter().find(|x| x.id == id) else { continue };
        let anchors: Vec<[f64; 3]> = t
            .get("Carriages")
            .map(Tag::items)
            .unwrap_or_default()
            .iter()
            .flat_map(|c| c.get("EntityPositioning").map(Tag::items).unwrap_or_default())
            .flat_map(|d| d.get("RotationAnchors").map(Tag::items).unwrap_or_default())
            .filter_map(tags::vec3)
            .collect();
        for a in anchors {
            let d = train
                .cars
                .iter()
                .map(|c| dist_to_segment([a[0], a[2]], [c[0], c[1]], [c[2], c[3]]))
                .fold(f64::INFINITY, f64::min);
            checked += 1;
            if d > worst {
                worst = d;
                worst_name = format!("{} ({})", train.name, uuid_text(&train.id));
            }
        }
    }
    println!("{checked} bogey anchors checked; farthest off its drawn carriage {worst:.3} blocks: {worst_name}");
    for t in rail.trains.iter().filter(|t| t.moving() || t.destination.is_some()).take(12) {
        println!(
            "  {}: {} cars, {:.0} blocks long, speed {:.2}, front {:?}, {} stops",
            t.name,
            t.cars.len(),
            t.length(),
            t.speed,
            t.front().map(|f| [f[0].round(), f[1].round()]),
            t.schedule.len()
        );
    }
    for s in rail.stations.iter().take(8) {
        println!("  station {:?} at {:.0}, {:.0}, heading {:.2}", s.name, s.place.at[0], s.place.at[2], s.place.heading);
    }
    Ok(())
}
