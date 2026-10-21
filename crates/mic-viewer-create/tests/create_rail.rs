// SPDX-License-Identifier: MPL-2.0
//! The readers against a real Create 6.0.10 save: `tests/data/create-rail`, cut out of a GameTest
//! world mic-rtsim's `RailFixtureGameTests` built (a straight, a quarter turn, an S-bend, two
//! stations, a signal, a two-carriage train, and a chain conveyor, frogport, packager, belt,
//! vault and depot beside it). Every number asserted here was read from the saved NBT.

use std::path::PathBuf;

use mic_viewer::api::Mark;
use mic_viewer::scene::SceneWriter;
use mic_viewer_create::blocks::{self, Detail, Kind, Thing};
use mic_viewer_create::draw;
use mic_viewer_create::rail::Rail;

fn world() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/create-rail")
}

fn near(p: [f64; 3], x: f64, z: f64, tol: f64) -> bool {
    (p[0] - x).abs() <= tol && (p[2] - z).abs() <= tol
}

fn length(line: &[[f64; 2]]) -> f64 {
    line.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum()
}

fn rail() -> Rail {
    Rail::open(&world()).expect("reads").expect("has create_tracks.dat")
}

#[test]
fn the_network_reads_with_its_curves() {
    let rail = rail();
    assert_eq!(rail.dims, ["minecraft:overworld"]);
    assert_eq!(rail.graphs.len(), 1);
    let g = &rail.graphs[0];
    assert_eq!(g.nodes.len(), 9);
    assert_eq!(g.edges.len(), 8, "16 directed connections, one edge each way");
    // Node positions are saved doubled: (1216, -120, 3209) is (608, -60, 1604.5).
    for (x, z) in [(608.0, 1604.5), (624.0, 1604.5), (625.0, 1604.5), (634.5, 1614.0), (638.5, 1657.0)] {
        assert!(g.nodes.iter().any(|n| near(n.p, x, z, 1e-9) && n.p[1] == -60.0), "a node at {x}, {z}");
    }
    let mut turns: Vec<_> = g.edges.iter().filter_map(|e| e.turn.as_ref()).collect();
    assert_eq!(turns.len(), 2);
    turns.sort_by(|a, b| a.length.total_cmp(&b.length));
    // The S-bend (13.72) and the quarter turn (14.92), as Create measures them.
    assert!((turns[0].length - 13.72).abs() < 0.02, "S-bend {}", turns[0].length);
    assert!((turns[1].length - 14.92).abs() < 0.02, "quarter turn {}", turns[1].length);
    // Each curve runs between its saved starts.
    for (t, a, b) in [(turns[1], (625.0, 1604.5), (634.5, 1614.0)), (turns[0], (634.5, 1635.0), (638.5, 1648.0))] {
        let (p0, p1) = (t.at(0.0), t.at(1.0));
        let ends = [
            near(p0, a.0, a.1, 1e-6) && near(p1, b.0, b.1, 1e-6),
            near(p0, b.0, b.1, 1e-6) && near(p1, a.0, a.1, 1e-6),
        ];
        assert!(ends.contains(&true), "{p0:?} {p1:?}");
    }
    // The quarter turn bulges outward of its chord, toward the corner (634.5, 1604.5).
    let mid = turns[1].at(0.5);
    assert!(mid[0] > 629.75 && mid[2] < 1609.25, "{mid:?}");
    let total: f64 = g.edges.iter().map(|e| e.len).sum();
    assert!((g.length - total).abs() < 1e-9);
}

#[test]
fn stations_signals_and_the_train_sit_on_the_track() {
    let rail = rail();
    assert_eq!(rail.stations.len(), 2);
    let central = rail.stations.iter().find(|s| s.name == "Fixture Central").expect("Fixture Central");
    let yard = rail.stations.iter().find(|s| s.name == "Fixture Yard").expect("Fixture Yard");
    assert!(near(central.place.at, 616.5, 1604.5, 1e-6), "{:?}", central.place.at);
    assert!(near(yard.place.at, 634.5, 1624.5, 1e-6), "{:?}", yard.place.at);
    assert_eq!(central.block, Some([616, -60, 1605]));
    assert_eq!(yard.block, Some([635, -60, 1624]));
    // An east-west station heads along x, the yard along z.
    assert!(central.place.heading.sin().abs() < 1e-6);
    assert!(yard.place.heading.cos().abs() < 1e-6);
    assert_eq!(rail.signals.len(), 1);
    let s = &rail.signals[0];
    assert!(near(s.place.at, 634.5, 1620.5, 1e-6), "{:?}", s.place.at);
    assert_eq!(s.types, ["ENTRY_SIGNAL", "ENTRY_SIGNAL"]);
    assert_eq!(s.state(), "RED");

    assert_eq!(rail.trains.len(), 1);
    let t = &rail.trains[0];
    assert_eq!(t.name, "Fixture Express");
    assert_eq!(t.cars.len(), 2);
    assert!(!t.moving() && !t.derailed);
    assert_eq!(t.at_station, Some(central.id));
    assert_eq!(t.state, "PRE_TRANSIT");
    assert!(t.paused && t.cyclic);
    assert_eq!(t.schedule.len(), 1);
    assert_eq!(t.schedule[0].describe(), "Go to Fixture Yard");
    assert_eq!(t.items.get("minecraft:coal"), Some(&32));
    // Both carriages stand on the straight at z 1604.5, between its nodes.
    for c in &t.cars {
        for (x, z) in [(c[0], c[1]), (c[2], c[3])] {
            assert!((z - 1604.5).abs() < 0.01 && (608.0..=625.0).contains(&x), "{c:?}");
        }
    }
}

