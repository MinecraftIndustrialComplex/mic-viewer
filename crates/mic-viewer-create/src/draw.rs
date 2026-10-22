// SPDX-License-Identifier: MPL-2.0
//! Create's rail and factories as viewer layers: their descriptions, their marks, their cards and
//! what `/api/find` finds among them. A backend composes these into its own scenes; the colour
//! `ink` (and `ink2`, `paper`) is the page's map ink for its theme.

use std::collections::{BTreeMap, HashMap};

use mic_viewer::api::{Card, Found, LayerDesc, LegendEntry, Mark, Section};
use mic_viewer::scene::SceneWriter;

use crate::blocks::{Detail, Kind, Net, Survey, Thing};
use crate::rail::{Rail, Train};
use crate::tags::{self, humanize, uuid_text};

pub const TRACK: &str = "rail.track";
pub const STATIONS: &str = "rail.stations";
pub const SIGNALS: &str = "rail.signals";
pub const TRAINS: &str = "rail.trains";
pub const MACHINES: &str = "fact.machines";
pub const GAUGES: &str = "fact.gauges";
pub const STORAGE: &str = "fact.storage";
pub const CHAINS: &str = "logi.chains";
pub const BELTS: &str = "logi.belts";
pub const PORTS: &str = "logi.ports";

/// The rail layers.
pub const RAIL_LAYERS: [&str; 4] = [TRACK, STATIONS, SIGNALS, TRAINS];
/// The layers read from block entities.
pub const BLOCK_LAYERS: [&str; 6] = [MACHINES, GAUGES, STORAGE, CHAINS, BELTS, PORTS];

/// Networks by length, longest first; the rest grey.
/// Network colours, none of them green: green is the districts' and the signals'.
pub const NETWORK_COLORS: [&str; 6] = ["#2f6db5", "#c2477a", "#8250c4", "#b4572e", "#4b5bd0", "#9a7b0a"];
pub const SMALL_NETWORK: &str = "#8b949e";
pub const BAD: &str = "#d1242f";
pub const WARN: &str = "#c08a1e";
pub const GOOD: &str = "#2da44e";
pub const AMBER: &str = "#b7791f";

/// Blocks a network needs, with a station, to get a colour of its own.
const NAMED_NETWORK_LENGTH: f64 = 200.0;

/// Zoom bands (blocks per CSS pixel).
pub const SITE_BPP: f64 = 2.0;
pub const TOWN_BPP: f64 = 16.0;
/// Signals draw closer than this.
pub const SIGNAL_BPP: f64 = 0.75;

/// The layer descriptions, rail first.
pub fn layer_descs() -> Vec<LayerDesc> {
    let mut track_legend: Vec<LegendEntry> = NETWORK_COLORS
        .iter()
        .enumerate()
        .map(|(i, c)| LegendEntry::new(&format!("Network {}", i + 1), c).line("rail"))
        .collect();
    track_legend.push(LegendEntry::new("Other networks and sidings", SMALL_NETWORK).line("rail"));
    vec![
        LayerDesc::new(TRACK, "Rail", "Track", "path", NETWORK_COLORS[0])
            .describe("Create's track as built, curves and all; each separate network in its own colour.")
            .kinds(&["path"])
            .line("rail")
            .width(2.0)
            .priority(80)
            .legend_entries(track_legend),
        LayerDesc::new(STATIONS, "Rail", "Stations", "point", "ink")
            .describe("Train stations with their Create names, turned along the track.")
            .glyph("station", 7.0)
            .priority(85)
            .labels_below(24.0)
            .legend_entries(vec![
                LegendEntry::new("Station", "ink").glyph("station"),
                LegendEntry::new("Train standing at it", "ink").glyph("station").st("warn"),
            ]),
        LayerDesc::new(SIGNALS, "Rail", "Signals", "point", GOOD)
            .describe("Signals, in their state as saved.")
            .glyph("signal", 6.0)
            // Closer than a town: a junction's signals are noise any further out.
            .zoom(0.0, SIGNAL_BPP)
            .priority(40)
            .legend_entries(vec![
                LegendEntry::new("Signal: clear", GOOD).glyph("signal"),
                LegendEntry::new("Signal: caution", WARN).glyph("signal"),
                LegendEntry::new("Signal: stop", BAD).glyph("signal"),
            ]),
        LayerDesc::new(TRAINS, "Rail", "Trains", "train", "ink")
            .describe("Trains where they are: a box for each carriage, the leading one pointed the way it runs.")
            .kinds(&["train"])
            .priority(90)
            // Station names first; trains' names once a yard is apart.
            .labels_below(1.5)
            .legend_entries(vec![
                LegendEntry::new("Train running", "ink").glyph("train"),
                LegendEntry::new("Train standing", "ink").glyph("train").st("off"),
                LegendEntry::new("Train derailed", BAD).glyph("train").st("bad"),
            ]),
        LayerDesc::new(MACHINES, "Factories", "Machines", "point", "ink")
            .describe("Create machines and generators read from the world's blocks (presses, mixers, crafters, saws, fans, wheels, engines), grouped where they stand together.")
            .glyph("circle", 6.0)
            .zoom(0.0, SITE_BPP)
            .priority(50)
            .labels_below(0.6)
            .legend_entries(vec![
                LegendEntry::new("Processing machine", "ink").glyph("circle"),
                LegendEntry::new("Power source", "ink").glyph("triangle"),
                LegendEntry::new("Arm or ejector", "ink").glyph("diamond"),
                LegendEntry::new("Stopped", "ink").glyph("circle").st("off"),
                LegendEntry::new("Network overstressed", "ink").glyph("circle").st("bad"),
            ]),
        LayerDesc::new(GAUGES, "Factories", "Factory gauges", "point", GOOD)
            .describe("Create's factory gauges: the item each keeps in stock, which is what the factory makes.")
            .glyph("gauge", 7.0)
            .zoom(0.0, 4.0)
            .priority(60)
            .labels_below(2.0)
            .legend_entries(vec![
                LegendEntry::new("Gauge: stock met", GOOD).glyph("gauge"),
                LegendEntry::new("Gauge: short", WARN).glyph("gauge").st("warn"),
            ]),
        LayerDesc::new(STORAGE, "Factories", "Storage", "point", "ink")
            .describe("Item vaults and fluid tanks, labelled with what they hold most of.")
            .glyph("square", 6.0)
            .zoom(0.0, SITE_BPP)
            .priority(45)
            .labels_below(0.6)
            .legend_entries(vec![
                LegendEntry::new("Item vault", "ink").glyph("square"),
                LegendEntry::new("Fluid tank", "#3f7f9f").glyph("square"),
            ]),
        LayerDesc::new(CHAINS, "Logistics", "Chain conveyors", "path", AMBER)
            .describe("Chain conveyor routes between pulleys, with the packages on them.")
            .kinds(&["path"])
            .line("chain")
            .width(2.5)
            .zoom(0.0, TOWN_BPP)
            .priority(70)
            .legend_entries(vec![
                LegendEntry::new("Chain conveyor", AMBER).line("chain"),
                LegendEntry::new("Chain conveyor, not turning", SMALL_NETWORK).line("chain"),
                LegendEntry::new("Packages on it", AMBER).glyph("dot"),
            ]),
        LayerDesc::new(BELTS, "Logistics", "Belts", "path", "ink")
            .describe("Mechanical belts, chevrons the way they run (none when stopped).")
            .kinds(&["path"])
            .line("belt")
            .zoom(0.0, SITE_BPP)
            .priority(55)
            .legend_entries(vec![
                LegendEntry::new("Belt, chevrons the way it runs", "ink").line("belt"),
                LegendEntry::new("Belt, stopped", "ink2").line("belt"),
            ]),
        LayerDesc::new(PORTS, "Logistics", "Package addresses", "point", AMBER)
            .describe("Frogports, packagers, postboxes and requesters, with the addresses they send to or take.")
            .glyph("port", 6.0)
            .zoom(0.0, 4.0)
            .priority(65)
            .labels_below(1.0)
            .legend_entries(vec![
                LegendEntry::new("Frogport", AMBER).glyph("port"),
                LegendEntry::new("Packager", "#8a5a2b").glyph("port"),
                LegendEntry::new("Postbox", "#c2477a").glyph("port"),
                LegendEntry::new("Requester or stock link", "#6b8e23").glyph("port"),
            ]),
    ]
}

