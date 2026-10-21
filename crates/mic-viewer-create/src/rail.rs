// SPDX-License-Identifier: MPL-2.0
//! Create 6's rail network as `data/create_tracks.dat` holds it (read from Create 6.0.10's
//! `GlobalRailwayManager`, `TrackGraph`, `TrackEdge`, `BezierConnection`, `Train` and `Carriage`
//! and checked against saved worlds):
//!
//! - `data.DimensionPalette`: `[{Id}]`; a location's `D` indexes it.
//! - `data.RailGraphs[]`: `{Id, Color, Nodes, Points}`. A node is `{Location: {Pos, D, YO?},
//!   Connections: [{To, EdgeData}]}`; `Pos` is the location at twice the block scale and `YO` a
//!   height offset in sixteenths, so the node is at `Pos / 2` (plus `YO / 16` on y). Every
//!   connection is saved from both ends. A turn's edge data has `Starts` and `Axes` (two
//!   `{V: [x, y, z]}` each, in world blocks): the curve is Create's cubic bezier through them
//!   ([`Bezier::new`]).
//! - `Points`: `create:station` `{Id, Name, Edge: [loc, loc], Position, BlockEntityPos, Ports,
//!   Assembling}`, `create:signal` `{Edge, Position, Type1, Type2, State1, State2}` and
//!   `create:observer` `{Edge, Position, BlockEntityPos, Activated, Filter}`; `Position` is the
//!   distance from `Edge[0]` along the edge towards `Edge[1]`.
//! - `data.Trains[]`: `{Id, Name (a text component), Graph, Carriages, Speed (blocks a tick, signed),
//!   Derailed, Station?, Navigation, Runtime: {State, CurrentEntry, Paused, Schedule}, DoubleEnded}`.
//!   A carriage's `FirstBogey` and `SecondBogey?` each have two `Points` (leading, trailing):
//!   `{Nodes: [loc, loc], Position}`, a distance along that directed edge. The train faces the way
//!   its points' edges run and moves that way when `Speed` is positive. A carriage's cargo is in
//!   `EntityPositioning[].items[].storage` and `.fluids[].storage`.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use mic_viewer_surveyor::nbt::{self, Tag};

use crate::tags::{self, Uuid, f, flag, i, s};

/// A file bigger than this once inflated is refused (a city-sized network inflates to about 12 MB).
pub const MAX_INFLATED: u64 = 512 << 20;

pub type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn len(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-4 { [0.0; 3] } else { scale(a, 1.0 / l) }
}
fn lerp(t: f64, a: V3, b: V3) -> V3 {
    add(a, scale(sub(b, a), t))
}
/// Minecraft's `Mth.equal`.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1e-5
}

/// A track node's location: Create's doubled coordinates and the dimension's palette index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Loc {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub d: i32,
}

impl Loc {
    fn read(t: &Tag) -> Option<(Loc, i32)> {
        let p = tags::pos3(t.get("Pos")?)?;
        let d = i(t, "D").unwrap_or(0) as i32;
        let yo = i(t, "YO").unwrap_or(0) as i32;
        Some((Loc { x: p[0], y: p[1], z: p[2], d }, yo))
    }

    /// The location in world blocks (`TrackNodeLocation.getLocation`).
    pub fn world(&self, yo: i32) -> V3 {
        [f64::from(self.x) / 2.0, f64::from(self.y) / 2.0 + f64::from(yo) / 16.0, f64::from(self.z) / 2.0]
    }
}

/// A turn: Create's `BezierConnection` curve, from `p1` (its first start) to `p2`, with control
/// points `q1` and `q2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezier {
    pub p1: V3,
    pub p2: V3,
    pub q1: V3,
    pub q2: V3,
    /// Create's own length: 16 chords.
    pub length: f64,
}

impl Bezier {
    /// The curve Create builds from a turn's starts and axes (`BezierConnection.Runtime`).
    pub fn new(starts: [V3; 2], axes: [V3; 2]) -> Bezier {
        let (end1, end2) = (starts[0], starts[1]);
        let (axis1, axis2) = (norm(axes[0]), norm(axes[1]));
        let h = handle_length(end1, end2, axis1, axis2);
        let q1 = add(scale(axis1, h), end1);
        let q2 = add(scale(axis2, h), end2);
        let mut b = Bezier { p1: end1, p2: end2, q1, q2, length: 0.0 };
        let mut prev = end1;
        let mut l = 0.0;
        for k in 0..=16 {
            let p = b.at(f64::from(k) / 16.0);
            l += len(sub(p, prev));
            prev = p;
        }
        b.length = l;
        b
    }

    /// The point at `t` (`VecHelper.bezier`).
    pub fn at(&self, t: f64) -> V3 {
        let v1 = lerp(t, self.p1, self.q1);
        let v2 = lerp(t, self.q1, self.q2);
        let v3 = lerp(t, self.q2, self.p2);
        lerp(t, lerp(t, v1, v2), lerp(t, v2, v3))
    }
}

