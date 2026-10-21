// SPDX-License-Identifier: MPL-2.0
//! A synthetic world served through the viewer: villages, roads, caravans on the move, herds,
//! dragons, factions and feuds, districts and train lines, a heat field, an event history, and a
//! terrain base map. Nothing here is a simulation; it is the page's fixture (development,
//! screenshots, the browser smoke test) and the budget tests' load.
//!
//! Deterministic for a seed: with `Clock::Still` the clock stands still, so screenshots repeat; with
//! `Clock::Stepped` every publish moves it a fixed number of ticks, so the world after n publishes is
//! the same on every run (the smoke test's live updates).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use mic_viewer::api::{
    Card, EventKind, EventRow, EventsAnswer, Frame, Freshness, Group, Histogram, LayerDesc, Layers, LegRow, Mark,
    ProjectionDesc, PyramidDesc, Section, Series, TICKS_PER_DAY, Tick, TileLayerDesc, TimeInfo, WorldInfo,
};
use mic_viewer::scene::{Scene, SceneWriter};
use mic_viewer::tiles::{Rendered, TILE, TileCache, TileError, TileSource, Tiles};
use mic_viewer::viewer::{Backend, Changes, EventQuery};

pub const OVERWORLD: &str = "minecraft:overworld";
pub const RING: &str = "demo:ring";
/// The overworld's half size, blocks.
pub const HALF: f64 = 6144.0;
/// The ring frame's wrap period and half height.
pub const RING_PERIOD: f64 = 4096.0;
pub const RING_HALF_Z: f64 = 1024.0;
/// Game time the demo starts at: day 40, 06:00.
pub const START: Tick = 40 * TICKS_PER_DAY;

/// splitmix64.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// [0, 1).
    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

fn hash2(seed: u64, x: i64, z: i64) -> f64 {
    let mut r = Rng::new(
        seed ^ (x as u64).wrapping_mul(0x632b_e59b_d9b4_e019) ^ (z as u64).wrapping_mul(0x8cb9_2ba7_2f3d_8dd7),
    );
    r.f()
}

/// Smooth value noise, a few octaves: about -1 to 1.
pub fn terrain(seed: u64, x: f64, z: f64) -> f64 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut wl = 2048.0;
    let mut norm = 0.0;
    for o in 0..6 {
        let (fx, fz) = (x / wl, z / wl);
        let (ix, iz) = (fx.floor(), fz.floor());
        let (tx, tz) = (fx - ix, fz - iz);
        let (sx, sz) = (tx * tx * (3.0 - 2.0 * tx), tz * tz * (3.0 - 2.0 * tz));
        let s = seed.wrapping_add(o * 7919);
        let (ix, iz) = (ix as i64, iz as i64);
        let a = hash2(s, ix, iz);
        let b = hash2(s, ix + 1, iz);
        let c = hash2(s, ix, iz + 1);
        let d = hash2(s, ix + 1, iz + 1);
        let v = a + (b - a) * sx + (c - a) * sz + (a - b - c + d) * sx * sz;
        sum += (v * 2.0 - 1.0) * amp;
        norm += amp;
        amp *= 0.5;
        wl *= 0.5;
    }
    // A continent: high in the middle, sea at the edges.
    let r = (x * x + z * z).sqrt() / HALF;
    sum / norm * 0.9 + 0.35 - r * r * 0.8
}

const SYL: &[&str] = &[
    "ash", "ford", "oak", "vale", "mor", "wick", "brook", "dun", "elm", "hal", "ston", "ley", "bar", "row", "kel",
    "mere", "thorn", "by", "wyn", "gar", "tor", "ridge", "fen", "holt",
];

fn name(r: &mut Rng) -> String {
    let n = 2 + r.below(2);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(SYL[r.below(SYL.len())]);
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

#[derive(Clone)]
pub struct Village {
    pub id: usize,
    pub name: String,
    pub x: f64,
    pub z: f64,
    pub kind: u16,
    pub pop: u32,
    pub faction: usize,
    pub prices: Vec<f64>,
}

#[derive(Clone)]
pub struct Road {
    pub a: usize,
    pub b: usize,
    pub flow: f64,
    pub hazard: u16,
}

#[derive(Clone)]
pub struct Herd {
    pub id: usize,
    pub species: &'static str,
    pub route: Vec<[f64; 2]>,
    pub count: u32,
    pub period: Tick,
}

#[derive(Clone)]
pub struct Faction {
    pub id: usize,
    pub name: String,
    pub x: f64,
    pub z: f64,
    pub r: f64,
    pub culture: u16,
    pub warriors: u32,
}

#[derive(Clone)]
pub struct District {
    pub id: usize,
    pub name: String,
    pub b: [f64; 4],
    pub state: u16,
    pub rate: f64,
}

/// The world's fixed parts.
pub struct World {
    pub seed: u64,
    pub villages: Vec<Village>,
    pub roads: Vec<Road>,
    pub herds: Vec<Herd>,
    pub factions: Vec<Faction>,
    pub feuds: Vec<(usize, usize, u16)>,
    pub districts: Vec<District>,
    pub lines: Vec<(usize, usize, u32)>,
    pub lairs: Vec<[f64; 2]>,
    pub events: Vec<EventRow>,
}

pub const KINDS: &[(&str, &str, &str, &str, u8)] = &[
    ("trade.deal", "Trade", "trade", "#3fb8af", 0),
    ("trade.shortage", "Shortage", "trade", "#e9a23b", 1),
    ("conflict.raid", "Raid", "conflict", "#e5484d", 2),
    ("conflict.feud", "Feud step", "conflict", "#d6409f", 1),
    ("ecology.hunt", "Dragon hunt", "ecology", "#8e4ec6", 1),
    ("ecology.birth", "Herd births", "ecology", "#46a758", 0),
    ("industry.stop", "District stopped", "industry", "#f76b15", 1),
    ("settle.grow", "Village grew", "settlements", "#e0b04a", 0),
];

/// Sizes of the demo world.
#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub villages: usize,
    pub districts: usize,
    pub caravans: usize,
    pub days_of_events: i64,
}