/// A network's colour by its rank, and its name.
pub struct Networks {
    /// Graph index to rank (0 longest).
    pub rank: Vec<usize>,
    pub colored: Vec<bool>,
}

impl Networks {
    pub fn of(rail: &Rail) -> Networks {
        let mut order: Vec<usize> = (0..rail.graphs.len()).collect();
        order.sort_by(|&a, &b| rail.graphs[b].length.total_cmp(&rail.graphs[a].length).then(a.cmp(&b)));
        let mut rank = vec![0; rail.graphs.len()];
        for (r, &g) in order.iter().enumerate() {
            rank[g] = r;
        }
        let has_station: Vec<bool> =
            (0..rail.graphs.len()).map(|g| rail.stations.iter().any(|s| s.place.graph == g)).collect();
        let colored = (0..rail.graphs.len())
            .map(|g| rank[g] < NETWORK_COLORS.len() && has_station[g] && rail.graphs[g].length >= NAMED_NETWORK_LENGTH)
            .collect();
        Networks { rank, colored }
    }

    pub fn color(&self, g: usize) -> &'static str {
        if self.colored.get(g).copied().unwrap_or(false) { NETWORK_COLORS[self.rank[g]] } else { SMALL_NETWORK }
    }

    /// The legend entry of graph `g`.
    pub fn ci(&self, g: usize) -> u16 {
        if self.colored.get(g).copied().unwrap_or(false) { self.rank[g] as u16 } else { NETWORK_COLORS.len() as u16 }
    }

    pub fn name(&self, g: usize) -> String {
        format!("Network {}", self.rank.get(g).map_or(0, |r| r + 1))
    }
}

fn train_doing(rail: &Rail, t: &Train) -> (String, Option<&'static str>) {
    let station_name = |id: &tags::Uuid| rail.station(id).map(|s| s.name.clone());
    if t.derailed {
        return ("Derailed".into(), Some("bad"));
    }
    let to = t
        .destination
        .as_ref()
        .and_then(station_name)
        .or_else(|| t.schedule.get(t.current).filter(|s| s.kind == "destination").map(|s| s.text.clone()));
    if t.moving() {
        let bps = t.speed.abs() * 20.0;
        let mut s = format!("Running at {bps:.0} blocks a second");
        if let Some(to) = to {
            s.push_str(&format!(" to {to}"));
        }
        if t.speed < 0.0 {
            s.push_str(", backwards");
        }
        return (s, None);
    }
    if let Some(at) = t.at_station.as_ref().and_then(station_name) {
        let why = if t.schedule.is_empty() {
            ", no schedule"
        } else if t.paused {
            ", schedule paused"
        } else if t.completed {
            ", schedule finished"
        } else {
            ""
        };
        return (format!("Standing at {at}{why}"), Some("off"));
    }
    if t.schedule.is_empty() {
        return ("Stopped on the line, no schedule".into(), Some("off"));
    }
    match to {
        Some(to) => (format!("Waiting on the line, bound for {to}"), Some("off")),
        None => ("Stopped on the line".into(), Some("off")),
    }
}