/// `VecHelper.intersect` on the horizontal plane: `t` and `u` with `p1 + t r = p2 + u s`.
fn intersect(p1: V3, p2: V3, r: V3, s: V3) -> Option<[f64; 2]> {
    let q = sub(p2, p1);
    let rcs = r[0] * s[2] - r[2] * s[0];
    if equal(rcs, 0.0) {
        return None;
    }
    let (rd, sd) = (scale(r, 1.0 / rcs), scale(s, 1.0 / rcs));
    Some([q[0] * sd[2] - q[2] * sd[0], q[0] * rd[2] - q[2] * rd[0]])
}

/// `BezierConnection.Runtime.determineHandles`: how far the control points sit from the ends.
fn handle_length(end1: V3, end2: V3, axis1: V3, axis2: V3) -> f64 {
    // axis × (0, 1, 0)
    let cross = |a: V3| [-a[2], 0.0, a[0]];
    let (cross1, cross2) = (cross(axis1), cross(axis2));
    let a1 = (-axis2[2]).atan2(-axis2[0]);
    let a2 = axis1[2].atan2(axis1[0]);
    let circle = std::f64::consts::TAU as f32 as f64;
    let mut angle = (a1 - a2 + circle) % circle;
    if (circle - angle).abs() < angle.abs() {
        angle = circle - angle;
    }
    let dist = len(sub(end2, end1));
    if equal(angle, 0.0) {
        if let Some(x) = intersect(end1, end2, axis1, cross2) {
            let (t, u) = (x[0].abs(), x[1].abs());
            let (min, max) = (t.min(u), t.max(u));
            if min > 1.2 && max / min > 1.0 && max / min < 3.0 {
                return max - min;
            }
        }
        return dist / 3.0;
    }
    let n = circle / angle;
    let factor = 4.0 / 3.0 * (std::f64::consts::PI / (2.0 * n)).tan();
    match intersect(end1, end2, cross1, cross2) {
        None => dist / 3.0,
        Some(x) => {
            let h = x[1].abs() * factor;
            if equal(h, 0.0) { 1.0 } else { h }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub loc: Loc,
    /// World position.
    pub p: V3,
}

/// One stretch of track between two nodes (Create saves it from both ends; this is one of them).
#[derive(Clone, Debug)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    /// The curve from `a` to `b`, for a turn.
    pub turn: Option<Bezier>,
    pub len: f64,
    pub material: String,
}

impl Edge {
    /// The point at fraction `t` from `a` (`TrackEdge.getPosition`).
    pub fn at(&self, nodes: &[Node], t: f64) -> V3 {
        let t = t.clamp(0.0, 1.0);
        match &self.turn {
            Some(b) => b.at(t),
            None => lerp(t, nodes[self.a as usize].p, nodes[self.b as usize].p),
        }
    }

    /// The edge as a line from `a` to `b` on the map (x, z): a turn one sample a block or so.
    pub fn line(&self, nodes: &[Node]) -> Vec<[f64; 2]> {
        match &self.turn {
            None => {
                let (a, b) = (nodes[self.a as usize].p, nodes[self.b as usize].p);
                vec![[a[0], a[2]], [b[0], b[2]]]
            }
            Some(bz) => {
                let n = (bz.length.ceil() as usize).clamp(4, 128);
                (0..=n)
                    .map(|k| {
                        let p = bz.at(k as f64 / n as f64);
                        [p[0], p[2]]
                    })
                    .collect()
            }
        }
    }

    /// The direction leaving `node` along this edge, on the map.
    fn leaving(&self, nodes: &[Node], node: u32) -> [f64; 2] {
        let (t0, t1) = if node == self.a { (0.0, 0.1) } else { (1.0, 0.9) };
        let (p, q) = (self.at(nodes, t0), self.at(nodes, t1));
        let d = [q[0] - p[0], q[2] - p[2]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-9);
        [d[0] / l, d[1] / l]
    }
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub id: Uuid,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// The dimension (palette index) most of its nodes are in.
    pub dim: i32,
    /// Track length in blocks.
    pub length: f64,
}

impl Graph {
    /// The network as strokes: edges joined end to end, through a junction along the straightest
    /// way on, so a line is a few long paths rather than an edge each. Every edge is in exactly one
    /// stroke. Edges between dimensions are left out.
    pub fn strokes(&self) -> Vec<Vec<[f64; 2]>> {
        let mut at: HashMap<u32, Vec<u32>> = HashMap::new();
        for (k, e) in self.edges.iter().enumerate() {
            if self.nodes[e.a as usize].loc.d != self.nodes[e.b as usize].loc.d {
                continue;
            }
            at.entry(e.a).or_default().push(k as u32);
            at.entry(e.b).or_default().push(k as u32);
        }
        let mut used = vec![false; self.edges.len()];
        let mut out = Vec::new();
        for start in 0..self.edges.len() {
            if used[start] || !at.get(&self.edges[start].a).is_some_and(|v| v.contains(&(start as u32))) {
                continue;
            }
            used[start] = true;
            let e = &self.edges[start];
            let mut line = e.line(&self.nodes);
            // Forward from b, then backward from a.
            for forward in [true, false] {
                let (mut node, mut came) = if forward { (e.b, start as u32) } else { (e.a, start as u32) };
                loop {
                    let ce = &self.edges[came as usize];
                    let arriving = {
                        let l = ce.leaving(&self.nodes, node);
                        [-l[0], -l[1]]
                    };
                    let Some(next) = at.get(&node).and_then(|v| {
                        v.iter()
                            .copied()
                            .filter(|&k| !used[k as usize])
                            .map(|k| {
                                let d = self.edges[k as usize].leaving(&self.nodes, node);
                                (k, d[0] * arriving[0] + d[1] * arriving[1])
                            })
                            .filter(|&(_, dot)| dot > 0.5)
                            .max_by(|x, y| x.1.total_cmp(&y.1))
                            .map(|(k, _)| k)
                    }) else {
                        break;
                    };
                    used[next as usize] = true;
                    let ne = &self.edges[next as usize];
                    let mut piece = ne.line(&self.nodes);
                    let other = if ne.a == node {
                        ne.b
                    } else {
                        piece.reverse();
                        ne.a
                    };
                    // `piece` now runs from `node` to `other`.
                    if forward {
                        line.extend(piece.into_iter().skip(1));
                    } else {
                        piece.reverse();
                        piece.pop();
                        piece.extend(line);
                        line = piece;
                    }
                    node = other;
                    came = next;
                }
            }
            out.push(line);
        }
        out
    }
}

/// A point on the track (a station, signal or observer), placed.
#[derive(Clone, Debug)]
pub struct Placed {
    pub graph: usize,
    /// The edge it is on (an index into the graph's `edges`) and how far along it from its `a`
    /// node (0 to 1).
    pub edge: u32,
    pub t: f64,
    /// World position.
    pub at: V3,
    /// The heading along the edge (`Edge[0]` to `Edge[1]`), radians, 0 east, clockwise.
    pub heading: f64,
}

#[derive(Clone, Debug)]
pub struct Station {
    pub id: Uuid,
    pub name: String,
    pub place: Placed,
    pub block: Option<[i32; 3]>,
    /// Package port addresses (Create 6's train postboxes).
    pub ports: Vec<String>,
    pub assembling: bool,
}

#[derive(Clone, Debug)]
pub struct Signal {
    pub place: Placed,
    /// `ENTRY_SIGNAL` or `CROSS_SIGNAL` for each side present.
    pub types: Vec<String>,
    /// `RED`, `YELLOW`, `GREEN` or `INVALID` for each side present.
    pub states: Vec<String>,
}

impl Signal {
    /// The most restrictive of its sides' states, ignoring `INVALID` (a side with no signal).
    pub fn state(&self) -> &str {
        let rank = |s: &str| match s {
            "RED" => 3,
            "YELLOW" => 2,
            "GREEN" => 1,
            _ => 0,
        };
        self.states.iter().map(String::as_str).max_by_key(|s| rank(s)).filter(|s| rank(s) > 0).unwrap_or("INVALID")
    }
}

#[derive(Clone, Debug)]
pub struct Observer {
    pub place: Placed,
    pub block: Option<[i32; 3]>,
    pub activated: bool,
    pub filter: Option<String>,
}

/// One schedule entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    /// The instruction (`destination`, `throttle`, `rename`, `deliver_packages`, ...).
    pub kind: String,
    /// What it says: a station name (`*` matches anything), a throttle percentage, a name.
    pub text: String,
    /// When it moves on, as text ("20 s").
    pub until: Vec<String>,
}