impl Default for Size {
    fn default() -> Self {
        Size { villages: 320, districts: 240, caravans: 600, days_of_events: 40 }
    }
}

impl World {
    pub fn new(seed: u64, size: Size) -> World {
        let mut r = Rng::new(seed);
        // Villages on land.
        let mut villages = Vec::new();
        let factions_n = 9;
        let mut factions = Vec::new();
        for id in 0..factions_n {
            let (x, z) = loop {
                let (x, z) = (r.range(-HALF * 0.7, HALF * 0.7), r.range(-HALF * 0.7, HALF * 0.7));
                if terrain(seed, x, z) > 0.05 {
                    break (x, z);
                }
            };
            factions.push(Faction {
                id,
                name: format!("{} {}", name(&mut r), ["Clan", "Horde", "League", "Kin"][id % 4]),
                x,
                z,
                r: r.range(700.0, 1600.0),
                culture: (id % 3) as u16,
                warriors: 20 + r.below(400) as u32,
            });
        }
        let mut tries = 0;
        while villages.len() < size.villages && tries < size.villages * 50 {
            tries += 1;
            let (x, z) = (r.range(-HALF, HALF), r.range(-HALF, HALF));
            let h = terrain(seed, x, z);
            if !(0.02..0.55).contains(&h) {
                continue;
            }
            let id = villages.len();
            let faction = factions
                .iter()
                .min_by(|a, b| ((a.x - x).hypot(a.z - z)).total_cmp(&(b.x - x).hypot(b.z - z)))
                .map_or(0, |f| f.id);
            villages.push(Village {
                id,
                name: name(&mut r),
                x: x.round(),
                z: z.round(),
                kind: [0, 0, 0, 0, 1, 1, 2, 3][r.below(8)],
                pop: (8.0 + r.f().powi(3) * 400.0) as u32,
                faction,
                prices: (0..4).map(|_| r.range(0.6, 1.5)).collect(),
            });
        }
        // Roads to the two nearest neighbours.
        let mut roads = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if villages.len() <= 5000 {
            for v in &villages {
                let mut near: Vec<(f64, usize)> =
                    villages.iter().filter(|w| w.id != v.id).map(|w| ((w.x - v.x).hypot(w.z - v.z), w.id)).collect();
                near.sort_by(|a, b| a.0.total_cmp(&b.0));
                for &(d, w) in near.iter().take(2) {
                    let key = (v.id.min(w), v.id.max(w));
                    if d < 1800.0 && seen.insert(key) {
                        roads.push(Road {
                            a: key.0,
                            b: key.1,
                            flow: r.range(0.5, 12.0),
                            hazard: [0, 0, 0, 1, 1, 2][r.below(6)],
                        });
                    }
                }
            }
        }
        // Herds wander loops on land.
        let species = ["Bison", "Deer", "Mammoth", "Elk", "Boar"];
        let mut herds = Vec::new();
        for id in 0..60 {
            let (cx, cz) = (r.range(-HALF * 0.8, HALF * 0.8), r.range(-HALF * 0.8, HALF * 0.8));
            if terrain(seed, cx, cz) < 0.05 {
                continue;
            }
            let rad = r.range(300.0, 900.0);
            let n = 6;
            let route = (0..=n)
                .map(|k| {
                    let a = std::f64::consts::TAU * f64::from(k) / f64::from(n);
                    let j = if k == n { 0.0 } else { r.range(-0.2, 0.2) };
                    [cx + rad * (a + j).cos(), cz + rad * 0.7 * (a + j).sin()]
                })
                .collect::<Vec<_>>();
            let mut route = route;
            let last = route.len() - 1;
            route[last] = route[0];
            herds.push(Herd {
                id,
                species: species[r.below(species.len())],
                route,
                count: (50.0 + r.f().powi(2) * 20_000.0) as u32,
                period: TICKS_PER_DAY * (3 + r.below(8) as i64),
            });
        }
        let mut feuds = Vec::new();
        for _ in 0..12 {
            let (a, b) = (r.below(factions_n), r.below(factions_n));
            if a != b {
                feuds.push((a.min(b), a.max(b), r.below(5) as u16));
            }
        }
        feuds.sort_unstable();
        feuds.dedup_by_key(|f| (f.0, f.1));
        let mut districts = Vec::new();
        for id in 0..size.districts {
            let (x, z) = if villages.is_empty() || size.districts > 10_000 {
                (r.range(-HALF, HALF), r.range(-HALF, HALF))
            } else {
                let v = &villages[r.below(villages.len())];
                (v.x + r.range(-300.0, 300.0), v.z + r.range(-300.0, 300.0))
            };
            let (w, h) = (16.0 * (2 + r.below(6)) as f64, 16.0 * (2 + r.below(6)) as f64);
            let x0 = (x / 16.0).floor() * 16.0;
            let z0 = (z / 16.0).floor() * 16.0;
            districts.push(District {
                id,
                name: format!("{} {}", name(&mut r), ["Works", "Smelter", "Mill", "Yard", "Foundry"][id % 5]),
                b: [x0, z0, x0 + w, z0 + h],
                state: [0, 0, 0, 0, 1, 2, 3][r.below(7)],
                rate: r.range(10.0, 900.0),
            });
        }
        let mut lines = Vec::new();
        if districts.len() > 1 && districts.len() <= 5000 {
            for _ in 0..(districts.len() / 6).max(1) {
                let (a, b) = (r.below(districts.len()), r.below(districts.len()));
                if a != b {
                    lines.push((a, b, 1 + r.below(30) as u32));
                }
            }
        }
        let lairs = (0..14)
            .map(|_| {
                loop {
                    let (x, z) = (r.range(-HALF * 0.9, HALF * 0.9), r.range(-HALF * 0.9, HALF * 0.9));
                    if terrain(seed, x, z) > 0.45 {
                        break [x, z];
                    }
                }
            })
            .collect();
        let mut w =
            World { seed, villages, roads, herds, factions, feuds, districts, lines, lairs, events: Vec::new() };
        w.events = w.history(&mut r, START - size.days_of_events * TICKS_PER_DAY, START);
        w
    }

