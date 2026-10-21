// SPDX-License-Identifier: MPL-2.0
//! The viewer API's types and their JSON (docs/api.md). Field names are the JSON's; marks use short
//! keys because a request may carry 2,000 of them.

use serde::{Deserialize, Serialize};

/// Game ticks.
pub type Tick = i64;
/// Ticks in a game day.
pub const TICKS_PER_DAY: Tick = 24_000;
/// Marks a request returns at most (VIEWER.md §6, TUNABLE).
pub const MARK_BUDGET: usize = 2_000;

/// `/api/world`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorldInfo {
    pub name: String,
    /// `save`, `live`, `host` or `demo`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_note: Option<String>,
    #[serde(default)]
    pub build: String,
    pub frames: Vec<Frame>,
    pub time: TimeInfo,
    pub freshness: Freshness,
    pub budget: usize,
    #[serde(default)]
    pub event_kinds: Vec<EventKind>,
    #[serde(default)]
    pub has_events: bool,
    #[serde(default)]
    pub stream: bool,
    /// Whether `/api/find` answers (the page shows its search box).
    #[serde(default)]
    pub find: bool,
}

/// One answer of `/api/find`: a named thing and where it is.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Found {
    pub layer: String,
    pub id: String,
    pub label: String,
    /// What it is, in words ("Station", "Train").
    #[serde(default)]
    pub kind: String,
    pub frame: String,
    pub x: f64,
    pub z: f64,
}

/// A dimension.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frame {
    pub id: String,
    pub label: String,
    pub projection: ProjectionDesc,
    /// `[x0, z0, x1, z1]` where the frame has content.
    pub bounds: [f64; 4],
    /// `[x0, z0, x1, z1]` the page opens on, inside `bounds`: the built-up part, when the base
    /// maps run far wider. None opens on `bounds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<[f64; 4]>,
    #[serde(default)]
    pub tiles: Vec<TileLayerDesc>,
}

/// How a frame's blocks lie on the page (docs/api.md §2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectionDesc {
    /// `flat`, `xwrap` or `plugin`.
    pub kind: String,
    /// East-west wrap period in blocks; 0 for none.
    #[serde(default)]
    pub period_x: f64,
    #[serde(default)]
    pub z_range: Option<[f64; 2]>,
    /// For `plugin`: the script and its factory and arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub args: serde_json::Value,
}

impl Default for ProjectionDesc {
    fn default() -> Self {
        ProjectionDesc::flat()
    }
}

impl ProjectionDesc {
    pub fn flat() -> Self {
        ProjectionDesc {
            kind: "flat".into(),
            period_x: 0.0,
            z_range: None,
            script: None,
            factory: None,
            args: serde_json::Value::Null,
        }
    }

    pub fn xwrap(period: f64, z_range: Option<[f64; 2]>) -> Self {
        ProjectionDesc { kind: "xwrap".into(), period_x: period, z_range, ..ProjectionDesc::flat() }
    }
}

/// A base map layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TileLayerDesc {
    pub id: String,
    pub label: String,
    /// With `{z}`, `{x}`, `{y}`.
    pub url: String,
    pub pyramid: PyramidDesc,
    pub bounds: [f64; 4],
    #[serde(default)]
    pub overlay: bool,
    #[serde(default)]
    pub attribution: String,
    /// Shown when the viewer has no choice of its own saved. None: the page shows the first base map
    /// and every overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_on: Option<bool>,
    /// Opacity, 0 to 1. None: opaque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PyramidDesc {
    /// `square` (zoom 0 is a block a pixel, `pixel(z) = 2^-z`) or a plug-in's name.
    pub kind: String,
    pub min_zoom: i32,
    pub max_zoom: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimeInfo {
    pub tick: Tick,
    pub ticks_per_day: Tick,
    /// Wall-clock milliseconds since the Unix epoch at `tick`.
    pub wall_ms: i64,
    pub tps: f64,
    pub running: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Freshness {
    pub age_ms: u64,
    pub cycle_ms: Option<u64>,
    #[serde(default)]
    pub note: String,
}

/// A kind of archived event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventKind {
    pub id: String,
    pub label: String,
    pub group: String,
    pub color: String,
    /// 0 routine, 1 notable, 2 major.
    pub severity: u8,
}