impl Stop {
    /// The entry as a sentence: "Go to Center 5", "Throttle 35%".
    pub fn describe(&self) -> String {
        match self.kind.as_str() {
            "destination" => format!("Go to {}", self.text),
            "throttle" => format!("Throttle {}%", self.text),
            "rename" => format!("Rename to {}", self.text),
            "deliver_packages" => "Deliver packages".into(),
            "fetch_packages" if self.text.is_empty() => "Fetch packages".into(),
            "fetch_packages" => format!("Fetch packages for {}", self.text),
            k => {
                let h = tags::humanize(k);
                if self.text.is_empty() { h } else { format!("{h} {}", self.text) }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Train {
    pub id: Uuid,
    pub name: String,
    pub graph: Option<usize>,
    /// Each carriage from its front bogey to its back one, `[x0, z0, x1, z1]`, the carriage that
    /// leads the way it runs first (a train moving backwards is turned round).
    pub cars: Vec<[f64; 4]>,
    /// Blocks a tick; negative when it runs backwards.
    pub speed: f64,
    pub derailed: bool,
    /// The station it stands at.
    pub at_station: Option<Uuid>,
    /// The schedule runtime's state: `PRE_TRANSIT`, `IN_TRANSIT`, `POST_TRANSIT`.
    pub state: String,
    pub paused: bool,
    pub completed: bool,
    pub cyclic: bool,
    pub schedule: Vec<Stop>,
    pub current: usize,
    /// The station it is heading for and the blocks left to it.
    pub destination: Option<Uuid>,
    pub to_go: Option<f64>,
    pub items: BTreeMap<String, i64>,
    pub fluids: BTreeMap<String, i64>,
    pub double_ended: bool,
    pub dim: i32,
}

impl Train {
    /// Where its front is (x, z).
    pub fn front(&self) -> Option<[f64; 2]> {
        self.cars.first().map(|c| [c[0], c[1]])
    }

    /// Blocks from the first bogey to the last.
    /// From the leading carriage's front to the last one's back, along the carriages and the gaps
    /// between them.
    pub fn length(&self) -> f64 {
        let d = |a: [f64; 2], b: [f64; 2]| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let cars: f64 = self.cars.iter().map(|c| d([c[0], c[1]], [c[2], c[3]])).sum();
        let gaps: f64 = self.cars.windows(2).map(|w| d([w[0][2], w[0][3]], [w[1][0], w[1][1]])).sum();
        cars + gaps
    }

    pub fn moving(&self) -> bool {
        self.speed.abs() > 1e-4
    }
}

/// A carriage on one bogey is saved as that bogey's two wheel points, two blocks apart, however long
/// its body. When every carriage rides on one bogey, stretch each to its share of the train:
/// consecutive bogeys stand `CarriageSpacing` apart, and each gap is split between the two
/// carriages with a block of coupling between them. The end carriages reach as far outward as
/// inward, no further than their contraption's extent.
fn stretch_single_bogey(cars: &mut [[f64; 4]], single: &[bool], extent: &[Option<f64>], pitch: &[f64]) {
    let n = cars.len();
    if n == 0 || single.iter().any(|s| !s) || pitch.len() + 1 != n {
        return;
    }
    let half = |k: usize| (pitch[k] - 1.0).max(2.0) / 2.0;
    for k in 0..n {
        // Toward the carriage ahead (the first point's side), and toward the one behind.
        let ahead = if k > 0 { Some(half(k - 1)) } else { None };
        let behind = if k + 1 < n { Some(half(k)) } else { None };
        let cap = extent[k].map_or(f64::INFINITY, |e| e / 2.0);
        let (ahead, behind) = match (ahead, behind) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, a.min(cap)),
            (None, Some(b)) => (b.min(cap), b),
            (None, None) => continue,
        };
        let c = cars[k];
        let (dx, dz) = (c[0] - c[2], c[1] - c[3]);
        let d = (dx * dx + dz * dz).sqrt();
        if d < 1e-6 {
            continue;
        }
        let (ux, uz) = (dx / d, dz / d);
        let (mx, mz) = ((c[0] + c[2]) / 2.0, (c[1] + c[3]) / 2.0);
        cars[k] = [mx + ux * ahead, mz + uz * ahead, mx - ux * behind, mz - uz * behind];
    }
}

/// Everything in one `create_tracks.dat`.
#[derive(Clone, Debug, Default)]
pub struct Rail {
    /// Dimension ids by palette index.
    pub dims: Vec<String>,
    pub graphs: Vec<Graph>,
    pub stations: Vec<Station>,
    pub signals: Vec<Signal>,
    pub observers: Vec<Observer>,
    pub trains: Vec<Train>,
    /// A directed node pair to (graph, edge, whether it runs against the edge).
    edge_at: HashMap<(Loc, Loc), (u32, u32, bool)>,
    graph_of: HashMap<Uuid, usize>,
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

impl Rail {
    /// Reads a world's `data/create_tracks.dat`; `Ok(None)` when it has none. The file is read
    /// whole into memory and closed before it is parsed.
    pub fn open(world: &Path) -> io::Result<Option<Rail>> {
        let path = world.join("data").join("create_tracks.dat");
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        Rail::from_bytes(&bytes).map(Some)
    }

    /// Parses the file's bytes (gzip NBT).
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Rail> {
        let root = nbt::read_gzip(bytes, MAX_INFLATED)?;
        Rail::from_tag(&root)
    }

    pub fn from_tag(root: &Tag) -> io::Result<Rail> {
        let data = root.get("data").ok_or_else(|| invalid("no data compound"))?;
        let mut rail = Rail {
            dims: data
                .get("DimensionPalette")
                .map(|p| p.items().iter().map(|d| s(d, "Id").unwrap_or("?").to_owned()).collect())
                .unwrap_or_default(),
            ..Rail::default()
        };
        for g in data.get("RailGraphs").map(Tag::items).unwrap_or_default() {
            rail.read_graph(g);
        }
        for g in data.get("RailGraphs").map(Tag::items).unwrap_or_default() {
            let Some(points) = g.get("Points") else { continue };
            let Some(&gi) = g.get("Id").and_then(tags::uuid).and_then(|u| rail.graph_of.get(&u)) else { continue };
            for p in points.get("create:station").map(Tag::items).unwrap_or_default() {
                let Some(place) = rail.place(p, gi) else { continue };
                rail.stations.push(Station {
                    id: p.get("Id").and_then(tags::uuid).unwrap_or_default(),
                    name: s(p, "Name").unwrap_or("Station").to_owned(),
                    place,
                    block: p.get("BlockEntityPos").and_then(tags::pos3),
                    ports: p
                        .get("Ports")
                        .map(Tag::items)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|x| s(x, "Address").filter(|a| !a.is_empty()).map(str::to_owned))
                        .collect(),
                    assembling: flag(p, "Assembling"),
                });
            }
            for p in points.get("create:signal").map(Tag::items).unwrap_or_default() {
                let Some(place) = rail.place(p, gi) else { continue };
                let mut types = Vec::new();
                let mut states = Vec::new();
                for k in ["1", "2"] {
                    if let Some(t) = s(p, &format!("Type{k}")) {
                        types.push(t.to_owned());
                        states.push(s(p, &format!("State{k}")).unwrap_or("INVALID").to_owned());
                    }
                }
                rail.signals.push(Signal { place, types, states });
            }
            for p in points.get("create:observer").map(Tag::items).unwrap_or_default() {
                let Some(place) = rail.place(p, gi) else { continue };
                rail.observers.push(Observer {
                    place,
                    block: p.get("BlockEntityPos").and_then(tags::pos3),
                    activated: flag(p, "Activated"),
                    filter: p.get("Filter").and_then(|x| s(x, "id")).map(str::to_owned),
                });
            }
        }
        for t in data.get("Trains").map(Tag::items).unwrap_or_default() {
            if let Some(tr) = rail.read_train(t) {
                rail.trains.push(tr);
            }
        }
        Ok(rail)
    }

    fn read_graph(&mut self, g: &Tag) {
        let id = g.get("Id").and_then(tags::uuid).unwrap_or_default();
        let gi = self.graphs.len();
        let mut nodes = Vec::new();
        for n in g.get("Nodes").map(Tag::items).unwrap_or_default() {
            let Some((loc, yo)) = n.get("Location").and_then(Loc::read) else {
                // Keep the indices `To` refers to.
                nodes.push(Node { loc: Loc { x: 0, y: 0, z: 0, d: -1 }, p: [0.0; 3] });
                continue;
            };
            nodes.push(Node { loc, p: loc.world(yo) });
        }
        let mut edges: Vec<Edge> = Vec::new();
        let mut seen: HashMap<(u32, u32), u32> = HashMap::new();
        let mut dims: HashMap<i32, usize> = HashMap::new();
        for (a, n) in g.get("Nodes").map(Tag::items).unwrap_or_default().iter().enumerate() {
            let a = a as u32;
            *dims.entry(nodes[a as usize].loc.d).or_default() += 1;
            for c in n.get("Connections").map(Tag::items).unwrap_or_default() {
                let Some(b) = i(c, "To").and_then(|v| u32::try_from(v).ok()) else { continue };
                if b as usize >= nodes.len() || b == a {
                    continue;
                }
                let key = (a.min(b), a.max(b));
                if let Some(&k) = seen.get(&key) {
                    let (la, lb) = (nodes[a as usize].loc, nodes[b as usize].loc);
                    let e = &edges[k as usize];
                    self.edge_at.insert((la, lb), (gi as u32, k, e.a != a));
                    continue;
                }
                let ed = c.get("EdgeData");
                let turn = ed.and_then(|ed| {
                    let st = ed.get("Starts")?.items();
                    let ax = ed.get("Axes")?.items();
                    if st.len() != 2 || ax.len() != 2 {
                        return None;
                    }
                    Some(Bezier::new(
                        [tags::vec3(&st[0])?, tags::vec3(&st[1])?],
                        [tags::vec3(&ax[0])?, tags::vec3(&ax[1])?],
                    ))
                });
                let (pa, pb) = (nodes[a as usize].p, nodes[b as usize].p);
                let inter = nodes[a as usize].loc.d != nodes[b as usize].loc.d;
                let len = if inter { 0.0 } else { turn.as_ref().map_or_else(|| self::len(sub(pb, pa)), |t| t.length) };
                let material = ed.and_then(|ed| s(ed, "Material")).unwrap_or("create:andesite").to_owned();
                let k = edges.len() as u32;
                edges.push(Edge { a, b, turn, len, material });
                seen.insert(key, k);
                let (la, lb) = (nodes[a as usize].loc, nodes[b as usize].loc);
                self.edge_at.insert((la, lb), (gi as u32, k, false));
                self.edge_at.insert((lb, la), (gi as u32, k, true));
            }
        }
        let length = edges.iter().map(|e| e.len).sum();
        let dim = dims.into_iter().max_by_key(|&(d, n)| (n, -d)).map_or(0, |(d, _)| d);
        self.graph_of.insert(id, gi);
        self.graphs.push(Graph { id, nodes, edges, dim, length });
    }

    /// The point `dist` blocks from `from` along the edge to `to`, and the direction there (x, z).
    pub fn point_on(&self, from: Loc, to: Loc, dist: f64) -> Option<(usize, V3, [f64; 2])> {
        let &(g, e, rev) = self.edge_at.get(&(from, to))?;
        let graph = &self.graphs[g as usize];
        let edge = &graph.edges[e as usize];
        let l = if edge.len > 0.0 { edge.len } else { 1.0 };
        let mut t = (dist / l).clamp(0.0, 1.0);
        if rev {
            t = 1.0 - t;
        }
        let p = edge.at(&graph.nodes, t);
        let dt = (0.5 / l).min(0.25);
        let (p0, p1) = (edge.at(&graph.nodes, t - dt), edge.at(&graph.nodes, t + dt));
        let mut d = [p1[0] - p0[0], p1[2] - p0[2]];
        if rev {
            d = [-d[0], -d[1]];
        }
        let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-9);
        Some((g as usize, p, [d[0] / n, d[1] / n]))
    }

    /// A graph point's place from its `Edge` and `Position`.
    fn place(&self, p: &Tag, graph: usize) -> Option<Placed> {
        let e = p.get("Edge")?.items();
        if e.len() != 2 {
            return None;
        }
        let (a, _) = Loc::read(&e[0])?;
        let (b, _) = Loc::read(&e[1])?;
        let dist = f(p, "Position").unwrap_or(0.0);
        let (g, at, d) = self.point_on(a, b, dist)?;
        let _ = graph;
        let &(_, e, rev) = self.edge_at.get(&(a, b))?;
        let l = self.graphs[g].edges[e as usize].len;
        let t = (dist / if l > 0.0 { l } else { 1.0 }).clamp(0.0, 1.0);
        Some(Placed { graph: g, at, heading: d[1].atan2(d[0]), edge: e, t: if rev { 1.0 - t } else { t } })
    }

    /// The way along the track from `from` to `to` (both on one network) as a line on the map,
    /// the shortest a train could run: through a junction only the way the track goes on (no
    /// turn sharper than a right angle), either way out of `from`. `None` when there is none.
    pub fn route(&self, from: &Placed, to: &Placed) -> Option<Vec<[f64; 2]>> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        if from.graph != to.graph {
            return None;
        }
        let g = &self.graphs[from.graph];
        let edge = |k: u32| &g.edges[k as usize];
        // Part of edge `k` from fraction `t0` to `t1` (either way), as a line.
        let part = |k: u32, t0: f64, t1: f64| -> Vec<[f64; 2]> {
            let e = edge(k);
            let steps = if e.turn.is_some() { ((e.len * (t1 - t0).abs()).ceil() as usize).clamp(1, 128) } else { 1 };
            (0..=steps)
                .map(|i| {
                    let p = e.at(&g.nodes, t0 + (t1 - t0) * i as f64 / steps as f64);
                    [p[0], p[2]]
                })
                .collect()
        };
        if from.edge == to.edge {
            return Some(part(from.edge, from.t, to.t));
        }
        let mut at: HashMap<u32, Vec<u32>> = HashMap::new();
        for (k, e) in g.edges.iter().enumerate() {
            if g.nodes[e.a as usize].loc.d == g.nodes[e.b as usize].loc.d {
                at.entry(e.a).or_default().push(k as u32);
                at.entry(e.b).or_default().push(k as u32);
            }
        }
        let other = |k: u32, n: u32| if edge(k).a == n { edge(k).b } else { edge(k).a };
        // Blocks from fraction `t` of edge `k` to its end at node `n`.
        let to_end = |k: u32, t: f64, n: u32| edge(k).len * if edge(k).a == n { t } else { 1.0 - t };
        // States: at a node, having come along an edge. Costs are kept in milli-blocks.
        type State = (u32, u32);
        let mut best: HashMap<State, (u64, Option<State>)> = HashMap::new();
        let mut heap = BinaryHeap::new();
        let fe = edge(from.edge);
        for n in [fe.a, fe.b] {
            let c = (to_end(from.edge, from.t, n) * 1000.0) as u64;
            best.insert((n, from.edge), (c, None));
            heap.push(Reverse((c, n, from.edge)));
        }
        let mut done: Option<(u64, (u32, u32))> = None;
        while let Some(Reverse((c, n, came))) = heap.pop() {
            if best.get(&(n, came)).is_some_and(|b| b.0 < c) {
                continue;
            }
            if done.is_some_and(|(d, _)| d <= c) {
                break;
            }
            let arriving = {
                let l = edge(came).leaving(&g.nodes, n);
                [-l[0], -l[1]]
            };
            for &k in at.get(&n).map(Vec::as_slice).unwrap_or_default() {
                if k == came {
                    continue;
                }
                let d = edge(k).leaving(&g.nodes, n);
                if d[0] * arriving[0] + d[1] * arriving[1] <= 0.0 {
                    continue;
                }
                if k == to.edge {
                    let total = c + (to_end(k, to.t, n) * 1000.0) as u64;
                    if done.is_none_or(|(d, _)| total < d) {
                        done = Some((total, (n, came)));
                    }
                    continue;
                }
                let m = other(k, n);
                let nc = c + (edge(k).len * 1000.0) as u64;
                if best.get(&(m, k)).is_none_or(|b| nc < b.0) {
                    best.insert((m, k), (nc, Some((n, came))));
                    heap.push(Reverse((nc, m, k)));
                }
            }
        }
        let (_, last) = done?;
        // The states back to the start: each is a node reached along an edge.
        let mut chain = vec![last];
        while let Some(&(_, Some(prev))) = best.get(chain.last()?) {
            chain.push(prev);
        }
        chain.reverse();
        let (n0, _) = chain[0];
        let mut line = part(from.edge, from.t, if fe.a == n0 { 0.0 } else { 1.0 });
        for w in chain.windows(2) {
            let ((n, _), (_, k)) = (w[0], w[1]);
            let mut p = edge(k).line(&g.nodes);
            if edge(k).a != n {
                p.reverse();
            }
            line.extend(p.into_iter().skip(1));
        }
        let (n, _) = *chain.last()?;
        let te = edge(to.edge);
        line.extend(part(to.edge, if te.a == n { 0.0 } else { 1.0 }, to.t).into_iter().skip(1));
        Some(line)
    }