    /// Synthetic events in `[from, to)`, about thirty a day.
    pub fn history(&self, r: &mut Rng, from: Tick, to: Tick) -> Vec<EventRow> {
        let mut out = Vec::new();
        if self.villages.is_empty() {
            return out;
        }
        let mut t = from;
        while t < to {
            // Busier on some days (a war in the middle of the history).
            let day = (t - START).div_euclid(TICKS_PER_DAY);
            let war = (-25..=-18).contains(&day);
            t += (r.range(200.0, if war { 900.0 } else { 1600.0 })) as Tick;
            if t >= to {
                break;
            }
            let k = if war && r.f() < 0.5 { 2 } else { r.below(KINDS.len()) };
            let v = &self.villages[r.below(self.villages.len())];
            let (kind, label, _, _, sev) = KINDS[k];
            let text = match kind {
                "trade.deal" => format!("{} sells grain to a caravan", v.name),
                "trade.shortage" => format!("{} runs short of tools", v.name),
                "conflict.raid" => format!("{} raid {}", self.factions[r.below(self.factions.len())].name, v.name),
                "conflict.feud" => format!("A feud near {} hardens", v.name),
                "ecology.hunt" => format!("A dragon hunts near {}", v.name),
                "ecology.birth" => format!("Calves born near {}", v.name),
                "industry.stop" => format!("A district near {} stops: no coal", v.name),
                _ => format!("{} grows", v.name),
            };
            let _ = label;
            out.push(EventRow {
                t,
                kind: kind.into(),
                sev,
                x: v.x + r.range(-80.0, 80.0),
                z: v.z + r.range(-80.0, 80.0),
                subject: Some(v.id.to_string()),
                object: None,
                mag: 1 + r.below(40) as i64,
                text,
                frame: None,
            });
        }
        out
    }

    /// Synthetic events on the ring frame in `[from, to)`, about ten a day: stream rows of a frame
    /// other than the first.
    pub fn ring_history(&self, r: &mut Rng, from: Tick, to: Tick) -> Vec<EventRow> {
        let mut out = Vec::new();
        let mut t = from;
        loop {
            t += r.range(600.0, 4200.0) as Tick;
            if t >= to {
                return out;
            }
            let (kind, _, _, _, sev) = KINDS[r.below(KINDS.len())];
            out.push(EventRow {
                t,
                kind: kind.into(),
                sev,
                x: r.range(-RING_PERIOD / 2.0, RING_PERIOD / 2.0),
                z: r.range(-RING_HALF_Z * 0.8, RING_HALF_Z * 0.8),
                subject: None,
                object: None,
                mag: 1,
                text: "On the ring, near the seam road".into(),
                frame: Some(RING.into()),
            });
        }
    }

    /// A caravan's road and trip at time `t`: (road index, forward, t0, t1).
    fn caravan_trip(&self, c: usize, t: Tick) -> Option<(usize, bool, Tick, Tick)> {
        if self.roads.is_empty() {
            return None;
        }
        let road = (c * 7919) % self.roads.len();
        let rd = &self.roads[road];
        let (a, b) = (&self.villages[rd.a], &self.villages[rd.b]);
        let len = (a.x - b.x).hypot(a.z - b.z);
        // 4 blocks a second walking: 5 ticks a block; a rest of a quarter day at each end.
        let travel = (len * 5.0) as Tick + 1;
        let cycle = 2 * (travel + TICKS_PER_DAY / 4);
        let phase = ((c as i64) * 104_729).rem_euclid(cycle);
        let k = (t + phase).div_euclid(cycle);
        let start = k * cycle - phase;
        let half = cycle / 2;
        let into = t - start;
        let forward = into < half;
        let t0 = start + if forward { 0 } else { half };
        Some((road, forward, t0, t0 + travel))
    }