/// The rail of dimension `dim` (palette index) into `w`.
pub fn rail_marks(rail: &Rail, dim: i32, w: &mut SceneWriter) {
    let nets = Networks::of(rail);
    for l in RAIL_LAYERS {
        w.layer(l);
    }
    for (g, graph) in rail.graphs.iter().enumerate() {
        if graph.dim != dim {
            continue;
        }
        for (k, p) in graph.strokes().into_iter().enumerate() {
            if p.len() < 2 {
                continue;
            }
            w.mark(TRACK, Mark::path(p).with_id(format!("{g}:{k}")).with_ci(nets.ci(g)));
        }
    }
    let standing: Vec<tags::Uuid> = rail.trains.iter().filter_map(|t| t.at_station).collect();
    for s in &rail.stations {
        if rail.graphs[s.place.graph].dim != dim {
            continue;
        }
        let mut m =
            Mark::point(uuid_text(&s.id), s.place.at[0], s.place.at[2]).with_heading(s.place.heading).with_ci(0);
        // A station Create named for itself ("Track Station") says nothing on the map; its card
        // and the search still have it.
        if !matches!(s.name.as_str(), "Track Station" | "") {
            m = m.with_label(s.name.clone());
        }
        if standing.contains(&s.id) {
            m = m.with_ci(1).with_state("warn");
        }
        w.mark(STATIONS, m);
    }
    for (k, sg) in rail.signals.iter().enumerate() {
        if rail.graphs[sg.place.graph].dim != dim {
            continue;
        }
        let ci = match sg.state() {
            "RED" => 2,
            "YELLOW" => 1,
            _ => 0,
        };
        w.mark(SIGNALS, Mark::point(k, sg.place.at[0], sg.place.at[2]).with_heading(sg.place.heading).with_ci(ci));
    }
    for t in &rail.trains {
        if t.dim != dim || t.cars.is_empty() {
            continue;
        }
        let (_, st) = train_doing(rail, t);
        let mut m = Mark::train(uuid_text(&t.id), t.cars.clone());
        // Create's default names say nothing on the map (the card still has them).
        if !matches!(t.name.as_str(), "Unnamed Train" | "Unnamed" | "") {
            m = m.with_label(t.name.clone());
        }
        m = match st {
            Some("bad") => m.with_ci(2).with_state("bad"),
            Some(s) => m.with_ci(1).with_state(s),
            None => m.with_ci(0),
        };
        w.mark(TRAINS, m);
    }
}

/// An id for a block: `x,y,z`, with `@dim` outside the overworld.
pub fn block_id(pos: [i32; 3], dim: &str) -> String {
    if dim == "minecraft:overworld" || dim.is_empty() {
        format!("{},{},{}", pos[0], pos[1], pos[2])
    } else {
        format!("{},{},{}@{dim}", pos[0], pos[1], pos[2])
    }
}

/// The block and dimension an id names.
pub fn parse_block_id(id: &str) -> Option<([i32; 3], String)> {
    let (p, dim) = id.split_once('@').unwrap_or((id, "minecraft:overworld"));
    let v: Vec<i32> = p.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() == 3).then(|| ([v[0], v[1], v[2]], dim.to_owned()))
}

fn center(p: [i32; 3]) -> [f64; 2] {
    [f64::from(p[0]) + 0.5, f64::from(p[2]) + 0.5]
}

fn state_of<'a>(things: impl IntoIterator<Item = &'a Thing>) -> Option<&'static str> {
    let mut any_running = false;
    let mut any_kinetic = false;
    for t in things {
        if t.net.is_some_and(|n| n.overstressed()) {
            return Some("bad");
        }
        if t.net.is_some() {
            any_kinetic = true;
            any_running |= t.running();
        }
    }
    (any_kinetic && !any_running).then_some("off")
}

/// Groups of things of one id standing within two blocks of each other.
fn groups<'a>(things: &[&'a Thing]) -> Vec<Vec<&'a Thing>> {
    let mut at: HashMap<(&str, [i32; 3]), usize> = HashMap::new();
    for (k, t) in things.iter().enumerate() {
        at.insert((t.id.as_str(), t.pos), k);
    }
    let mut parent: Vec<usize> = (0..things.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut x = x;
        while p[x] != r {
            let n = p[x];
            p[x] = r;
            x = n;
        }
        r
    }
    for (k, t) in things.iter().enumerate() {
        for dx in -2..=2 {
            for dy in -2..=2 {
                for dz in -2..=2 {
                    if let Some(&o) = at.get(&(t.id.as_str(), [t.pos[0] + dx, t.pos[1] + dy, t.pos[2] + dz])) {
                        let (a, b) = (find(&mut parent, k), find(&mut parent, o));
                        if a != b {
                            parent[a] = b;
                        }
                    }
                }
            }
        }
    }
    let mut by: BTreeMap<usize, Vec<&Thing>> = BTreeMap::new();
    for (k, t) in things.iter().enumerate() {
        let r = find(&mut parent, k);
        by.entry(r).or_default().push(*t);
    }
    by.into_values().collect()
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        return one.to_owned();
    }
    let lower = one.to_lowercase();
    if lower.ends_with('s') && !lower.ends_with("ss") {
        format!("{n} {lower}")
    } else if lower.ends_with("ss") || lower.ends_with("ch") || lower.ends_with("sh") || lower.ends_with('x') {
        format!("{n} {lower}es")
    } else {
        format!("{n} {lower}s")
    }
}

fn top_item(items: &BTreeMap<String, i64>) -> Option<(&String, i64)> {
    items.iter().max_by_key(|(_, n)| **n).map(|(k, n)| (k, *n))
}