    /// A travelling point (`{Nodes: [loc, loc], Position}`) as a world position.
    fn travelling(&self, p: &Tag) -> Option<(usize, V3)> {
        let n = p.get("Nodes")?.items();
        if n.len() != 2 {
            return None;
        }
        let (a, _) = Loc::read(&n[0])?;
        let (b, _) = Loc::read(&n[1])?;
        self.point_on(a, b, f(p, "Position").unwrap_or(0.0)).map(|(g, at, _)| (g, at))
    }

    fn read_train(&self, t: &Tag) -> Option<Train> {
        let id = t.get("Id").and_then(tags::uuid)?;
        let name = s(t, "Name").map(tags::plain_text).unwrap_or_else(|| "Train".into());
        let mut graph = t.get("Graph").and_then(tags::uuid).and_then(|u| self.graph_of.get(&u).copied());
        let speed = f(t, "Speed").unwrap_or(0.0);
        let mut cars = Vec::new();
        let mut items = BTreeMap::new();
        let mut fluids = BTreeMap::new();
        let mut dim = None;
        // Per carriage: whether it rides on one bogey, and its contraption's extent along the track
        // where the save gives it.
        let mut single = Vec::new();
        let mut extent = Vec::new();
        for c in t.get("Carriages").map(Tag::items).unwrap_or_default() {
            let first = c.get("FirstBogey");
            single.push(c.get("SecondBogey").and_then(|b| b.get("Points")).is_none());
            let bounds = c
                .get("Entity")
                .and_then(|e| e.get("Contraption"))
                .and_then(|k| k.get("BoundsFront"))
                .map(Tag::items)
                .unwrap_or_default()
                .iter()
                .filter_map(tags::num)
                .collect::<Vec<_>>();
            extent.push((bounds.len() == 6).then(|| (bounds[3] - bounds[0]).min(bounds[5] - bounds[2])));
            let last = c.get("SecondBogey").filter(|b| b.get("Points").is_some()).or(first);
            let front =
                first.and_then(|b| b.get("Points")).and_then(|p| p.items().first()).and_then(|p| self.travelling(p));
            let back =
                last.and_then(|b| b.get("Points")).and_then(|p| p.items().get(1)).and_then(|p| self.travelling(p));
            let positioning = c.get("EntityPositioning").map(Tag::items).unwrap_or_default();
            match (front, back) {
                (Some((g, a)), Some((_, b))) => {
                    graph.get_or_insert(g);
                    cars.push([a[0], a[2], b[0], b[2]]);
                }
                _ => {
                    // The track moved under it: the carriage entity's own anchors.
                    let anchors: Vec<V3> = positioning
                        .iter()
                        .flat_map(|d| d.get("RotationAnchors").map(Tag::items).unwrap_or_default())
                        .filter_map(tags::vec3)
                        .collect();
                    if let (Some(a), Some(b)) = (anchors.first(), anchors.get(1)) {
                        cars.push([a[0], a[2], b[0], b[2]]);
                    }
                }
            }
            for d in positioning {
                if dim.is_none() {
                    dim = i(d, "Dim").map(|v| v as i32);
                }
                for k in ["items", "fluids"] {
                    for x in d.get(k).map(Tag::items).unwrap_or_default() {
                        if let Some(st) = x.get("storage") {
                            tags::stacks(st, &mut items, &mut fluids);
                        }
                    }
                }
            }
        }
        if cars.len() == single.len() {
            let pitch: Vec<f64> =
                t.get("CarriageSpacing").and_then(tags::ints).unwrap_or_default().into_iter().map(f64::from).collect();
            stretch_single_bogey(&mut cars, &single, &extent, &pitch);
        }
        if speed < 0.0 {
            cars.reverse();
            for c in &mut cars {
                *c = [c[2], c[3], c[0], c[1]];
            }
        }
        let rt = t.get("Runtime");
        let sched = rt.and_then(|r| r.get("Schedule"));
        let schedule =
            sched.and_then(|s| s.get("Entries")).map(Tag::items).unwrap_or_default().iter().map(read_stop).collect();
        let nav = t.get("Navigation");
        Some(Train {
            id,
            name,
            graph,
            cars,
            speed,
            derailed: flag(t, "Derailed"),
            at_station: t.get("Station").and_then(tags::uuid),
            state: rt.and_then(|r| s(r, "State")).unwrap_or("PRE_TRANSIT").to_owned(),
            paused: rt.is_some_and(|r| flag(r, "Paused")),
            completed: rt.is_some_and(|r| flag(r, "Completed")),
            cyclic: sched.is_some_and(|s| flag(s, "Cyclic")),
            schedule,
            current: rt.and_then(|r| i(r, "CurrentEntry")).unwrap_or(0).max(0) as usize,
            destination: nav.and_then(|n| n.get("Destination")).and_then(tags::uuid),
            to_go: nav.and_then(|n| f(n, "DistanceToDestination")),
            items,
            fluids,
            double_ended: flag(t, "DoubleEnded"),
            dim: dim.unwrap_or_else(|| graph.map_or(0, |g| self.graphs[g].dim)),
        })
    }