    /// The scene of a frame at `t`.
    pub fn scene(&self, frame: &str, t: Tick, caravans: usize) -> Scene {
        let mut w = SceneWriter::new();
        for l in LAYER_IDS {
            w.layer(l);
        }
        if frame == RING {
            return self.ring_scene(t);
        }
        for v in &self.villages {
            let s = (f64::from(v.pop) / 60.0).sqrt().clamp(0.6, 3.0);
            w.mark(
                "settle.villages",
                Mark::point(v.id, v.x, v.z).with_ci(v.kind).with_size(s).with_label(v.name.clone()),
            );
            w.mark("settle.labels", Mark::Label { x: v.x, z: v.z, text: v.name.clone() });
            let p = v.prices[0];
            let ci = if p < 0.85 {
                0
            } else if p < 1.15 {
                1
            } else {
                2
            };
            w.mark("trade.prices", Mark::point(v.id, v.x, v.z).with_ci(ci).with_size(1.2));
        }
        for (i, rd) in self.roads.iter().enumerate() {
            let (a, b) = (&self.villages[rd.a], &self.villages[rd.b]);
            let mut m = Mark::edge([a.x, a.z], [b.x, b.z], 1.0 + rd.flow / 3.0).with_ci(rd.hazard);
            if let Mark::Edge { id, .. } = &mut m {
                *id = Some(i.to_string());
            }
            w.mark("travel.roads", m);
        }
        for c in 0..caravans {
            if let Some((road, fwd, t0, t1)) = self.caravan_trip(c, t) {
                let rd = &self.roads[road];
                let (a, b) = (&self.villages[rd.a], &self.villages[rd.b]);
                let (p, q) = if fwd { ([a.x, a.z], [b.x, b.z]) } else { ([b.x, b.z], [a.x, a.z]) };
                w.mark("travel.caravans", Mark::leg(c, p, q, t0, t1).with_ci((c % 3) as u16));
            }
        }
        for h in &self.herds {
            let at = self.herd_at(h, t);
            let r = (f64::from(h.count)).sqrt() * 2.5 + 60.0;
            w.mark("ecology.herds", Mark::disc(Some(h.id.to_string()), at[0], at[1], r).with_ci(species_ci(h.species)));
            w.mark(
                "ecology.routes",
                Mark::Path {
                    id: Some(h.id.to_string()),
                    p: h.route.clone(),
                    ci: Some(species_ci(h.species)),
                    c: None,
                    w: Some(1.5),
                },
            );
        }
        for (i, lair) in self.lairs.iter().enumerate() {
            w.mark("ecology.lairs", Mark::point(i, lair[0], lair[1]).with_glyph("triangle"));
            // A dragon flies out to a village and back over two days.
            let cycle = 2 * TICKS_PER_DAY;
            let phase = (i as i64 * 31_337).rem_euclid(cycle);
            let start = (t + phase).div_euclid(cycle) * cycle - phase;
            if !self.villages.is_empty() {
                let v = &self.villages[(i * 37) % self.villages.len()];
                let half = cycle / 2;
                let (p, q, t0) =
                    if t - start < half { (*lair, [v.x, v.z], start) } else { ([v.x, v.z], *lair, start + half) };
                w.mark("ecology.dragons", Mark::leg(i, p, q, t0, t0 + half / 2).with_ci((i % 3) as u16));
            }
        }
        for f in &self.factions {
            w.mark(
                "conflict.factions",
                Mark::disc(Some(f.id.to_string()), f.x, f.z, f.r).with_ci(f.culture).with_label(f.name.clone()),
            );
        }
        for &(a, b, step) in &self.feuds {
            let (fa, fb) = (&self.factions[a], &self.factions[b]);
            let mut m = Mark::edge([fa.x, fa.z], [fb.x, fb.z], 2.0 + f64::from(step)).with_ci(step);
            if let Mark::Edge { id, .. } = &mut m {
                *id = Some(format!("{a}-{b}"));
            }
            w.mark("conflict.feuds", m);
        }
        for d in &self.districts {
            w.mark("industry.districts", Mark::rect(Some(d.id.to_string()), d.b).with_ci(d.state));
        }
        for (i, &(a, b, trips)) in self.lines.iter().enumerate() {
            let (da, db) = (&self.districts[a].b, &self.districts[b].b);
            let mut m = Mark::edge(
                [(da[0] + da[2]) / 2.0, (da[1] + da[3]) / 2.0],
                [(db[0] + db[2]) / 2.0, (db[1] + db[3]) / 2.0],
                1.0 + f64::from(trips) / 10.0,
            );
            if let Mark::Edge { id, .. } = &mut m {
                *id = Some(i.to_string());
            }
            w.mark("industry.lines", m);
        }
        // A heat field over the continent, 512-block cells.
        let n = (2.0 * HALF / 512.0) as u32;
        let v = (0..n * n)
            .map(|i| {
                let (cx, cz) = (-HALF + 256.0 + f64::from(i % n) * 512.0, -HALF + 256.0 + f64::from(i / n) * 512.0);
                let h = terrain(self.seed, cx, cz);
                (h > 0.0).then(|| (30.0 - cz / HALF * 18.0 - h * 25.0).round())
            })
            .collect();
        w.mark(
            "space.heat",
            Mark::Cell {
                x0: -HALF,
                z0: -HALF,
                size: 512.0,
                w: n,
                h: n,
                v,
                ramp: Some("heat".into()),
                lo: Some(-10.0),
                hi: Some(45.0),
            },
        );
        w.finish(t, 0.0)
    }

    fn ring_scene(&self, t: Tick) -> Scene {
        let mut w = SceneWriter::new();
        for l in LAYER_IDS {
            w.layer(l);
        }
        let mut r = Rng::new(self.seed ^ 77);
        let mut pts = Vec::new();
        for i in 0..40 {
            let (x, z) =
                (r.range(-RING_PERIOD / 2.0, RING_PERIOD / 2.0), r.range(-RING_HALF_Z * 0.9, RING_HALF_Z * 0.9));
            pts.push([x, z]);
            w.mark(
                "settle.villages",
                Mark::point(format!("r{i}"), x.round(), z.round()).with_ci((i % 2) as u16).with_label(name(&mut r)),
            );
        }
        // A caravan that crosses the seam: the source cuts its leg in two pieces.
        let half = RING_PERIOD / 2.0;
        let leg = Mark::Leg {
            id: "seam".into(),
            p: vec![[half - 300.0, 0.0, half, 100.0], [-half, 100.0, -half + 300.0, 200.0]],
            t0: t - 1000,
            t1: t + 4000,
            ci: Some(1),
            c: None,
            g: None,
            l: None,
        };
        w.mark("travel.caravans", leg);
        w.mark("travel.roads", Mark::edge(pts[0], pts[1], 2.0));
        w.finish(t, RING_PERIOD)
    }

    pub fn herd_at(&self, h: &Herd, t: Tick) -> [f64; 2] {
        let f = t.rem_euclid(h.period) as f64 / h.period as f64;
        let segs = h.route.len() - 1;
        let pos = f * segs as f64;
        let i = (pos.floor() as usize).min(segs - 1);
        let k = pos - i as f64;
        let (a, b) = (h.route[i], h.route[i + 1]);
        [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k]
    }