/// Block entities (any chunks of one dimension) into `w`.
pub fn block_marks(things: &[Thing], dim: &str, w: &mut SceneWriter) {
    for l in BLOCK_LAYERS {
        w.layer(l);
    }
    // Machines, power and movers, grouped.
    let drawn: Vec<&Thing> =
        things.iter().filter(|t| matches!(t.kind, Kind::Machine | Kind::Power | Kind::Mover)).collect();
    for g in groups(&drawn) {
        let first = g[0];
        let n = g.len();
        let (sx, sz) = g.iter().fold((0.0, 0.0), |a, t| {
            let c = center(t.pos);
            (a.0 + c[0], a.1 + c[1])
        });
        let (glyph, ci) = match first.kind {
            Kind::Power => ("triangle", 1),
            Kind::Mover => ("diamond", 2),
            _ => ("circle", 0),
        };
        let mut m = Mark::point(block_id(first.pos, dim), sx / n as f64, sz / n as f64)
            .with_glyph(glyph)
            .with_ci(ci)
            .with_label(plural(n, &first.name()));
        if n > 1 {
            m = m.with_size(1.0 + (n as f64).ln() * 0.35);
        }
        match state_of(g.iter().copied()) {
            Some("bad") => m = m.with_state("bad").with_ci(4),
            Some(s) => m = m.with_state(s).with_ci(3),
            None => {}
        }
        w.mark(MACHINES, m);
    }
    for t in things {
        match &t.detail {
            Detail::Gauge(slots) if !slots.is_empty() => {
                let short = slots.iter().any(|s| !s.satisfied);
                let label = slots.iter().map(|s| humanize(&s.item)).collect::<Vec<_>>().join(", ");
                let c = center(t.pos);
                let mut m = Mark::point(block_id(t.pos, dim), c[0], c[1]).with_label(label).with_ci(0);
                if short {
                    m = m.with_ci(1).with_state("warn");
                }
                w.mark(GAUGES, m);
            }
            Detail::Store { controller: true, items, fluids, capacity } => {
                let c = center(t.pos);
                let tank = t.id.contains("tank");
                let mut m = Mark::point(block_id(t.pos, dim), c[0], c[1]).with_ci(u16::from(tank));
                let top = if tank { top_item(fluids) } else { top_item(items) };
                if let Some((id, _)) = top {
                    m = m.with_label(humanize(id));
                }
                if *capacity > 1000 || *capacity > 20 && !tank {
                    m = m.with_size(1.3);
                }
                w.mark(STORAGE, m);
            }
            Detail::Chain { links, travelling, .. } => {
                for q in links {
                    // Each link once: from the lower pulley, or from this one when the other is
                    // in a chunk not read (it may never be).
                    let other = things.iter().find(|o| o.pos == *q && o.kind == Kind::Chain);
                    if other.is_some() && *q < t.pos {
                        continue;
                    }
                    let mut n: u32 = travelling.iter().filter(|(to, _)| to == q).map(|(_, n)| n).sum();
                    if let Some(Detail::Chain { travelling: back, .. }) = other.map(|o| &o.detail) {
                        n += back.iter().filter(|(to, _)| *to == t.pos).map(|(_, n)| n).sum::<u32>();
                    }
                    let mut m = Mark::path(vec![center(t.pos), center(*q)]).with_id(block_id(t.pos, dim));
                    if n > 0 {
                        m = m.with_count(n);
                    }
                    // The key's entries: turning, or not.
                    m = m.with_ci(if t.running() { 0 } else { 1 });
                    w.mark(CHAINS, m);
                }
            }
            Detail::Frogport { address, .. } => port(w, t, dim, address, 0),
            Detail::Requester { address, .. } => port(w, t, dim, address, 3),
            Detail::Address(a) => {
                let ci = if t.id.ends_with("postbox") {
                    2
                } else if t.id.ends_with("packager") {
                    1
                } else {
                    3
                };
                port(w, t, dim, a, ci);
            }
            _ => {}
        }
    }
    // Belts: segments by controller, in index order.
    let mut belts: BTreeMap<[i32; 3], Vec<(u32, &Thing)>> = BTreeMap::new();
    for t in things {
        if let Detail::Belt { controller, index, .. } = &t.detail {
            belts.entry(*controller).or_default().push((*index, t));
        }
    }
    for (ctl, mut segs) in belts {
        segs.sort_by_key(|s| s.0);
        let mut p: Vec<[f64; 2]> = Vec::new();
        for (_, t) in &segs {
            let c = center(t.pos);
            if p.last() != Some(&c) {
                p.push(c);
            }
        }
        let first = segs[0].1;
        if p.len() == 1 {
            // A vertical belt, or one segment read: a stub along nothing.
            p.push([p[0][0] + 0.01, p[0][1]]);
        }
        let running = segs.iter().any(|(_, t)| t.running());
        let mut m = Mark::path(p.clone()).with_id(block_id(ctl, dim));
        if running {
            // Create's `getMovementFacing`: along z with the speed's sign, along x against it.
            let d = [p[p.len() - 1][0] - p[0][0], p[p.len() - 1][1] - p[0][1]];
            let along_x = d[0].abs() > d[1].abs();
            let sign = if first.speed >= 0.0 { 1.0 } else { -1.0 };
            let moving = if along_x { [-sign, 0.0] } else { [0.0, sign] };
            if moving[0] * d[0] + moving[1] * d[1] < 0.0 {
                let mut r = p;
                r.reverse();
                m = Mark::path(r).with_id(block_id(ctl, dim));
            }
            m = m.with_arrows().with_ci(0);
        } else {
            m = m.with_ci(1);
        }
        w.mark(BELTS, m);
    }
}

fn port(w: &mut SceneWriter, t: &Thing, dim: &str, address: &str, ci: u16) {
    let c = center(t.pos);
    let mut m = Mark::point(block_id(t.pos, dim), c[0], c[1]).with_ci(ci);
    if !address.is_empty() {
        m = m.with_label(address.to_owned());
    }
    w.mark(PORTS, m);
}