/// `/api/layers`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Layers {
    pub groups: Vec<Group>,
    pub layers: Vec<LayerDesc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub label: String,
}

/// A layer descriptor (VIEWER.md §4.3): everything the page needs to draw a layer it has never
/// heard of.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerDesc {
    pub id: String,
    pub group: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub kinds: Vec<String>,
    pub priority: i32,
    pub default_on: bool,
    pub min_bpp: f64,
    pub max_bpp: f64,
    pub style: Style,
    #[serde(default)]
    pub legend: Vec<LegendEntry>,
    pub has_cards: bool,
    pub has_past: bool,
    #[serde(default)]
    pub animated: bool,
}

impl LayerDesc {
    /// A layer with defaults: every zoom, on, no legend, cards.
    pub fn new(id: &str, group: &str, label: &str, kind: &str, color: &str) -> LayerDesc {
        LayerDesc {
            id: id.into(),
            group: group.into(),
            label: label.into(),
            description: String::new(),
            kinds: vec![kind.into()],
            priority: 0,
            default_on: true,
            min_bpp: 0.0,
            max_bpp: 1e9,
            style: Style::new(color),
            legend: Vec::new(),
            has_cards: true,
            has_past: false,
            animated: kind == "leg",
        }
    }

    pub fn describe(mut self, d: &str) -> Self {
        self.description = d.into();
        self
    }

    pub fn priority(mut self, p: i32) -> Self {
        self.priority = p;
        self
    }

    pub fn zoom(mut self, min_bpp: f64, max_bpp: f64) -> Self {
        self.min_bpp = min_bpp;
        self.max_bpp = max_bpp;
        self
    }

    pub fn off(mut self) -> Self {
        self.default_on = false;
        self
    }

    pub fn glyph(mut self, g: &str, size: f64) -> Self {
        self.style.glyph = g.into();
        self.style.size = size;
        self
    }

    pub fn width(mut self, w: f64) -> Self {
        self.style.width = w;
        self
    }

    pub fn kinds(mut self, kinds: &[&str]) -> Self {
        self.kinds = kinds.iter().map(|k| (*k).to_owned()).collect();
        self
    }

    pub fn legend(mut self, entries: &[(&str, &str)]) -> Self {
        self.legend = entries.iter().map(|(l, c)| LegendEntry::new(l, c)).collect();
        self
    }

    /// The legend, entries built in full.
    pub fn legend_entries(mut self, entries: Vec<LegendEntry>) -> Self {
        self.legend = entries;
        self
    }

    /// The paths' line style (see [`Style::line`]).
    pub fn line(mut self, l: &str) -> Self {
        self.style.line = Some(l.into());
        self
    }

    /// Labels on the map at `bpp` and closer (see [`Style::label_bpp`]).
    pub fn labels_below(mut self, bpp: f64) -> Self {
        self.style.label_bpp = Some(bpp);
        self
    }

    pub fn no_cards(mut self) -> Self {
        self.has_cards = false;
        self
    }