    pub fn card(&self, layer: &str, id: &str, t: Tick) -> Option<Card> {
        let n: usize = id.parse().ok().unwrap_or(usize::MAX);
        Some(match layer {
            "settle.villages" | "trade.prices" => {
                let v = self.villages.get(n)?;
                let f = &self.factions[v.faction];
                let goods = ["grain", "tools", "cloth", "iron"];
                let mut prices = Section::new("Prices against their anchor");
                for (g, p) in goods.iter().zip(&v.prices) {
                    prices = prices.field_unit(g, (p * 100.0).round() / 100.0, "x anchor");
                }
                let series = Series {
                    label: "Grain price".into(),
                    unit: "x anchor".into(),
                    points: (0..30)
                        .map(|d| {
                            let tt = t - (30 - d) * TICKS_PER_DAY;
                            [tt as f64, v.prices[0] * (1.0 + 0.15 * ((d as f64) * 0.7 + v.id as f64).sin())]
                        })
                        .collect(),
                };
                let neighbours: Vec<String> = self
                    .roads
                    .iter()
                    .filter_map(|r| {
                        if r.a == v.id {
                            Some(r.b)
                        } else if r.b == v.id {
                            Some(r.a)
                        } else {
                            None
                        }
                    })
                    .map(|o| self.villages[o].name.clone())
                    .collect();
                Card::new(layer, v.id, v.name.clone())
                    .subtitle(["Human village", "Ribbit village", "Player base", "Haven"][v.kind as usize])
                    .at(v.x, v.z)
                    .field("Population", v.pop)
                    .link("Faction", f.name.clone(), "conflict.factions", f.id)
                    .field("Neighbours", if neighbours.is_empty() { "none".to_owned() } else { neighbours.join(", ") })
                    .field_unit("Treasury", (f64::from(v.pop) * 3.7).round(), "coins")
                    .section(prices)
                    .series(series)
            }
            "travel.caravans" => {
                let (road, fwd, t0, t1) = self.caravan_trip(n, t)?;
                let rd = &self.roads[road];
                let (a, b) = if fwd { (rd.a, rd.b) } else { (rd.b, rd.a) };
                let (va, vb) = (&self.villages[a], &self.villages[b]);
                let progress = ((t - t0) as f64 / (t1 - t0).max(1) as f64).clamp(0.0, 1.0);
                let at = mic_viewer::api::leg_at(&[[va.x, va.z, vb.x, vb.z]], t0, t1, t);
                Card::new(layer, n, format!("Caravan {n}"))
                    .subtitle(format!("{} to {}", va.name, vb.name))
                    .at(at[0], at[1])
                    .link("Home", va.name.clone(), "settle.villages", a)
                    .link("Bound for", vb.name.clone(), "settle.villages", b)
                    .field("Stage", if progress >= 1.0 { "resting" } else { "on the road" })
                    .field_unit("Progress", (progress * 100.0).round(), "%")
                    .field("Members", 3 + n % 9)
                    .section(Section::new("Cargo").field_unit("grain", 64 + (n % 5) * 16, "items").field_unit(
                        "cloth",
                        12 + n % 20,
                        "items",
                    ))
            }
            "travel.roads" => {
                let rd = self.roads.get(n)?;
                let (a, b) = (&self.villages[rd.a], &self.villages[rd.b]);
                Card::new(layer, n, format!("Road {} to {}", a.name, b.name))
                    .at((a.x + b.x) / 2.0, (a.z + b.z) / 2.0)
                    .field_unit("Goods in transit", (rd.flow * 10.0).round(), "items")
                    .field("Raid hazard", ["low", "middling", "high"][rd.hazard as usize])
                    .field_unit("Length", a.x.hypot(a.z).round(), "blocks")
            }
            "ecology.herds" | "ecology.routes" => {
                let h = self.herds.iter().find(|h| h.id == n)?;
                let at = self.herd_at(h, t);
                Card::new(layer, n, format!("{} herd {n}", h.species))
                    .at(at[0], at[1])
                    .field("Head", h.count)
                    .field_unit("Loop", h.period / TICKS_PER_DAY, "days")
                    .section(
                        Section::new("Ledger this year")
                            .field("Born", h.count / 9)
                            .field("Died", h.count / 14)
                            .field("Eaten", h.count / 40),
                    )
            }
            "ecology.lairs" | "ecology.dragons" => {
                let l = self.lairs.get(n)?;
                Card::new(layer, n, format!("Dragon of lair {n}"))
                    .subtitle(["Fire", "Ice", "Lightning"][n % 3])
                    .at(l[0], l[1])
                    .field_unit("Fullness", 40 + (n * 13) % 60, "%")
                    .field("Hoard", 200 + n * 57)
            }
            "conflict.factions" => {
                let f = self.factions.get(n)?;
                Card::new(layer, n, f.name.clone())
                    .at(f.x, f.z)
                    .field("Culture", ["Goblin", "Orc", "Bandit"][f.culture as usize])
                    .field("Warriors now", f.warriors)
                    .field_unit("Reach", f.r.round(), "blocks")
            }
            "conflict.feuds" => {
                let (a, b) = id.split_once('-')?;
                let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
                let (fa, fb) = (self.factions.get(a)?, self.factions.get(b)?);
                let step = self.feuds.iter().find(|f| f.0 == a && f.1 == b)?.2;
                Card::new(layer, id, format!("{} against {}", fa.name, fb.name))
                    .field("Step", ["Notice", "Insult", "Raid", "War", "Siege"][step as usize])
                    .link("Side", fa.name.clone(), "conflict.factions", a)
                    .link("Side", fb.name.clone(), "conflict.factions", b)
            }
            "industry.districts" => {
                let d = self.districts.get(n)?;
                Card::new(layer, n, d.name.clone())
                    .subtitle(["Replaying", "Learning", "Stopped", "Starved"][d.state as usize])
                    .at((d.b[0] + d.b[2]) / 2.0, (d.b[1] + d.b[3]) / 2.0)
                    .field_unit("Output", d.rate.round(), "items/h")
                    .field_unit("Size", ((d.b[2] - d.b[0]) / 16.0 * (d.b[3] - d.b[1]) / 16.0) as u32, "chunks")
                    .note("as of the last save")
            }
            _ => return None,
        })
    }
}

fn species_ci(s: &str) -> u16 {
    match s {
        "Bison" => 0,
        "Deer" => 1,
        "Mammoth" => 2,
        "Elk" => 3,
        _ => 4,
    }
}

pub const LAYER_IDS: &[&str] = &[
    "settle.villages",
    "settle.labels",
    "trade.prices",
    "travel.roads",
    "travel.caravans",
    "ecology.herds",
    "ecology.routes",
    "ecology.lairs",
    "ecology.dragons",
    "conflict.factions",
    "conflict.feuds",
    "industry.districts",
    "industry.lines",
    "space.heat",
];