fn fmt_int(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (k, ch) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn fluid_text(mb: i64) -> String {
    if mb >= 1000 { format!("{:.1} buckets", mb as f64 / 1000.0) } else { format!("{mb} mB") }
}

/// A rail card (`rail.*`).
pub fn rail_card(rail: &Rail, layer: &str, id: &str) -> Option<Card> {
    let nets = Networks::of(rail);
    let station_link = |c: Card, label: &str, sid: &tags::Uuid| match rail.station(sid) {
        Some(s) => c.link(label, s.name.clone(), STATIONS, uuid_text(sid)),
        None => c,
    };
    match layer {
        STATIONS => {
            let s = rail.stations.iter().find(|s| uuid_text(&s.id) == id)?;
            let g = s.place.graph;
            let mut c = Card::new(layer, id, s.name.clone())
                .subtitle(format!("Station on {}", nets.name(g)))
                .at(s.place.at[0], s.place.at[2]);
            c = c.link("Network", nets.name(g), TRACK, format!("{g}:0"));
            if let Some(b) = s.block {
                c = c.field("Block", format!("{}, {}, {}", b[0], b[1], b[2]));
            }
            if s.assembling {
                c = c.field("State", "assembling a train");
            }
            for a in &s.ports {
                c = c.field("Postbox address", a.clone());
            }
            let here: Vec<&Train> = rail.trains.iter().filter(|t| t.at_station == Some(s.id)).collect();
            let mut sec = Section::new("Trains standing here");
            for t in &here {
                sec.fields
                    .push(mic_viewer::api::Field::new(&t.name, train_doing(rail, t).0).link(TRAINS, uuid_text(&t.id)));
            }
            if !sec.fields.is_empty() {
                c = c.section(sec);
            }
            let mut calling = Section::new("Trains whose schedule calls here");
            for t in &rail.trains {
                let stops = t.schedule.iter().filter(|st| st.kind == "destination").count();
                let Some((k, st)) = t
                    .schedule
                    .iter()
                    .filter(|st| st.kind == "destination")
                    .enumerate()
                    .find(|(_, st)| matches_station(&st.text, &s.name))
                else {
                    continue;
                };
                let mut v = if stops == 1 { "its only stop".to_owned() } else { format!("stop {} of {stops}", k + 1) };
                if let Some(w) = st.until.first() {
                    v.push_str(&format!(", {w}"));
                }
                if t.destination == Some(s.id) {
                    v.push_str(", bound here now");
                }
                calling.fields.push(mic_viewer::api::Field::new(&t.name, v).link(TRAINS, uuid_text(&t.id)));
            }
            if !calling.fields.is_empty() {
                c = c.section(calling);
            }
            Some(c)
        }
        TRAINS => {
            let t = rail.trains.iter().find(|t| uuid_text(&t.id) == id)?;
            let (doing, _) = train_doing(rail, t);
            let mut c = Card::new(layer, id, t.name.clone()).subtitle(doing);
            if let Some(f) = t.front() {
                c = c.at(f[0], f[1]);
            }
            if t.moving() {
                c = c.field_unit("Speed", (t.speed.abs() * 20.0 * 10.0).round() / 10.0, "blocks/s");
            }
            if let Some(sid) = &t.destination {
                c = station_link(c, "Bound for", sid);
                if let Some(d) = t.to_go {
                    c = c.field_unit("Distance left", d.round(), "blocks");
                }
            }
            if let Some(sid) = &t.at_station {
                c = station_link(c, "Standing at", sid);
            }
            c = c.field("Carriages", t.cars.len());
            c = c.field_unit("Length", t.length().round(), "blocks");
            if let Some(g) = t.graph {
                c = c.link("Network", nets.name(g), TRACK, format!("{g}:0"));
            }
            if t.double_ended {
                c = c.field("Driven from", "either end");
            }
            if !t.schedule.is_empty() {
                let title = if t.cyclic { "Schedule (repeats)" } else { "Schedule" };
                let mut sec = Section::new(title);
                for (k, st) in t.schedule.iter().enumerate() {
                    let mut v = st.describe();
                    if !st.until.is_empty() {
                        v.push_str(&format!(", then {}", st.until.join(", or ")));
                    }
                    let label = if k == t.current { format!("{} (now)", k + 1) } else { format!("{}", k + 1) };
                    sec.fields.push(mic_viewer::api::Field::new(&label, v));
                }
                c = c.section(sec);
            } else {
                c = c.field("Schedule", "none");
            }
            if !t.items.is_empty() || !t.fluids.is_empty() {
                let mut sec = Section::new("Cargo");
                let mut items: Vec<_> = t.items.iter().collect();
                items.sort_by(|a, b| b.1.cmp(a.1));
                for (id, n) in items.into_iter().take(16) {
                    sec = sec.field_unit(&humanize(id), *n, "items");
                }
                for (id, n) in &t.fluids {
                    sec = sec.field(&humanize(id), fluid_text(*n));
                }
                c = c.section(sec);
            } else {
                c = c.field("Cargo", "empty");
            }
            Some(c)
        }
        TRACK => {
            let g: usize = id.split(':').next()?.parse().ok()?;
            let graph = rail.graphs.get(g)?;
            let stations: Vec<_> = rail.stations.iter().filter(|s| s.place.graph == g).collect();
            let trains: Vec<_> = rail.trains.iter().filter(|t| t.graph == Some(g)).collect();
            let mut c = Card::new(layer, id, nets.name(g)).subtitle("A separate Create rail network");
            let mid = graph.nodes.first().map(|n| n.p).unwrap_or_default();
            c = c.at(mid[0], mid[2]);
            c = c.field_unit("Track", graph.length.round(), "blocks");
            c = c.field("Stations", stations.len());
            c = c.field("Trains", trains.len());
            c = c.field("Running now", trains.iter().filter(|t| t.moving()).count());
            let mut sec = Section::new("Stations");
            for s in stations.iter().take(24) {
                sec.fields.push(mic_viewer::api::Field::new(&s.name, "").link(STATIONS, uuid_text(&s.id)));
            }
            if !sec.fields.is_empty() {
                c = c.section(sec);
            }
            let mut sec = Section::new("Trains");
            for t in trains.iter().take(24) {
                sec.fields
                    .push(mic_viewer::api::Field::new(&t.name, train_doing(rail, t).0).link(TRAINS, uuid_text(&t.id)));
            }
            if !sec.fields.is_empty() {
                c = c.section(sec);
            }
            Some(c)
        }
        SIGNALS => {
            let k: usize = id.parse().ok()?;
            let s = rail.signals.get(k)?;
            let state = match s.state() {
                "RED" => "stop",
                "YELLOW" => "caution",
                "GREEN" => "clear",
                _ => "not set",
            };
            let kind = if s.types.iter().any(|t| t == "CROSS_SIGNAL") { "Chain signal" } else { "Signal" };
            Some(Card::new(layer, id, kind).subtitle(format!("Showing {state}")).at(s.place.at[0], s.place.at[2]).link(
                "Network",
                nets.name(s.place.graph),
                TRACK,
                format!("{}:0", s.place.graph),
            ))
        }
        _ => None,
    }
}

/// A schedule's station filter against a name: Create's `*` matches any run of characters.
pub fn matches_station(filter: &str, name: &str) -> bool {
    if !filter.contains('*') {
        return filter == name;
    }
    let parts: Vec<&str> = filter.split('*').collect();
    let mut rest = name;
    for (k, p) in parts.iter().enumerate() {
        if p.is_empty() {
            continue;
        }
        if k == 0 {
            match rest.strip_prefix(p) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if k == parts.len() - 1 {
            return rest.ends_with(p);
        } else {
            match rest.find(p) {
                Some(i) => rest = &rest[i + p.len()..],
                None => return false,
            }
        }
    }
    true
}

fn net_fields(c: Card, n: Option<&Net>, speed: f32) -> Card {
    let mut c = c;
    if speed != 0.0 {
        c = c.field_unit("Speed", f64::from(speed.abs()), "RPM");
    }
    if let Some(n) = n {
        let v = format!("{} of {} su", fmt_int(n.stress.round() as i64), fmt_int(n.capacity.round() as i64));
        c = c.field("Network stress", v);
        if n.overstressed() {
            c = c.field("State", "overstressed: everything on this network stands still");
        }
        // Create saves what a block takes or gives per RPM; the network counts it at the speed.
        let rpm = f64::from(speed.abs());
        if n.adds_stress > 0.0 {
            c = if rpm > 0.0 {
                c.field_unit("Takes", (f64::from(n.adds_stress) * rpm).round(), "su")
            } else {
                c.field_unit("Takes", f64::from(n.adds_stress), "su per RPM")
            };
        }
        if n.adds_capacity > 0.0 {
            c = if rpm > 0.0 {
                c.field_unit("Gives", (f64::from(n.adds_capacity) * rpm).round(), "su")
            } else {
                c.field_unit("Gives", f64::from(n.adds_capacity), "su per RPM")
            };
        }
    }
    c
}

/// A block entity's card (`fact.*`, `logi.*`), from what the survey has read.
pub fn block_card(survey: &Survey, layer: &str, id: &str) -> Option<Card> {
    let (pos, dim) = parse_block_id(id)?;
    let t = survey.at(&dim, pos)?;
    let c0 = center(t.pos);
    let mut c =
        Card::new(layer, id, t.name()).at(c0[0], c0[1]).field("Block", format!("{}, {}, {}", pos[0], pos[1], pos[2]));
    match &t.detail {
        Detail::Gauge(slots) => {
            c = c.subtitle("Keeps items in stock: what this factory makes");
            for s in slots {
                let mut v = format!("{} wanted", fmt_int(s.target));
                if !s.satisfied {
                    v.push_str(&format!(", {} on hand: short", fmt_int(s.level)));
                }
                if !s.address.is_empty() {
                    v.push_str(&format!(", sends to {}", s.address));
                }
                c = c.field(&humanize(&s.item), v);
            }
        }
        Detail::Store { items, fluids, capacity, .. } => {
            let tank = t.id.contains("tank");
            if tank {
                c = c.field("Holds", fluid_text(*capacity));
            } else {
                c = c.field_unit("Slots", *capacity, "stacks");
            }
            let mut sec = Section::new("Contents");
            let mut v: Vec<_> = items.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            for (id, n) in v.into_iter().take(16) {
                sec = sec.field_unit(&humanize(id), *n, "items");
            }
            for (id, n) in fluids {
                sec = sec.field(&humanize(id), fluid_text(*n));
            }
            if sec.fields.is_empty() {
                c = c.field("Contents", "empty");
            } else {
                c = c.section(sec);
            }
        }
        Detail::Chain { links, travelling, looping, addresses } => {
            c = c.subtitle("Chain conveyor pulley");
            c = c.field("Links", links.len());
            let on: u32 = travelling.iter().map(|x| x.1).sum::<u32>() + looping;
            c = c.field_unit("Packages on it", on, "packages");
            for q in links {
                let d = (((q[0] - pos[0]).pow(2) + (q[2] - pos[2]).pow(2)) as f64).sqrt();
                c = c.link(
                    &format!("To {}, {}, {}", q[0], q[1], q[2]),
                    format!("{d:.0} blocks"),
                    CHAINS,
                    block_id(*q, &dim),
                );
            }
            if !addresses.is_empty() {
                c = c.field("Addressed to", addresses.join(", "));
            }
            c = net_fields(c, t.net.as_ref(), t.speed);
        }
        Detail::Frogport { address, chain } => {
            c = c.subtitle("Takes packages off a chain conveyor, and puts them on");
            c = c.field("Address", if address.is_empty() { "any".to_owned() } else { address.clone() });
            if let Some(q) = chain {
                c = c.link("On the chain at", format!("{}, {}, {}", q[0], q[1], q[2]), CHAINS, block_id(*q, &dim));
            }
        }
        Detail::Requester { address, request } => {
            c = c.subtitle("Orders items when powered");
            c = c.field("Sends to", address.clone());
            for (id, n) in request {
                c = c.field_unit(&humanize(id), *n, "items");
            }
        }
        Detail::Address(a) => {
            if !a.is_empty() {
                c = c.field("Address", a.clone());
            }
        }
        Detail::Belt { length, .. } => {
            c = c.subtitle("Mechanical belt").field_unit("Length", *length, "blocks");
            c = net_fields(c, t.net.as_ref(), t.speed);
        }
        Detail::None => {
            c = net_fields(c, t.net.as_ref(), t.speed);
            if layer == MACHINES {
                let (all, _) = survey.in_chunks(&dim, [(pos[0] >> 4, pos[2] >> 4)]);
                let same = all.iter().filter(|o| o.id == t.id).count();
                if same > 1 {
                    c = c.field("Of these in its chunk", same);
                }
            }
        }
    }
    Some(c)
}

/// What the blocks in an area hold, as card sections: what it makes, machines, power and stress,
/// storage, logistics.
pub fn census_sections(things: &[Thing]) -> Vec<Section> {
    let mut out = Vec::new();
    let mut makes = Section::new("What it makes (factory gauges)");
    let mut made: BTreeMap<String, (i64, bool)> = BTreeMap::new();
    for t in things {
        if let Detail::Gauge(slots) = &t.detail {
            for s in slots {
                let e = made.entry(s.item.clone()).or_insert((0, true));
                e.0 += s.target;
                e.1 &= s.satisfied;
            }
        }
    }
    for (item, (n, ok)) in &made {
        let v = if *ok { format!("{} kept in stock", fmt_int(*n)) } else { format!("{} wanted, short", fmt_int(*n)) };
        makes = makes.field(&humanize(item), v);
    }
    if !makes.fields.is_empty() {
        out.push(makes);
    }
    // Per kind: how many, how many idle (stopped, or an unlit furnace), how many overstressed, and
    // the word for idle.
    let mut machines: BTreeMap<String, (usize, usize, usize, &str)> = BTreeMap::new();
    for t in things.iter().filter(|t| matches!(t.kind, Kind::Machine | Kind::Mover)) {
        let word = if t.net.is_none() && t.speed == 0.0 && t.active.is_some() && !t.id.contains("motor") {
            if t.id.contains("furnace") || t.id.contains("smoker") { "unlit" } else { "stopped" }
        } else {
            "stopped"
        };
        let e = machines.entry(t.name()).or_insert((0, 0, 0, word));
        e.0 += 1;
        if t.net.is_some_and(|n| n.overstressed()) {
            e.2 += 1;
        } else if t.idle() {
            e.1 += 1;
        }
    }
    if !machines.is_empty() {
        let mut sec = Section::new("Machines");
        let mut v: Vec<_> = machines.into_iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1.0));
        for (name, (n, idle, over, word)) in v.into_iter().take(20) {
            let mut s = format!("{n}");
            if over > 0 {
                s.push_str(&format!(", {over} overstressed"));
            }
            if idle > 0 {
                s.push_str(&format!(", {idle} {word}"));
            }
            sec = sec.field(&name, s);
        }
        out.push(sec);
    }
    let mut nets: BTreeMap<i64, Net> = BTreeMap::new();
    let mut power: BTreeMap<String, usize> = BTreeMap::new();
    for t in things {
        if let Some(n) = t.net {
            nets.entry(n.id).or_insert(n);
        }
        if t.kind == Kind::Power {
            *power.entry(t.name()).or_default() += 1;
        }
    }
    if !nets.is_empty() || !power.is_empty() {
        let mut sec = Section::new("Power and stress");
        for (name, n) in &power {
            sec = sec.field(name, *n);
        }
        // One line for the stress, not one a network: what is used of what is given, and how many
        // networks stand still for want of it.
        let v: Vec<_> = nets.values().filter(|n| n.capacity > 0.0 || n.stress > 0.0).collect();
        if !v.is_empty() {
            let (used, given) = v.iter().fold((0.0, 0.0), |(u, g), n| (u + n.stress, g + n.capacity));
            let nets = if v.len() == 1 { "1 network".to_owned() } else { format!("{} networks", v.len()) };
            sec = sec.field(
                "Stress used",
                format!("{} of {} su, {nets}", fmt_int(used.round() as i64), fmt_int(given.round() as i64)),
            );
            let over = v.iter().filter(|n| n.overstressed()).count();
            if over > 0 {
                sec = sec.field("Overstressed, standing still", format!("{over} of {}", v.len()));
            }
        }
        if !sec.fields.is_empty() {
            out.push(sec);
        }
    }
    let mut items: BTreeMap<String, i64> = BTreeMap::new();
    let mut fluids: BTreeMap<String, i64> = BTreeMap::new();
    let (mut vaults, mut tanks, mut containers) = (0, 0, 0);
    for t in things {
        match &t.detail {
            Detail::Store { controller: true, items: i, fluids: f, .. } => {
                if t.id.contains("tank") {
                    tanks += 1;
                } else {
                    vaults += 1;
                }
                for (k, n) in i {
                    *items.entry(k.clone()).or_default() += n;
                }
                for (k, n) in f {
                    *fluids.entry(k.clone()).or_default() += n;
                }
            }
            _ if t.kind == Kind::Container => containers += 1,
            _ => {}
        }
    }
    if vaults + tanks + containers > 0 {
        let mut sec = Section::new("Storage");
        if vaults > 0 {
            sec = sec.field("Item vaults", vaults);
        }
        if tanks > 0 {
            sec = sec.field("Fluid tanks", tanks);
        }
        if containers > 0 {
            sec = sec.field("Chests, barrels and the like", containers);
        }
        let mut v: Vec<_> = items.into_iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        for (id, n) in v.into_iter().take(10) {
            sec = sec.field_unit(&humanize(&id), n, "items");
        }
        for (id, n) in fluids {
            sec = sec.field(&humanize(&id), fluid_text(n));
        }
        out.push(sec);
    }
    let belts: Vec<_> = things.iter().filter(|t| t.kind == Kind::Belt).collect();
    let chains: Vec<_> = things.iter().filter(|t| t.kind == Kind::Chain).collect();
    let mut addrs: Vec<String> = Vec::new();
    let mut packages = 0;
    let mut chain_len = 0.0;
    for t in things {
        match &t.detail {
            Detail::Frogport { address, .. } | Detail::Requester { address, .. } | Detail::Address(address) => {
                if !address.is_empty() && !addrs.contains(address) {
                    addrs.push(address.clone());
                }
            }
            Detail::Chain { links, travelling, looping, .. } => {
                packages += travelling.iter().map(|x| x.1).sum::<u32>() + looping;
                for q in links {
                    chain_len += (((q[0] - t.pos[0]).pow(2) + (q[2] - t.pos[2]).pow(2)) as f64).sqrt() / 2.0;
                }
            }
            _ => {}
        }
    }
    if !belts.is_empty() || !chains.is_empty() || !addrs.is_empty() {
        let mut sec = Section::new("Logistics");
        if !belts.is_empty() {
            let running = belts.iter().filter(|t| t.running()).count();
            sec = sec.field("Belt blocks", format!("{}, {} running", belts.len(), running));
        }
        if !chains.is_empty() {
            sec = sec.field("Chain conveyor pulleys", chains.len());
            sec = sec.field_unit("Chain length", chain_len.round(), "blocks");
            sec = sec.field_unit("Packages on chains", packages, "packages");
        }
        if !addrs.is_empty() {
            addrs.sort();
            sec = sec.field("Package addresses", addrs.join(", "));
        }
        out.push(sec);
    }
    out
}