#[test]
fn a_route_follows_the_track_round_the_curve() {
    let rail = rail();
    let by = |n: &str| rail.stations.iter().find(|s| s.name == n).unwrap().place.clone();
    let (central, yard) = (by("Fixture Central"), by("Fixture Yard"));
    let r = rail.route(&central, &yard).expect("a way");
    assert!((r[0][0] - 616.5).abs() < 1e-6 && (r[0][1] - 1604.5).abs() < 1e-6, "{:?}", r[0]);
    let end = r[r.len() - 1];
    assert!((end[0] - 634.5).abs() < 1e-6 && (end[1] - 1624.5).abs() < 1e-6, "{end:?}");
    // 8.5 along the straight, the 14.92 turn, 10.5 down to the yard.
    let l = length(&r);
    assert!((l - 33.92).abs() < 0.3, "{l}");
    // It bends: some point lies well off the chord, near the turn's middle.
    assert!(r.iter().any(|p| p[0] > 629.0 && p[1] < 1610.0));
    // The way back is the same length.
    let back = rail.route(&yard, &central).expect("the way back");
    assert!((length(&back) - l).abs() < 1e-6);
    // A station to itself is a point.
    assert!(length(&rail.route(&central, &central).unwrap()) < 1e-9);
}

#[test]
fn the_rail_draws_as_track_stations_signals_and_a_train() {
    let rail = rail();
    let mut w = SceneWriter::new();
    draw::rail_marks(&rail, 0, &mut w);
    let scene = w.finish(0, 0.0);
    let marks = |l: &str| scene.layers.get(l).map(|r| r.marks().to_vec()).unwrap_or_default();
    let track = marks(draw::TRACK);
    assert!(!track.is_empty());
    // Every edge is drawn: the strokes' length is the network's (turns sampled finely).
    let drawn: f64 = track
        .iter()
        .map(|m| match m {
            Mark::Path { p, .. } => length(p),
            _ => 0.0,
        })
        .sum();
    assert!((drawn - rail.graphs[0].length).abs() < 0.2, "{drawn} of {}", rail.graphs[0].length);
    let stations = marks(draw::STATIONS);
    assert_eq!(stations.len(), 2);
    assert!(stations.iter().all(|m| matches!(m, Mark::Point { l: Some(_), .. })));
    assert_eq!(marks(draw::SIGNALS).len(), 1);
    let trains = marks(draw::TRAINS);
    assert_eq!(trains.len(), 1);
    assert!(
        matches!(&trains[0], Mark::Train { cars, l: Some(l), .. } if cars.len() == 2 && l.contains("Fixture Express"))
    );
    let card = draw::rail_card(&rail, draw::TRAINS, &mic_viewer_create::tags::uuid_text(&rail.trains[0].id))
        .expect("a train card");
    let text = serde_json::to_string(&card).unwrap();
    assert!(text.contains("Fixture Yard") && text.contains("Coal"), "{text}");
}

fn things() -> Vec<Thing> {
    let chunks = [(38, 100), (38, 101), (39, 100), (39, 101), (39, 102), (39, 103)];
    let dir = blocks::region_dir(&world(), "minecraft:overworld");
    let (read, bad) = blocks::read_region(&dir, 1, 3, &chunks).expect("the region reads");
    assert_eq!(bad, 0);
    read.into_iter().flat_map(|(_, t)| t).collect()
}

#[test]
fn the_factory_block_entities_read() {
    let t = things();
    let at = |p: [i32; 3]| t.iter().find(|x| x.pos == p).unwrap_or_else(|| panic!("nothing at {p:?}"));
    let a = at([610, -58, 1610]);
    assert_eq!(a.kind, Kind::Chain);
    assert!(matches!(&a.detail, Detail::Chain { links, .. } if links == &[[610, -58, 1618]]));
    assert!((a.speed - 32.0).abs() < 1e-6);
    let f = at([612, -58, 1618]);
    assert_eq!(f.kind, Kind::Port);
    assert_eq!(f.detail, Detail::Frogport { address: "Fixture Depot".into(), chain: Some([610, -58, 1618]) });
    assert_eq!(at([614, -60, 1610]).detail, Detail::Address("Fixture Mill".into()));
    let b = at([614, -59, 1614]);
    assert_eq!(b.kind, Kind::Belt);
    assert!(matches!(b.detail, Detail::Belt { index: 0, length: 5, .. }));
    let segs = t.iter().filter(|x| matches!(x.detail, Detail::Belt { controller: [614, -59, 1614], .. })).count();
    assert_eq!(segs, 5, "the controller and four segments");
    let v = at([620, -60, 1610]);
    assert_eq!(v.kind, Kind::Storage);
    match &v.detail {
        Detail::Store { controller: true, items, .. } => {
            assert_eq!(items.get("minecraft:iron_ingot"), Some(&64));
            assert_eq!(items.get("minecraft:copper_ingot"), Some(&32));
        }
        d => panic!("vault: {d:?}"),
    }
    // Drawn: the chain once, the belt as one run, the ports and the vault.
    let mut w = SceneWriter::new();
    draw::block_marks(&t, "minecraft:overworld", &mut w);
    let scene = w.finish(0, 0.0);
    let n = |l: &str| scene.layers.get(l).map_or(0, |r| r.len());
    assert_eq!(n(draw::CHAINS), 1);
    assert_eq!(n(draw::BELTS), 1);
    assert!(n(draw::PORTS) >= 2);
    assert_eq!(n(draw::STORAGE), 1);
    let sections = draw::census_sections(&t);
    let text = serde_json::to_string(&sections).unwrap();
    assert!(text.contains("Fixture Mill") || text.contains("Fixture Depot"), "{text}");
}