pub fn layers() -> Layers {
    Layers {
        groups: [
            ("settlements", "Settlements"),
            ("trade", "Trade"),
            ("travel", "Travel"),
            ("ecology", "Ecology"),
            ("conflict", "Conflict"),
            ("industry", "Industry"),
            ("space", "World model"),
        ]
        .iter()
        .map(|(id, label)| Group { id: (*id).into(), label: (*label).into() })
        .collect(),
        layers: vec![
            LayerDesc::new("settle.villages", "settlements", "Villages", "point", "#e0b04a")
                .describe("Villages and bases, sized by population")
                .priority(10)
                .legend(&[
                    ("Human", "#e0b04a"),
                    ("Ribbit", "#7ccf6b"),
                    ("Player base", "#5eb1ef"),
                    ("Haven", "#c9a3f5"),
                ]),
            LayerDesc::new("settle.labels", "settlements", "Names", "label", "#e8e2d0")
                .describe("Village names up close")
                .priority(1)
                .zoom(0.0, 6.0)
                .no_cards(),
            LayerDesc::new("trade.prices", "trade", "Grain price", "point", "#3fb8af")
                .describe("Each village's grain price against its anchor")
                .priority(4)
                .off()
                .legend(&[("Cheap", "#46a758"), ("Near anchor", "#e9e3cf"), ("Dear", "#e5484d")]),
            LayerDesc::new("travel.roads", "travel", "Roads", "edge", "#a08d6a")
                .describe("Roads between villages, width by goods in transit, colour by raid hazard")
                .priority(5)
                .legend(&[("Safe", "#a08d6a"), ("Some raids", "#e9a23b"), ("Dangerous", "#e5484d")]),
            LayerDesc::new("travel.caravans", "travel", "Caravans", "leg", "#f5d90a")
                .describe("Caravans on their legs, placed by game time")
                .priority(8)
                .glyph("diamond", 5.0)
                .zoom(0.0, 48.0)
                .legend(&[("Grain", "#f5d90a"), ("Tools", "#ffb224"), ("Cloth", "#ff8b3e")]),
            LayerDesc::new("ecology.herds", "ecology", "Herds", "area", "#46a758")
                .describe("Herd footprints, sized by head count")
                .priority(3)
                .legend(&[
                    ("Bison", "#8d6e4f"),
                    ("Deer", "#b08a5a"),
                    ("Mammoth", "#9a8478"),
                    ("Elk", "#7f9a5a"),
                    ("Boar", "#6b5b4f"),
                ]),
            LayerDesc::new("ecology.routes", "ecology", "Herd routes", "path", "#46a758")
                .describe("The loops herds walk")
                .priority(2)
                .off()
                .legend(&[
                    ("Bison", "#8d6e4f"),
                    ("Deer", "#b08a5a"),
                    ("Mammoth", "#9a8478"),
                    ("Elk", "#7f9a5a"),
                    ("Boar", "#6b5b4f"),
                ]),
            LayerDesc::new("ecology.lairs", "ecology", "Dragon lairs", "point", "#8e4ec6")
                .describe("Lairs and their hoards")
                .priority(7)
                .glyph("triangle", 7.0),
            LayerDesc::new("ecology.dragons", "ecology", "Dragons", "leg", "#d19dff")
                .describe("Dragons in flight")
                .priority(9)
                .glyph("star", 7.0)
                .legend(&[("Fire", "#ff6b4a"), ("Ice", "#7cd4fd"), ("Lightning", "#d19dff")]),
            LayerDesc::new("conflict.factions", "conflict", "Factions", "area", "#e5484d")
                .describe("Faction reach")
                .priority(1)
                .legend(&[("Goblin", "#6fae3b"), ("Orc", "#c4542a"), ("Bandit", "#8a7d6b")]),
            LayerDesc::new("conflict.feuds", "conflict", "Feuds", "edge", "#d6409f")
                .describe("Feuds between factions, coloured by step")
                .priority(6)
                .legend(&[
                    ("Notice", "#a18072"),
                    ("Insult", "#e9a23b"),
                    ("Raid", "#f76b15"),
                    ("War", "#e5484d"),
                    ("Siege", "#b4235a"),
                ]),
            LayerDesc::new("industry.districts", "industry", "Districts", "area", "#f76b15")
                .describe("Industrial districts, the box of their chunks")
                .priority(6)
                .legend(&[
                    ("Replaying", "#f76b15"),
                    ("Learning", "#5eb1ef"),
                    ("Stopped", "#8b8d98"),
                    ("Starved", "#e5484d"),
                ]),
            LayerDesc::new("industry.lines", "industry", "Train lines", "edge", "#5eb1ef")
                .describe("Train lines, width by trips a day")
                .priority(4)
                .off(),
            LayerDesc::new("space.heat", "space", "Temperature", "cell", "#ff6b4a")
                .describe("Year-round temperature, degrees C, 512-block cells")
                .priority(0)
                .off()
                .no_cards(),
        ],
    }
}

/// The terrain base map: a square pyramid, zoom -7 (128 blocks a pixel) to 0 (one block).
pub struct Terrain {
    pub seed: u64,
}

pub fn terrain_color(h: f64) -> [u8; 3] {
    let lerp = |a: [f64; 3], b: [f64; 3], k: f64| {
        let k = k.clamp(0.0, 1.0);
        [(a[0] + (b[0] - a[0]) * k) as u8, (a[1] + (b[1] - a[1]) * k) as u8, (a[2] + (b[2] - a[2]) * k) as u8]
    };
    if h < 0.0 {
        lerp([38.0, 74.0, 110.0], [14.0, 30.0, 56.0], -h * 2.0)
    } else if h < 0.03 {
        [196, 184, 140]
    } else if h < 0.35 {
        lerp([88.0, 128.0, 70.0], [62.0, 100.0, 54.0], (h - 0.03) / 0.32)
    } else if h < 0.55 {
        lerp([110.0, 108.0, 78.0], [128.0, 118.0, 104.0], (h - 0.35) / 0.2)
    } else {
        lerp([150.0, 146.0, 140.0], [240.0, 242.0, 246.0], (h - 0.55) / 0.2)
    }
}