/// Stations and trains whose names hold `q`, best first.
pub fn find(rail: &Rail, frame_of: &dyn Fn(i32) -> String, q: &str, limit: usize) -> Vec<(u8, Found)> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let score = |name: &str| -> Option<u8> {
        let n = name.to_lowercase();
        if n == q {
            Some(0)
        } else if n.starts_with(&q) {
            Some(1)
        } else if n.contains(&q) {
            Some(2)
        } else {
            None
        }
    };
    let mut out = Vec::new();
    for s in &rail.stations {
        if let Some(sc) = score(&s.name) {
            out.push((
                sc,
                Found {
                    layer: STATIONS.into(),
                    id: uuid_text(&s.id),
                    label: s.name.clone(),
                    kind: "Station".into(),
                    frame: frame_of(rail.graphs[s.place.graph].dim),
                    x: s.place.at[0],
                    z: s.place.at[2],
                },
            ));
        }
    }
    for t in &rail.trains {
        let Some(f) = t.front() else { continue };
        if let Some(sc) = score(&t.name) {
            out.push((
                sc,
                Found {
                    layer: TRAINS.into(),
                    id: uuid_text(&t.id),
                    label: t.name.clone(),
                    kind: "Train".into(),
                    frame: frame_of(t.dim),
                    x: f[0],
                    z: f[1],
                },
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.cmp(&b.1.label)));
    out.truncate(limit);
    out
}

/// Things read so far in `dim` (frame `frame`) whose kind, address or gauge product matches `q`,
/// scored as [`find`] scores names plus 3, so the rail and the districts come first.
pub fn find_things<'a>(
    things: impl IntoIterator<Item = &'a Thing>,
    dim: &str,
    frame: &str,
    q: &str,
) -> Vec<(u8, Found)> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let score = |name: &str| -> Option<u8> {
        let n = name.to_lowercase();
        if n == q {
            Some(3)
        } else if n.starts_with(&q) {
            Some(4)
        } else if n.contains(&q) {
            Some(5)
        } else {
            None
        }
    };
    let mut out = Vec::new();
    for t in things {
        let layer = match t.kind {
            Kind::Machine | Kind::Power | Kind::Mover => MACHINES,
            Kind::Gauge => GAUGES,
            Kind::Storage => STORAGE,
            Kind::Port => PORTS,
            Kind::Chain => CHAINS,
            Kind::Belt => BELTS,
            Kind::Container | Kind::Other => continue,
        };
        let name = t.name();
        // What it is called for: an address, the products a gauge keeps in stock.
        let mut what: Vec<String> = match &t.detail {
            Detail::Address(a) | Detail::Frogport { address: a, .. } if !a.is_empty() => vec![a.clone()],
            Detail::Gauge(slots) => slots.iter().map(|s| humanize(&s.item)).collect(),
            _ => Vec::new(),
        };
        what.retain(|w| !w.is_empty());
        let best = what
            .iter()
            .filter_map(|w| score(w).map(|sc| (sc, format!("{w}: {}", name.to_lowercase()))))
            .chain(score(&name).map(|sc| (sc, name.clone())))
            .min_by(|a, b| a.0.cmp(&b.0));
        let Some((sc, label)) = best else { continue };
        let c = center(t.pos);
        out.push((
            sc,
            Found {
                layer: layer.into(),
                id: block_id(t.pos, dim),
                label: format!("{label}, at {}, {}", t.pos[0], t.pos[2]),
                kind: name,
                frame: frame.to_owned(),
                x: c[0],
                z: c[1],
            },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_filters_match_like_create_s() {
        assert!(matches_station("Center 5", "Center 5"));
        assert!(!matches_station("Center", "Center 5"));
        assert!(matches_station("Center*", "Center 5"));
        assert!(matches_station("*5", "Center 5"));
        assert!(matches_station("C*r*5", "Center 5"));
        assert!(!matches_station("X*", "Center 5"));
    }

    #[test]
    fn block_ids_round_trip() {
        assert_eq!(
            parse_block_id(&block_id([1, -2, 3], "minecraft:overworld")),
            Some(([1, -2, 3], "minecraft:overworld".into()))
        );
        assert_eq!(
            parse_block_id(&block_id([1, 2, 3], "minecraft:the_nether")),
            Some(([1, 2, 3], "minecraft:the_nether".into()))
        );
        assert_eq!(parse_block_id("x"), None);
    }

    #[test]
    fn plurals_read_naturally() {
        assert_eq!(plural(1, "Mechanical press"), "Mechanical press");
        assert_eq!(plural(3, "Mechanical press"), "3 mechanical presses");
        assert_eq!(plural(9, "Mechanical crafter"), "9 mechanical crafters");
        assert_eq!(plural(2, "Crushing wheels"), "2 crushing wheels");
    }

    #[test]
    fn every_layer_has_a_legend_and_a_known_id() {
        for d in layer_descs() {
            assert!(!d.legend.is_empty(), "{}", d.id);
            assert!(RAIL_LAYERS.contains(&d.id.as_str()) || BLOCK_LAYERS.contains(&d.id.as_str()));
        }
    }
}