    pub fn past(mut self) -> Self {
        self.has_past = true;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Style {
    pub color: String,
    pub glyph: String,
    pub size: f64,
    pub width: f64,
    /// The line style of the layer's paths (`rail`, `chain`, `belt`, `flow`, `dash`, `dot`), unless a
    /// mark names its own (`st`). None: a plain line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Point and path labels (`l`) are drawn on the map at this zoom and closer (blocks per CSS
    /// pixel). None: only in the tooltip. Area labels are drawn whenever the area is big enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_bpp: Option<f64>,
}

impl Style {
    pub fn new(color: &str) -> Style {
        Style { color: color.into(), glyph: "circle".into(), size: 6.0, width: 2.0, line: None, label_bpp: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegendEntry {
    pub label: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph: Option<String>,
    /// A line style, for an entry drawn as a line (see [`Style::line`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// A state (`bad`, `warn`, `off`, as a mark's `st`), for an entry that names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub st: Option<String>,
}

impl LegendEntry {
    pub fn new(label: &str, color: &str) -> LegendEntry {
        LegendEntry { label: label.into(), color: color.into(), glyph: None, line: None, st: None }
    }

    pub fn glyph(mut self, g: &str) -> Self {
        self.glyph = Some(g.into());
        self
    }

    pub fn line(mut self, l: &str) -> Self {
        self.line = Some(l.into());
        self
    }

    pub fn st(mut self, st: &str) -> Self {
        self.st = Some(st.into());
        self
    }
}

/// One mark (docs/api.md §4), tagged by `k`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Mark {
    Point {
        id: String,
        x: f64,
        z: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        g: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        s: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        l: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ring: bool,
        /// A heading in radians (0 east, clockwise as z runs south) for glyphs drawn turned
        /// (`station`, `signal`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        a: Option<f64>,
        /// A state the page draws on the glyph: `bad` (a red ring), `warn` (amber), `off` (hollow).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        st: Option<String>,
    },
    Leg {
        id: String,
        p: Vec<[f64; 4]>,
        t0: Tick,
        t1: Tick,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        g: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        l: Option<String>,
    },
    Path {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        p: Vec<[f64; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        w: Option<f64>,
        /// The line style (see [`Style::line`]); None: the layer's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        st: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        l: Option<String>,
        /// Arrows along it, the way `p` runs.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        arrow: bool,
        /// Things on it (packages on a chain), drawn spread along it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        n: Option<u32>,
    },
    Edge {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        a: [f64; 2],
        b: [f64; 2],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        w: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<String>,
    },
    Area {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#box: Option<[f64; 4]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        l: Option<String>,
        /// Outline rings (each closed, even-odd), drawn instead of the box or disc: a district's
        /// chunks. `box` should still be set (their bounds) for older pages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rings: Option<Vec<Vec<[f64; 2]>>>,
        /// The outline's style: `dash`, `dot`, `bad`; None: solid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        st: Option<String>,
    },
    /// A train: one box per carriage, `[x0, z0, x1, z1]` from its front bogey to its back one, the
    /// leading carriage first (so the train runs towards `cars[0]`'s `x0, z0`).
    Train {
        id: String,
        cars: Vec<[f64; 4]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        c: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        l: Option<String>,
        /// `bad` (derailed), `off` (standing), `warn`; None: running.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        st: Option<String>,
    },
    Cell {
        x0: f64,
        z0: f64,
        size: f64,
        w: u32,
        h: u32,
        v: Vec<Option<f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ramp: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lo: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hi: Option<f64>,
    },
    Cluster {
        x: f64,
        z: f64,
        n: u64,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        by: std::collections::BTreeMap<String, u64>,
    },
    Label {
        x: f64,
        z: f64,
        text: String,
    },
}

impl Mark {
    /// A point with only its id and place.
    pub fn point(id: impl ToString, x: f64, z: f64) -> Mark {
        Mark::Point {
            id: id.to_string(),
            x,
            z,
            ci: None,
            c: None,
            g: None,
            s: None,
            l: None,
            ring: false,
            a: None,
            st: None,
        }
    }

    /// A straight leg.
    pub fn leg(id: impl ToString, a: [f64; 2], b: [f64; 2], t0: Tick, t1: Tick) -> Mark {
        Mark::Leg { id: id.to_string(), p: vec![[a[0], a[1], b[0], b[1]]], t0, t1, ci: None, c: None, g: None, l: None }
    }

    pub fn edge(a: [f64; 2], b: [f64; 2], w: f64) -> Mark {
        Mark::Edge { id: None, a, b, w: Some(w), ci: None, c: None }
    }

    pub fn disc(id: Option<String>, x: f64, z: f64, r: f64) -> Mark {
        Mark::Area {
            id,
            x: Some(x),
            z: Some(z),
            r: Some(r),
            r#box: None,
            ci: None,
            c: None,
            l: None,
            rings: None,
            st: None,
        }
    }

    pub fn rect(id: Option<String>, b: [f64; 4]) -> Mark {
        Mark::Area { id, x: None, z: None, r: None, r#box: Some(b), ci: None, c: None, l: None, rings: None, st: None }
    }

    /// A polyline.
    pub fn path(p: Vec<[f64; 2]>) -> Mark {
        Mark::Path { id: None, p, ci: None, c: None, w: None, st: None, l: None, arrow: false, n: None }
    }

    /// An outline of `rings` (closed polygons, even-odd), with their bounds as its box.
    pub fn outline(id: Option<String>, rings: Vec<Vec<[f64; 2]>>) -> Mark {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for q in rings.iter().flatten() {
            b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
        }
        if !b[0].is_finite() {
            b = [0.0; 4];
        }
        Mark::Area {
            id,
            x: None,
            z: None,
            r: None,
            r#box: Some(b),
            ci: None,
            c: None,
            l: None,
            rings: Some(rings),
            st: None,
        }
    }

    /// A train of `cars` (see [`Mark::Train`]).
    pub fn train(id: impl ToString, cars: Vec<[f64; 4]>) -> Mark {
        Mark::Train { id: id.to_string(), cars, ci: None, c: None, l: None, st: None }
    }

    /// Sets a state (points, paths' styles, areas' outlines, trains): see each kind's `st`.
    pub fn with_state(mut self, v: &str) -> Mark {
        match &mut self {
            Mark::Point { st, .. } | Mark::Path { st, .. } | Mark::Area { st, .. } | Mark::Train { st, .. } => {
                *st = Some(v.to_owned())
            }
            _ => {}
        }
        self
    }

    /// Sets a point's heading (radians).
    pub fn with_heading(mut self, v: f64) -> Mark {
        if let Mark::Point { a, .. } = &mut self {
            *a = Some(v);
        }
        self
    }

    /// Sets a path's width (CSS px) or an edge's.
    pub fn with_width(mut self, v: f64) -> Mark {
        match &mut self {
            Mark::Path { w, .. } | Mark::Edge { w, .. } => *w = Some(v),
            _ => {}
        }
        self
    }

    /// Arrows along a path.
    pub fn with_arrows(mut self) -> Mark {
        if let Mark::Path { arrow, .. } = &mut self {
            *arrow = true;
        }
        self
    }

    /// Things spread along a path.
    pub fn with_count(mut self, v: u32) -> Mark {
        if let Mark::Path { n, .. } = &mut self {
            *n = Some(v);
        }
        self
    }

    /// Sets the id that opens its card (no-op for kinds without one: cells, clusters, labels).
    pub fn with_id(mut self, v: impl ToString) -> Mark {
        match &mut self {
            Mark::Point { id, .. } | Mark::Leg { id, .. } | Mark::Train { id, .. } => *id = v.to_string(),
            Mark::Path { id, .. } | Mark::Edge { id, .. } | Mark::Area { id, .. } => *id = Some(v.to_string()),
            _ => {}
        }
        self
    }

    /// Sets the legend index (no-op for kinds without one).
    pub fn with_ci(mut self, v: u16) -> Mark {
        match &mut self {
            Mark::Point { ci, .. }
            | Mark::Leg { ci, .. }
            | Mark::Path { ci, .. }
            | Mark::Edge { ci, .. }
            | Mark::Area { ci, .. }
            | Mark::Train { ci, .. } => *ci = Some(v),
            _ => {}
        }
        self
    }

    /// Sets an explicit colour.
    pub fn with_color(mut self, v: &str) -> Mark {
        match &mut self {
            Mark::Point { c, .. }
            | Mark::Leg { c, .. }
            | Mark::Path { c, .. }
            | Mark::Edge { c, .. }
            | Mark::Area { c, .. }
            | Mark::Train { c, .. } => *c = Some(v.to_owned()),
            _ => {}
        }
        self
    }

    /// Sets the label (points, legs, paths, areas, trains).
    pub fn with_label(mut self, v: impl Into<String>) -> Mark {
        match &mut self {
            Mark::Point { l, .. }
            | Mark::Leg { l, .. }
            | Mark::Path { l, .. }
            | Mark::Area { l, .. }
            | Mark::Train { l, .. } => *l = Some(v.into()),
            _ => {}
        }
        self
    }

    /// Sets a point's size factor.
    pub fn with_size(mut self, v: f64) -> Mark {
        if let Mark::Point { s, .. } = &mut self {
            *s = Some(v);
        }
        self
    }

    /// Sets a point's or leg's glyph.
    pub fn with_glyph(mut self, v: &str) -> Mark {
        match &mut self {
            Mark::Point { g, .. } | Mark::Leg { g, .. } => *g = Some(v.to_owned()),
            _ => {}
        }
        self
    }

    pub fn with_ring(mut self) -> Mark {
        if let Mark::Point { ring, .. } = &mut self {
            *ring = true;
        }
        self
    }

    /// The kind's name, as `k` in JSON.
    pub fn kind(&self) -> &'static str {
        match self {
            Mark::Point { .. } => "point",
            Mark::Leg { .. } => "leg",
            Mark::Path { .. } => "path",
            Mark::Edge { .. } => "edge",
            Mark::Area { .. } => "area",
            Mark::Train { .. } => "train",
            Mark::Cell { .. } => "cell",
            Mark::Cluster { .. } => "cluster",
            Mark::Label { .. } => "label",
        }
    }

    /// The legend index, when it has one.
    pub fn legend_index(&self) -> Option<u16> {
        match self {
            Mark::Point { ci, .. }
            | Mark::Leg { ci, .. }
            | Mark::Path { ci, .. }
            | Mark::Edge { ci, .. }
            | Mark::Area { ci, .. }
            | Mark::Train { ci, .. } => *ci,
            _ => None,
        }
    }

    /// The block rectangle `[x0, z0, x1, z1]` the mark covers (a leg: all of its pieces).
    pub fn bbox(&self) -> [f64; 4] {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let mut add = |x: f64, z: f64| {
            b[0] = b[0].min(x);
            b[1] = b[1].min(z);
            b[2] = b[2].max(x);
            b[3] = b[3].max(z);
        };
        match self {
            Mark::Point { x, z, .. } | Mark::Cluster { x, z, .. } | Mark::Label { x, z, .. } => add(*x, *z),
            Mark::Leg { p, .. } => {
                for q in p {
                    add(q[0], q[1]);
                    add(q[2], q[3]);
                }
            }
            Mark::Path { p, .. } => {
                for q in p {
                    add(q[0], q[1]);
                }
            }
            Mark::Edge { a, b: e, .. } => {
                add(a[0], a[1]);
                add(e[0], e[1]);
            }
            Mark::Train { cars, .. } => {
                for q in cars {
                    add(q[0], q[1]);
                    add(q[2], q[3]);
                }
            }
            Mark::Area { x, z, r, r#box, rings, .. } => {
                for q in rings.iter().flatten().flatten() {
                    add(q[0], q[1]);
                }
                if let Some(bx) = r#box {
                    add(bx[0], bx[1]);
                    add(bx[2], bx[3]);
                } else if let (Some(x), Some(z)) = (x, z) {
                    let r = r.unwrap_or(0.0);
                    add(x - r, z - r);
                    add(x + r, z + r);
                }
            }
            Mark::Cell { x0, z0, size, w, h, .. } => {
                add(*x0, *z0);
                add(x0 + size * f64::from(*w), z0 + size * f64::from(*h));
            }
        }
        if !b[0].is_finite() {
            return [0.0; 4];
        }
        b
    }

    /// Where the mark stands at time `t` (a leg's mover; the centre of anything else), for binning.
    pub fn anchor(&self, t: Tick) -> [f64; 2] {
        match self {
            Mark::Leg { p, t0, t1, .. } => leg_at(p, *t0, *t1, t),
            Mark::Edge { a, b, .. } => [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
            Mark::Train { cars, .. } if !cars.is_empty() => [cars[0][0], cars[0][1]],
            _ => {
                let b = self.bbox();
                [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
            }
        }
    }

    /// The thing's id, for marks that have one.
    pub fn id(&self) -> Option<&str> {
        match self {
            Mark::Point { id, .. } | Mark::Leg { id, .. } | Mark::Train { id, .. } => Some(id),
            Mark::Path { id, .. } | Mark::Edge { id, .. } | Mark::Area { id, .. } => id.as_deref(),
            _ => None,
        }
    }
}

/// The place on a leg's pieces at time `t` (before `t0` its start, after `t1` its end), by length.
/// The page's `MicViewer.legAt` is the same function.
pub fn leg_at(p: &[[f64; 4]], t0: Tick, t1: Tick, t: Tick) -> [f64; 2] {
    let Some(first) = p.first() else { return [0.0, 0.0] };
    let last = p[p.len() - 1];
    if t <= t0 || t1 <= t0 {
        return if t1 <= t0 && t >= t1 { [last[2], last[3]] } else { [first[0], first[1]] };
    }
    if t >= t1 {
        return [last[2], last[3]];
    }
    let total: f64 = p.iter().map(|q| (q[2] - q[0]).hypot(q[3] - q[1])).sum();
    if total <= 0.0 {
        return [first[0], first[1]];
    }
    let mut want = total * ((t - t0) as f64 / (t1 - t0) as f64);
    for q in p {
        let len = (q[2] - q[0]).hypot(q[3] - q[1]);
        if want <= len {
            let f = if len > 0.0 { want / len } else { 0.0 };
            return [q[0] + (q[2] - q[0]) * f, q[1] + (q[3] - q[1]) * f];
        }
        want -= len;
    }
    [last[2], last[3]]
}

/// One layer in a `/api/marks` answer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LayerMarks {
    pub version: u64,
    pub total: u64,
    pub shown: u64,
    pub clustered: bool,
    pub marks: Vec<Mark>,
    /// Why the layer has nothing in this frame at all (a module the world does not run, a part
    /// that failed to read), for the page's layer list. Absent when it has marks somewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
}

/// `/api/marks`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MarksAnswer {
    pub t: Tick,
    pub frame: String,
    pub layers: std::collections::BTreeMap<String, LayerMarks>,
}

/// A card (VIEWER.md §4.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Card {
    pub layer: String,
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<Tick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub series: Vec<Series>,
}

impl Card {
    pub fn new(layer: &str, id: impl ToString, title: impl Into<String>) -> Card {
        Card { layer: layer.into(), id: id.to_string(), title: title.into(), ..Card::default() }
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Card {
        self.subtitle = Some(s.into());
        self
    }

    pub fn at(mut self, x: f64, z: f64) -> Card {
        self.at = Some([x, z]);
        self
    }

    /// The game tick its fields are as of (a card built from a save says when that was).
    pub fn as_of(mut self, t: Tick) -> Card {
        self.as_of = Some(t);
        self
    }

    pub fn field(mut self, label: &str, value: impl Into<Value>) -> Card {
        self.fields.push(Field::new(label, value));
        self
    }

    pub fn field_unit(mut self, label: &str, value: impl Into<Value>, unit: &str) -> Card {
        self.fields.push(Field::new(label, value).unit(unit));
        self
    }

    pub fn link(mut self, label: &str, value: impl Into<Value>, layer: &str, id: impl ToString) -> Card {
        self.fields.push(Field::new(label, value).link(layer, id));
        self
    }

    pub fn section(mut self, s: Section) -> Card {
        if !s.fields.is_empty() {
            self.sections.push(s);
        }
        self
    }

    pub fn series(mut self, s: Series) -> Card {
        self.series.push(s);
        self
    }

    pub fn note(mut self, n: impl Into<String>) -> Card {
        self.note = Some(n.into());
        self
    }
}

/// A field's value: text or a number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Num(f64),
    Text(String),
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Num(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::Num(f64::from(v))
    }
}
impl From<u64> for Value {
    fn from(v: u64) -> Value {
        Value::Num(v as f64)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::Num(v as f64)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Value {
        Value::Num(f64::from(v))
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::Num(f64::from(v))
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Value {
        Value::Num(v as f64)
    }
}
impl From<u16> for Value {
    fn from(v: u16) -> Value {
        Value::Num(f64::from(v))
    }
}
impl From<u8> for Value {
    fn from(v: u8) -> Value {
        Value::Num(f64::from(v))
    }
}
impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Text(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Text(v.to_owned())
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Text(if v { "yes" } else { "no" }.into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub label: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
}

impl Field {
    pub fn new(label: &str, value: impl Into<Value>) -> Field {
        Field { label: label.into(), value: value.into(), unit: None, link: None }
    }

    pub fn unit(mut self, unit: &str) -> Field {
        self.unit = Some(unit.into());
        self
    }

    pub fn link(mut self, layer: &str, id: impl ToString) -> Field {
        self.link = Some(Link { layer: layer.into(), id: id.to_string() });
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub layer: String,
    pub id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub fields: Vec<Field>,
}

impl Section {
    pub fn new(title: &str) -> Section {
        Section { title: title.into(), fields: Vec::new() }
    }

    pub fn field(mut self, label: &str, value: impl Into<Value>) -> Section {
        self.fields.push(Field::new(label, value));
        self
    }

    pub fn field_unit(mut self, label: &str, value: impl Into<Value>, unit: &str) -> Section {
        self.fields.push(Field::new(label, value).unit(unit));
        self
    }

    pub fn push(&mut self, f: Field) {
        self.fields.push(f);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Series {
    pub label: String,
    #[serde(default)]
    pub unit: String,
    pub points: Vec<[f64; 2]>,
}

/// An archived event (VIEWER.md §10.2), as the page sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRow {
    pub t: Tick,
    pub kind: String,
    pub sev: u8,
    pub x: f64,
    pub z: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    #[serde(default)]
    pub mag: i64,
    #[serde(default)]
    pub text: String,
    /// The frame it happened in. None: the world's first frame. A stream `events` row in another
    /// frame than the page's is listed and flashed only there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

/// An archived leg.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegRow {
    pub t0: Tick,
    pub t1: Tick,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub p: Vec<[f64; 4]>,
}

/// `/api/events`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EventsAnswer {
    pub rows: Vec<EventRow>,
    pub legs: Vec<LegRow>,
    pub truncated: bool,
}

/// `/api/events/histogram`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Histogram {
    pub from: Tick,
    pub to: Tick,
    pub buckets: usize,
    pub series: std::collections::BTreeMap<String, Vec<u64>>,
    /// `[tick, wall ms]` pairs.
    pub wall: Vec<[i64; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_serialize_with_short_keys() {
        let m = Mark::point(7, 10.0, -3.0).with_ci(2).with_size(1.5);
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"k":"point","id":"7","x":10.0,"z":-3.0,"ci":2,"s":1.5}"#);
        let a = Mark::rect(None, [0.0, 0.0, 16.0, 32.0]);
        assert_eq!(serde_json::to_string(&a).unwrap(), r#"{"k":"area","box":[0.0,0.0,16.0,32.0]}"#);
        let back: Mark = serde_json::from_str(r#"{"k":"edge","a":[0,0],"b":[3,4],"w":2}"#).unwrap();
        assert_eq!(back, Mark::edge([0.0, 0.0], [3.0, 4.0], 2.0));
    }

    #[test]
    fn ids_cards_and_empty_layers() {
        let e = Mark::edge([0.0, 0.0], [3.0, 4.0], 2.0).with_id("road-7");
        assert_eq!(e.id(), Some("road-7"));
        assert_eq!(Mark::point(1, 0.0, 0.0).with_id(9).id(), Some("9"));
        assert_eq!(Mark::disc(None, 0.0, 0.0, 4.0).with_id("a").id(), Some("a"));
        let label = Mark::Label { x: 0.0, z: 0.0, text: "x".into() };
        assert_eq!(label.clone().with_id("no"), label, "a label has no id");
        let c = Card::new("towns", 3, "Ashford").as_of(48_000);
        assert_eq!(c.as_of, Some(48_000));
        assert!(serde_json::to_string(&c).unwrap().contains(r#""as_of":48000"#));
        // `empty` is left out unless there is a reason.
        let lm = LayerMarks::default();
        assert!(!serde_json::to_string(&lm).unwrap().contains("empty"));
        let lm = LayerMarks { empty: Some("no ships module".into()), ..LayerMarks::default() };
        assert!(serde_json::to_string(&lm).unwrap().contains(r#""empty":"no ships module""#));
    }

    #[test]
    fn optional_fields_stay_out_of_the_json_until_set() {
        let row: EventRow = serde_json::from_str(r#"{"t":5,"kind":"trade.deal","sev":0,"x":1,"z":2}"#).unwrap();
        assert_eq!(row.frame, None);
        assert!(!serde_json::to_string(&row).unwrap().contains("frame"));
        let ring = EventRow { frame: Some("demo:ring".into()), ..row };
        assert!(serde_json::to_string(&ring).unwrap().contains(r#""frame":"demo:ring""#));
        let d: TileLayerDesc = serde_json::from_str(
            r#"{"id":"t","label":"T","url":"t/{z}/{x}/{y}.png","pyramid":{"kind":"square","min_zoom":-7,"max_zoom":0},"bounds":[0,0,1,1]}"#,
        )
        .unwrap();
        assert_eq!((d.default_on, d.alpha), (None, None));
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("default_on") && !json.contains("alpha"), "{json}");
        let d = TileLayerDesc { default_on: Some(false), alpha: Some(0.6), ..d };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains(r#""default_on":false"#) && json.contains(r#""alpha":0.6"#), "{json}");
    }

    #[test]
    fn leg_positions() {
        // Two pieces of length 10 and 30: at a quarter of the time, at the end of the first.
        let p = [[0.0, 0.0, 10.0, 0.0], [10.0, 0.0, 10.0, 30.0]];
        assert_eq!(leg_at(&p, 100, 500, 50), [0.0, 0.0]);
        assert_eq!(leg_at(&p, 100, 500, 200), [10.0, 0.0]);
        assert_eq!(leg_at(&p, 100, 500, 300), [10.0, 10.0]);
        assert_eq!(leg_at(&p, 100, 500, 900), [10.0, 30.0]);
        assert_eq!(leg_at(&[], 0, 1, 0), [0.0, 0.0]);
        assert_eq!(leg_at(&p, 100, 100, 100), [10.0, 30.0]);
    }

    #[test]
    fn bboxes() {
        assert_eq!(Mark::disc(None, 10.0, 20.0, 5.0).bbox(), [5.0, 15.0, 15.0, 25.0]);
        let leg = Mark::Leg {
            id: "1".into(),
            p: vec![[0.0, 5.0, 10.0, -5.0], [10.0, -5.0, 20.0, 0.0]],
            t0: 0,
            t1: 1,
            ci: None,
            c: None,
            g: None,
            l: None,
        };
        assert_eq!(leg.bbox(), [0.0, -5.0, 20.0, 5.0]);
    }
}