impl TileSource for Terrain {
    fn render(&self, layer: &str, z: i32, x: i64, y: i64) -> Result<Rendered, TileError> {
        if layer != "terrain" {
            return Err(TileError::Missing(format!("no layer {layer}")));
        }
        if !(-7..=0).contains(&z) {
            return Err(TileError::Missing(format!("zoom {z} is outside -7 to 0")));
        }
        let px = 2f64.powi(-z);
        let span = px * f64::from(TILE);
        let (x0, z0) = (x as f64 * span, y as f64 * span);
        if x0 >= HALF || z0 >= HALF || x0 + span <= -HALF || z0 + span <= -HALF {
            return Ok(Rendered::Empty);
        }
        let mut out = vec![0u8; (TILE * TILE * 4) as usize];
        for j in 0..TILE {
            for i in 0..TILE {
                let (bx, bz) = (x0 + (f64::from(i) + 0.5) * px, z0 + (f64::from(j) + 0.5) * px);
                let k = ((j * TILE + i) * 4) as usize;
                if bx.abs() > HALF || bz.abs() > HALF {
                    continue;
                }
                let h = terrain(self.seed, bx, bz);
                // A little relief: lit from the north-west.
                let e = px.max(4.0);
                let dh = terrain(self.seed, bx - e, bz - e) - h;
                let shade = (1.0 + dh * 60.0 / e.sqrt()).clamp(0.75, 1.2);
                let c = terrain_color(h);
                let s = if h < 0.0 { 1.0 } else { shade };
                out[k] = (f64::from(c[0]) * s).min(255.0) as u8;
                out[k + 1] = (f64::from(c[1]) * s).min(255.0) as u8;
                out[k + 2] = (f64::from(c[2]) * s).min(255.0) as u8;
                out[k + 3] = 255;
            }
        }
        Ok(Rendered::Rgba(out))
    }
    fn opaque(&self, _layer: &str) -> bool {
        false
    }
    fn version(&self) -> u32 {
        1
    }
}

/// The demo as a viewer backend.
/// How the demo's clock moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// Stands at START; nothing is published.
    Still,
    /// 20 ticks a wall-clock second; published every 2 s.
    Wall,
    /// Each publish moves it `ticks`; published every `every_ms`.
    Stepped { ticks: Tick, every_ms: u64 },
}

impl Clock {
    /// Wall-clock milliseconds between publishes, or None when nothing is published.
    pub fn cycle_ms(self) -> Option<u64> {
        match self {
            Clock::Still => None,
            Clock::Wall => Some(2000),
            Clock::Stepped { every_ms, .. } => Some(every_ms),
        }
    }
}

pub struct Demo {
    pub world: World,
    pub size: Size,
    /// The clock moves (not `Clock::Still`).
    pub running: bool,
    pub clock: Clock,
    started: Instant,
    publishes: AtomicI64,
    layers: Arc<Layers>,
    tiles: Tiles,
    changes: Changes,
    current: RwLock<(Tick, Arc<Scene>, Arc<Scene>)>,
    live_events: Mutex<Vec<EventRow>>,
    watchers: AtomicUsize,
}

impl Demo {
    /// A demo on the wall clock (`running`) or standing still.
    pub fn new(seed: u64, size: Size, running: bool) -> Demo {
        Demo::with_clock(seed, size, if running { Clock::Wall } else { Clock::Still })
    }

    pub fn with_clock(seed: u64, size: Size, clock: Clock) -> Demo {
        let world = World::new(seed, size);
        let s = Arc::new(world.scene(OVERWORLD, START, size.caravans));
        let r = Arc::new(world.scene(RING, START, 0));
        Demo {
            size,
            running: clock != Clock::Still,
            clock,
            started: Instant::now(),
            publishes: AtomicI64::new(0),
            layers: Arc::new(layers()),
            tiles: Tiles::new(Arc::new(Terrain { seed }), TileCache::in_memory(64 << 20), true),
            changes: Changes::new(),
            current: RwLock::new((START, s, r)),
            live_events: Mutex::new(Vec::new()),
            world,
            watchers: AtomicUsize::new(0),
        }
    }

    /// The game tick now: 20 a second from START on the wall clock; START plus the publishes' steps
    /// when stepped.
    pub fn now(&self) -> Tick {
        match self.clock {
            Clock::Still => START,
            Clock::Wall => START + (self.started.elapsed().as_millis() as Tick) / 50,
            Clock::Stepped { ticks, .. } => START + self.publishes.load(Ordering::Acquire) * ticks,
        }
    }

    /// Publishes a new scene (the host's step tail would): called every `clock.cycle_ms()`.
    pub fn publish(&self) {
        self.publishes.fetch_add(1, Ordering::AcqRel);
        let t = self.now();
        let s = Arc::new(self.world.scene(OVERWORLD, t, self.size.caravans));
        let r = Arc::new(self.world.scene(RING, t, 0));
        let prev = {
            let mut g = self.current.write().unwrap_or_else(|e| e.into_inner());
            let prev = g.0;
            *g = (t, s, r);
            prev
        };
        if t > prev {
            let mut rng = Rng::new(self.world.seed ^ t as u64);
            let mut rows = self.world.history(&mut rng, prev, t);
            rows.extend(self.world.ring_history(&mut rng, prev, t));
            rows.sort_by_key(|e| e.t);
            self.live_events.lock().unwrap_or_else(|e| e.into_inner()).extend(rows);
        }
        self.changes.publish();
    }

    pub fn open_streams(&self) -> usize {
        self.watchers.load(Ordering::Relaxed)
    }

    fn all_events(&self) -> Vec<EventRow> {
        let live = self.live_events.lock().unwrap_or_else(|e| e.into_inner());
        self.world.events.iter().chain(live.iter()).cloned().collect()
    }
}

/// Whether an event is in the query's frame (no frame on either side: the overworld, the first).
fn in_frame(e: &EventRow, q: &EventQuery) -> bool {
    e.frame.as_deref().unwrap_or(OVERWORLD) == q.frame.as_deref().unwrap_or(OVERWORLD)
}