    pub fn graph_index(&self, id: &Uuid) -> Option<usize> {
        self.graph_of.get(id).copied()
    }

    pub fn station(&self, id: &Uuid) -> Option<&Station> {
        self.stations.iter().find(|s| &s.id == id)
    }

    /// The dimension palette index of `dim` (a dimension id such as `minecraft:overworld`).
    pub fn dim_index(&self, dim: &str) -> Option<i32> {
        self.dims.iter().position(|d| d == dim).map(|i| i as i32)
    }
}

fn read_stop(e: &Tag) -> Stop {
    let ins = e.get("Instruction");
    let kind = ins.and_then(|x| s(x, "Id")).unwrap_or("?");
    let kind = kind.rsplit(':').next().unwrap_or(kind).to_owned();
    let data = ins.and_then(|x| x.get("Data"));
    let text = data
        .and_then(|d| s(d, "Text").map(str::to_owned).or_else(|| i(d, "Value").map(|v| v.to_string())))
        .unwrap_or_default();
    let mut until = Vec::new();
    for alt in e.get("Conditions").map(Tag::items).unwrap_or_default() {
        let parts: Vec<String> = alt.items().iter().map(describe_condition).collect();
        if !parts.is_empty() {
            until.push(parts.join(" and "));
        }
    }
    Stop { kind, text, until }
}