impl Backend for Demo {
    fn world(&self) -> WorldInfo {
        let tick = self.current.read().unwrap_or_else(|e| e.into_inner()).0;
        let terrain = TileLayerDesc {
            id: "terrain".into(),
            label: "Terrain".into(),
            url: "tiles/terrain/{z}/{x}/{y}.png".into(),
            pyramid: PyramidDesc { kind: "square".into(), min_zoom: -7, max_zoom: 0 },
            bounds: [-HALF, -HALF, HALF, HALF],
            overlay: false,
            attribution: "synthetic terrain".into(),
            default_on: None,
            alpha: None,
        };
        WorldInfo {
            name: "Demo world".into(),
            source: "demo".into(),
            source_note: Some(match self.clock {
                Clock::Still => "synthetic, clock stopped".into(),
                Clock::Wall => "synthetic, moving at 20 ticks a second".into(),
                Clock::Stepped { ticks, every_ms } => format!("synthetic, {ticks} ticks every {every_ms} ms"),
            }),
            build: format!("mic-viewer-demo {}", env!("CARGO_PKG_VERSION")),
            frames: vec![
                Frame {
                    id: OVERWORLD.into(),
                    label: "Overworld".into(),
                    projection: ProjectionDesc::flat(),
                    bounds: [-HALF, -HALF, HALF, HALF],
                    tiles: vec![terrain],
                },
                Frame {
                    id: RING.into(),
                    label: "Ring (wraps east to west)".into(),
                    projection: ProjectionDesc::xwrap(RING_PERIOD, Some([-RING_HALF_Z, RING_HALF_Z])),
                    bounds: [-RING_PERIOD / 2.0, -RING_HALF_Z, RING_PERIOD / 2.0, RING_HALF_Z],
                    tiles: Vec::new(),
                },
            ],
            time: TimeInfo {
                tick,
                ticks_per_day: TICKS_PER_DAY,
                wall_ms: 1_792_560_000_000 + (tick - START) * 50,
                tps: match self.clock {
                    Clock::Still => 0.0,
                    Clock::Wall => 20.0,
                    Clock::Stepped { ticks, every_ms } => ticks as f64 * 1000.0 / every_ms.max(1) as f64,
                },
                running: self.running,
            },
            freshness: Freshness {
                age_ms: 0,
                cycle_ms: self.clock.cycle_ms(),
                note: match self.clock.cycle_ms() {
                    Some(ms) => format!("republished every {} s", ms as f64 / 1000.0),
                    None => "nothing moves".into(),
                },
            },
            budget: mic_viewer::api::MARK_BUDGET,
            event_kinds: KINDS
                .iter()
                .map(|(id, label, group, color, severity)| EventKind {
                    id: (*id).into(),
                    label: (*label).into(),
                    group: (*group).into(),
                    color: (*color).into(),
                    severity: *severity,
                })
                .collect(),
            has_events: true,
            stream: true,
        }
    }

    fn layers(&self) -> Arc<Layers> {
        Arc::clone(&self.layers)
    }

    fn scene(&self, frame: &str, t: Option<Tick>) -> Option<Arc<Scene>> {
        let g = self.current.read().unwrap_or_else(|e| e.into_inner());
        match (frame, t) {
            (OVERWORLD, None) => Some(Arc::clone(&g.1)),
            (RING, None) => Some(Arc::clone(&g.2)),
            // The demo can show any past: its world is a function of time.
            (OVERWORLD | RING, Some(t)) => Some(Arc::new(self.world.scene(frame, t, self.size.caravans))),
            _ => None,
        }
    }

    fn card(&self, layer: &str, id: &str, t: Option<Tick>) -> Option<Card> {
        self.world.card(layer, id, t.unwrap_or_else(|| self.current.read().unwrap_or_else(|e| e.into_inner()).0))
    }

    fn events(&self, q: &EventQuery) -> EventsAnswer {
        let mut rows: Vec<EventRow> = self
            .all_events()
            .into_iter()
            .filter(|e| e.t >= q.from && e.t < q.to && q.wants(&e.kind) && in_frame(e, q))
            .filter(|e| q.area.is_none_or(|b| e.x >= b[0] && e.x <= b[2] && e.z >= b[1] && e.z <= b[3]))
            .collect();
        let truncated = rows.len() > q.limit;
        // The newest rows when there are too many.
        if truncated {
            rows.drain(..rows.len() - q.limit);
        }
        // The raid parties' walks, as legs.
        let legs = rows
            .iter()
            .filter(|e| e.kind == "conflict.raid")
            .take(200)
            .map(|e| LegRow {
                t0: e.t - 2000,
                t1: e.t,
                kind: "conflict.party".into(),
                subject: e.subject.clone(),
                p: vec![[e.x - 400.0, e.z - 300.0, e.x, e.z]],
            })
            .collect();
        EventsAnswer { rows, legs, truncated }
    }

    fn histogram(&self, q: &EventQuery) -> Histogram {
        let events = self.all_events();
        let from = if q.from == Tick::MIN { events.first().map_or(0, |e| e.t) } else { q.from };
        let to = if q.to == Tick::MAX { self.now().max(from + 1) } else { q.to };
        let buckets = q.limit.max(1);
        let width = ((to - from) as f64 / buckets as f64).max(1.0);
        let mut series: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for e in events.iter().filter(|e| e.t >= from && e.t < to && q.wants(&e.kind) && in_frame(e, q)) {
            let b = (((e.t - from) as f64 / width) as usize).min(buckets - 1);
            series.entry(e.kind.clone()).or_insert_with(|| vec![0; buckets])[b] += 1;
        }
        let wall_of = |t: Tick| 1_792_560_000_000 + (t - START) * 50;
        Histogram { from, to, buckets, series, wall: vec![[from, wall_of(from)], [to, wall_of(to)]] }
    }

    fn events_since(&self, cursor: &mut u64) -> Vec<EventRow> {
        let live = self.live_events.lock().unwrap_or_else(|e| e.into_inner());
        let from = (*cursor as usize).min(live.len());
        *cursor = live.len() as u64;
        live[from..].to_vec()
    }

    fn tiles(&self, source: &str) -> Option<&Tiles> {
        (source == "terrain").then_some(&self.tiles)
    }

    fn changes(&self) -> Option<&Changes> {
        Some(&self.changes)
    }

    fn watchers(&self, open: usize) {
        self.watchers.store(open, Ordering::Relaxed);
    }
}