fn describe_condition(c: &Tag) -> String {
    let id = s(c, "Id").unwrap_or("?");
    let id = id.rsplit(':').next().unwrap_or(id);
    let d = c.get("Data");
    let value = d.and_then(|d| i(d, "Value"));
    match id {
        "delay" | "idle" => {
            let unit = match d.and_then(|d| i(d, "TimeUnit")).unwrap_or(1) {
                0 => "ticks",
                2 => "min",
                _ => "s",
            };
            let what = if id == "delay" { "wait" } else { "idle" };
            format!("{what} {} {unit}", value.unwrap_or(0))
        }
        "time_of_day" => "time of day".into(),
        "powered" => "redstone".into(),
        "player_count" => format!("{} players", value.unwrap_or(0)),
        k => tags::humanize(k).to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        len(sub(a, b)) < 1e-6
    }

    #[test]
    fn single_bogey_carriages_stretch_to_their_bodies() {
        // A train of six carriages on one bogey each, bogeys 5, 7, 11, 7, 5 apart along z,
        // the train's front toward -z (each carriage's first point is its lower z).
        let starts = [35.5, 40.5, 47.5, 58.5, 65.5, 70.5];
        let mut cars: Vec<[f64; 4]> = starts.iter().map(|z| [24.5, *z, 24.5, z + 2.0]).collect();
        let extent = [Some(7.0), Some(5.0), Some(11.0), Some(5.0), Some(5.0), Some(3.0)];
        stretch_single_bogey(&mut cars, &[true; 6], &extent, &[5.0, 7.0, 11.0, 7.0, 5.0]);
        let lens: Vec<f64> = cars.iter().map(|c| c[3] - c[1]).collect();
        assert_eq!(lens, [4.0, 5.0, 8.0, 8.0, 5.0, 3.5]);
        // A block of coupling between each pair.
        for w in cars.windows(2) {
            assert!((w[1][1] - w[0][3] - 1.0).abs() < 1e-9, "{w:?}");
        }
        // A train with a carriage on two bogeys keeps its points.
        let mut two = vec![[0.0, 0.0, 0.0, 6.0], [0.0, 8.0, 0.0, 10.0]];
        let before = two.clone();
        stretch_single_bogey(&mut two, &[false, true], &[None, None], &[8.0]);
        assert_eq!(two, before);
    }

    #[test]
    fn a_quarter_turn_is_a_circle_arc() {
        // East from (0, 0) turning to south: ends at (10, 10), heading south. Create's axes point
        // from each end along the track, away from the other end's side.
        let b = Bezier::new([[0.0, 64.0, 0.0], [10.0, 64.0, 10.0]], [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]);
        assert!(close(b.at(0.0), [0.0, 64.0, 0.0]));
        assert!(close(b.at(1.0), [10.0, 64.0, 10.0]));
        // The handle of a quarter circle of radius r is r * 4/3 * tan(pi/8).
        let h = 10.0 * 4.0 / 3.0 * (std::f64::consts::PI / 8.0).tan();
        assert!(close(b.q1, [h, 64.0, 0.0]), "{:?}", b.q1);
        assert!(close(b.q2, [10.0, 64.0, 10.0 - h]), "{:?}", b.q2);
        // Its midpoint sits on the circle round (0, 10).
        let m = b.at(0.5);
        let r = ((m[0] - 0.0).powi(2) + (m[2] - 10.0).powi(2)).sqrt();
        assert!((r - 10.0).abs() < 0.03, "{r}");
        // A quarter of 2 pi r, near enough with 16 chords.
        assert!((b.length - std::f64::consts::FRAC_PI_2 * 10.0).abs() < 0.1, "{}", b.length);
    }

    #[test]
    fn an_s_bend_takes_create_s_parallel_handles() {
        // Parallel ends offset sideways (an S): angle 0, so the handles come from the crossing.
        let b = Bezier::new([[0.0, 0.0, 0.0], [12.0, 0.0, 4.0]], [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]]);
        assert!(close(b.at(0.0), [0.0, 0.0, 0.0]) && close(b.at(1.0), [12.0, 0.0, 4.0]));
        assert!(b.q1[0] > 0.0 && b.q2[0] < 12.0);
        assert!((b.q1[2]).abs() < 1e-9 && (b.q2[2] - 4.0).abs() < 1e-9);
    }
}
