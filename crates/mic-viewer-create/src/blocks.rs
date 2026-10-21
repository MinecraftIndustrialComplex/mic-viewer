// SPDX-License-Identifier: MPL-2.0
//! Block entities out of a world's region files, read only where something asks, as small records.
//!
//! An Anvil region file `r.<rx>.<rz>.mca` starts with 1,024 four-byte locations (three bytes of
//! 4 KiB sector offset, one of sector count), chunk `(cx & 31) + (cz & 31) * 32`; a chunk is a
//! four-byte length, a compression byte (1 gzip, 2 zlib, 3 none; plus 128 when the chunk is in
//! `c.<cx>.<cz>.mcc` beside the region) and an NBT compound whose `block_entities` list this reads.
//! The rest of the chunk (sections, heightmaps, entities) is skipped without being built.
//!
//! A region file is read whole into memory and closed before anything is parsed, so nothing holds a
//! world's file open and a server writing it meanwhile costs at most a torn chunk, which reads as
//! an error and is skipped. Nothing is ever written.
//!
//! What is kept of a block entity ([`Thing`]) was read from Create 6.0.10's block entities' `write`
//! methods and checked against saved worlds: kinetic blocks' `Network {Id, Stress, Capacity,
//! AddedStress?, AddedCapacity?}` and `Speed` (saved even when the network is overstressed, when
//! the block itself stands still); belts' `Controller` (index 0, the end the belt's facing runs
//! from), `Index` and `Length`; chain conveyors' `Connections` (offsets to the other pulleys) and
//! `TravellingPackages [{Target, Packages}]`; package ports' addresses; factory gauges' slots
//! (`top_left` etc.: `Filter {id, count}`, `RecipeAddress`, `Satisfied`); vaults' and tanks'
//! contents.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mic_viewer_surveyor::nbt::{self, Tag};

use crate::tags::{self, f, flag, i, s};

/// A chunk bigger than this once inflated is skipped.
const MAX_CHUNK: u64 = 64 << 20;
/// A region file bigger than this is not read.
const MAX_REGION: u64 = 1 << 30;

/// A kinetic network as a block saw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Net {
    pub id: i64,
    pub stress: f32,
    pub capacity: f32,
    /// What this block adds: stress it takes, capacity it gives.
    pub adds_stress: f32,
    pub adds_capacity: f32,
}

impl Net {
    /// More stress than capacity: every block on it stands still.
    pub fn overstressed(&self) -> bool {
        self.stress > self.capacity + 1e-3
    }
}

/// What a block entity is, for the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Processing: presses, mixers, deployers, crafters, saws, fans, spouts, furnaces.
    Machine,
    /// Gives the kinetic network capacity: wheels, engines, motors.
    Power,
    /// Vaults and tanks.
    Storage,
    /// Arms and ejectors.
    Mover,
    /// Package addresses: frogports, packagers, postboxes, requesters, stock links.
    Port,
    /// Factory gauges.
    Gauge,
    Belt,
    Chain,
    /// Chests, barrels and the like: counted, not drawn.
    Container,
    /// Shafts, funnels, chutes, tunnels and the rest: counted, not drawn.
    Other,
}

/// A factory gauge's slot: the item it keeps in stock.
#[derive(Clone, Debug, PartialEq)]
pub struct GaugeSlot {
    pub item: String,
    pub target: i64,
    pub address: String,
    pub satisfied: bool,
    /// The level it last saw.
    pub level: i64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Detail {
    #[default]
    None,
    /// A package address (packager sign, postbox or requester).
    Address(String),
    Frogport {
        address: String,
        /// The chain pulley it hangs from.
        chain: Option<[i32; 3]>,
    },
    Requester {
        address: String,
        request: Vec<(String, i64)>,
    },
    Belt {
        controller: [i32; 3],
        index: u32,
        length: u32,
    },
    Chain {
        /// The pulleys it links to (absolute).
        links: Vec<[i32; 3]>,
        /// Packages on the way to each linked pulley.
        travelling: Vec<([i32; 3], u32)>,
        looping: u32,
        /// Package addresses on it.
        addresses: Vec<String>,
    },
    Gauge(Vec<GaugeSlot>),
    Store {
        /// The multiblock's controller (only it holds the contents).
        controller: bool,
        items: BTreeMap<String, i64>,
        fluids: BTreeMap<String, i64>,
        /// Item slots, or fluid capacity in mB.
        capacity: i64,
    },
}

/// One block entity, kept small.
#[derive(Clone, Debug, PartialEq)]
pub struct Thing {
    pub pos: [i32; 3],
    pub id: String,
    pub kind: Kind,
    pub net: Option<Net>,
    /// RPM as saved (signed).
    pub speed: f32,
    /// Whether it works now, where the save says: turning for a kinetic block, lit for a furnace.
    /// None for a block with no such state (a spout, a blaze heater).
    pub active: Option<bool>,
    pub detail: Detail,
}

impl Thing {
    /// Working: turning on a network that is not overstressed, or lit. A block the save gives no
    /// such state counts as working.
    pub fn running(&self) -> bool {
        self.active != Some(false) && !self.net.is_some_and(|n| n.overstressed())
    }

    /// Whether the save says it is not working (a stopped kinetic block, an unlit furnace).
    pub fn idle(&self) -> bool {
        !self.running() && (self.active.is_some() || self.net.is_some())
    }

    /// Words for what it is ("Mechanical press", "Steam engine").
    pub fn name(&self) -> String {
        match self.id.as_str() {
            "create:powered_shaft" => "Steam engine".into(),
            "create:crushing_wheel_controller" => "Crushing wheels".into(),
            "create:motor" => "Creative motor".into(),
            "create:drill" => "Mechanical drill".into(),
            "create:saw" => "Mechanical saw".into(),
            "create:belt" => "Belt".into(),
            "create:factory_panel" => "Factory gauge".into(),
            "create:packager_link" => "Stock link".into(),
            id => tags::humanize(id),
        }
    }
}

/// Machines that take no stress but process.
const PROCESSING: &[&str] = &[
    "create:spout",
    "create:item_drain",
    "create:crushing_wheel_controller",
    "create:millstone",
    "create:blaze_heater",
    "minecraft:furnace",
    "minecraft:blast_furnace",
    "minecraft:smoker",
    "minecraft:brewing_stand",
    "minecraft:crafter",
];

const CONTAINERS: &[&str] = &[
    "minecraft:chest",
    "minecraft:trapped_chest",
    "minecraft:barrel",
    "minecraft:shulker_box",
    "minecraft:hopper",
    "minecraft:dropper",
    "minecraft:dispenser",
    "lootr:lootr_chest",
    "lootr:lootr_barrel",
    "create:depot",
    "create:toolbox",
];

/// Kinetic blocks that only carry rotation (no stress of their own).
fn transmission(id: &str) -> bool {
    matches!(
        id,
        "create:simple_kinetic"
            | "create:encased_shaft"
            | "create:encased_cogwheel"
            | "create:encased_large_cogwheel"
            | "create:gearbox"
            | "create:clutch"
            | "create:gearshift"
            | "create:adjustable_chain_gearshift"
            | "create:sequenced_gearshift"
            | "create:rotation_speed_controller"
            | "create:speedometer"
            | "create:stressometer"
            | "create:flywheel"
            | "create:chain_drive"
            | "create:powered_latch"
    )
}

fn classify(id: &str, net: Option<&Net>) -> Kind {
    match id {
        "create:belt" => return Kind::Belt,
        "create:chain_conveyor" => return Kind::Chain,
        "create:factory_panel" => return Kind::Gauge,
        "create:item_vault" | "create:fluid_tank" | "create:creative_fluid_tank" => return Kind::Storage,
        "create:mechanical_arm" | "create:weighted_ejector" => return Kind::Mover,
        "create:package_frogport"
        | "create:packager"
        | "create:repackager"
        | "create:redstone_requester"
        | "create:stock_ticker"
        | "create:packager_link" => return Kind::Port,
        _ => {}
    }
    if id.ends_with("postbox") {
        return Kind::Port;
    }
    if PROCESSING.contains(&id) {
        return Kind::Machine;
    }
    if CONTAINERS.contains(&id) {
        return Kind::Container;
    }
    if let Some(n) = net {
        if n.adds_capacity > 0.0 {
            return Kind::Power;
        }
        if n.adds_stress > 0.0 && !transmission(id) {
            return Kind::Machine;
        }
    }
    if matches!(id, "create:motor" | "create:creative_motor" | "create:hand_crank" | "create:steam_engine") {
        return Kind::Power;
    }
    Kind::Other
}

fn read_net(be: &Tag) -> Option<Net> {
    let n = be.get("Network")?;
    Some(Net {
        id: i(n, "Id")?,
        stress: f(n, "Stress").unwrap_or(0.0) as f32,
        capacity: f(n, "Capacity").unwrap_or(0.0) as f32,
        adds_stress: f(n, "AddedStress").unwrap_or(0.0) as f32,
        adds_capacity: f(n, "AddedCapacity").unwrap_or(0.0) as f32,
    })
}

fn add3(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Package addresses on items under `t` (a package's `create:package_address` component).
fn addresses(t: &Tag, out: &mut Vec<String>, depth: usize) {
    if depth > 16 {
        return;
    }
    match t {
        Tag::Compound(v) => {
            for (k, x) in v {
                if k.ends_with("package_address")
                    && let Some(a) = x.as_str()
                    && !a.is_empty()
                    && !out.iter().any(|o| o == a)
                {
                    out.push(a.to_owned());
                }
                addresses(x, out, depth + 1);
            }
        }
        Tag::List(v) => v.iter().for_each(|x| addresses(x, out, depth + 1)),
        _ => {}
    }
}

/// A block entity as a [`Thing`]; None for one without an id or position.
pub fn thing(be: &Tag) -> Option<Thing> {
    let id = s(be, "id")?.to_owned();
    let pos = [i(be, "x")? as i32, i(be, "y")? as i32, i(be, "z")? as i32];
    let net = read_net(be);
    let kind = classify(&id, net.as_ref());
    let speed = f(be, "Speed").unwrap_or(0.0) as f32;
    // Kinetic block entities always save their speed; furnaces (1.21.1) their burn time.
    let active = if be.get("Speed").is_some() { Some(speed.abs() > 0.0) } else { f(be, "BurnTime").map(|t| t > 0.0) };
    let detail = match kind {
        Kind::Belt => Detail::Belt {
            controller: be.get("Controller").and_then(tags::pos3).unwrap_or(pos),
            index: i(be, "Index").unwrap_or(0).max(0) as u32,
            length: i(be, "Length").unwrap_or(1).max(1) as u32,
        },
        Kind::Chain => {
            let links: Vec<[i32; 3]> = be
                .get("Connections")
                .map(Tag::items)
                .unwrap_or_default()
                .iter()
                .filter_map(tags::pos3)
                .map(|d| add3(pos, d))
                .collect();
            let mut addrs = Vec::new();
            let travelling = be
                .get("TravellingPackages")
                .map(Tag::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|t| {
                    let to = add3(pos, t.get("Target").and_then(tags::pos3)?);
                    let p = t.get("Packages").map(Tag::items).unwrap_or_default();
                    p.iter().for_each(|x| addresses(x, &mut addrs, 0));
                    Some((to, p.len() as u32))
                })
                .collect();
            let looping = be.get("LoopingPackages").map(Tag::items).unwrap_or_default();
            looping.iter().for_each(|x| addresses(x, &mut addrs, 0));
            Detail::Chain { links, travelling, looping: looping.len() as u32, addresses: addrs }
        }
        Kind::Port => match id.as_str() {
            "create:package_frogport" => Detail::Frogport {
                address: s(be, "AddressFilter").unwrap_or("").to_owned(),
                chain: be.get("Target").and_then(|t| t.get("relative_pos")).and_then(tags::pos3).map(|d| add3(pos, d)),
            },
            "create:redstone_requester" => {
                let mut items: BTreeMap<String, i64> = BTreeMap::new();
                if let Some(r) = be.get("EncodedRequest") {
                    for e in r.get("ordered_stacks").and_then(|o| o.get("entries")).map(Tag::items).unwrap_or_default()
                    {
                        if let Some(st) = e.get("item_stack").and_then(|x| s(x, "id")) {
                            *items.entry(st.to_owned()).or_insert(0) += i(e, "count").unwrap_or(1);
                        }
                    }
                }
                Detail::Requester {
                    address: s(be, "EncodedAddress").unwrap_or("").to_owned(),
                    request: items.into_iter().collect(),
                }
            }
            "create:stock_ticker" => Detail::Address(s(be, "PreviousAddress").unwrap_or("").to_owned()),
            _ => Detail::Address(
                s(be, "SignAddress")
                    .filter(|a| !a.is_empty())
                    .or_else(|| s(be, "AddressFilter"))
                    .or_else(|| s(be, "ComputerAddress"))
                    .unwrap_or("")
                    .to_owned(),
            ),
        },
        Kind::Gauge => {
            let mut slots = Vec::new();
            for k in ["top_left", "top_right", "bottom_left", "bottom_right"] {
                let Some(sl) = be.get(k) else { continue };
                let Some(fl) = sl.get("Filter") else { continue };
                let Some(item) = s(fl, "id") else { continue };
                slots.push(GaugeSlot {
                    item: item.to_owned(),
                    target: i(fl, "count").unwrap_or(0),
                    address: s(sl, "RecipeAddress").unwrap_or("").to_owned(),
                    satisfied: flag(sl, "Satisfied"),
                    level: i(sl, "LastLevel").unwrap_or(0),
                });
            }
            Detail::Gauge(slots)
        }
        Kind::Storage => {
            let controller = be.get("Controller").and_then(tags::pos3).is_none_or(|c| c == pos);
            let mut items = BTreeMap::new();
            let mut fluids = BTreeMap::new();
            let mut capacity = 0;
            if controller {
                if let Some(inv) = be.get("Inventory") {
                    tags::stacks(inv, &mut items, &mut fluids);
                    capacity = i(inv, "Size").unwrap_or(0);
                }
                if let Some(t) = be.get("TankContent") {
                    tags::stacks(t, &mut items, &mut fluids);
                    // A tank holds 8 buckets a block.
                    capacity = i(be, "Size").unwrap_or(1).max(1).pow(2) * i(be, "Height").unwrap_or(1).max(1) * 8000;
                }
            }
            Detail::Store { controller, items, fluids, capacity }
        }
        _ => Detail::None,
    };
    Some(Thing { pos, id, kind, net, speed, active, detail })
}

/// The region folder of a dimension.
pub fn region_dir(world: &Path, dim: &str) -> PathBuf {
    match dim {
        "minecraft:overworld" | "" => world.join("region"),
        "minecraft:the_nether" => world.join("DIM-1").join("region"),
        "minecraft:the_end" => world.join("DIM1").join("region"),
        d => {
            let (ns, path) = d.split_once(':').unwrap_or(("minecraft", d));
            world.join("dimensions").join(ns).join(path).join("region")
        }
    }
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// The block entities of the chunks `wanted` (absolute chunk coordinates, all in region
/// `rx, rz`) from `dir`. A chunk the file does not have reads as none; a chunk that does not
/// parse is left out of the answer (and counted in the second value).
/// Chunks read from a region file, each with its block entities of interest.
pub type ChunkThings = Vec<((i32, i32), Vec<Thing>)>;

pub fn read_region(dir: &Path, rx: i32, rz: i32, wanted: &[(i32, i32)]) -> io::Result<(ChunkThings, usize)> {
    let path = dir.join(format!("r.{rx}.{rz}.mca"));
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((wanted.iter().map(|&c| (c, Vec::new())).collect(), 0));
        }
        Err(e) => return Err(e),
    };
    if meta.len() > MAX_REGION {
        return Err(invalid(format!("{} is {} bytes", path.display(), meta.len())));
    }
    let raw = std::fs::read(&path)?;
    let mut out = Vec::with_capacity(wanted.len());
    let mut bad = 0;
    for &(cx, cz) in wanted {
        match chunk_bytes(&raw, dir, cx, cz).and_then(|b| match b {
            None => Ok(Vec::new()),
            Some(b) => block_entities(&b),
        }) {
            Ok(bes) => out.push(((cx, cz), bes.iter().filter_map(thing).collect())),
            Err(_) => bad += 1,
        }
    }
    Ok((out, bad))
}

/// One chunk's NBT, inflated; None when the region has no such chunk.
fn chunk_bytes(raw: &[u8], dir: &Path, cx: i32, cz: i32) -> io::Result<Option<Vec<u8>>> {
    if raw.len() < 8192 {
        return Ok(None);
    }
    let k = ((cx & 31) + (cz & 31) * 32) as usize * 4;
    let off = (u32::from(raw[k]) << 16 | u32::from(raw[k + 1]) << 8 | u32::from(raw[k + 2])) as usize * 4096;
    if off == 0 {
        return Ok(None);
    }
    if off + 5 > raw.len() {
        return Err(invalid("chunk past the file's end"));
    }
    let n = u32::from_be_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]) as usize;
    let comp = raw[off + 4];
    let body: Vec<u8>;
    let data: &[u8] = if comp & 128 != 0 {
        body = std::fs::read(dir.join(format!("c.{cx}.{cz}.mcc")))?;
        &body
    } else {
        if n < 1 || off + 4 + n > raw.len() {
            return Err(invalid("chunk length past the file's end"));
        }
        &raw[off + 5..off + 4 + n]
    };
    let mut out = Vec::new();
    match comp & 127 {
        1 => {
            flate2::read::GzDecoder::new(data).take(MAX_CHUNK + 1).read_to_end(&mut out)?;
        }
        2 => {
            flate2::read::ZlibDecoder::new(data).take(MAX_CHUNK + 1).read_to_end(&mut out)?;
        }
        3 => out.extend_from_slice(data),
        c => return Err(invalid(format!("compression {c}"))),
    }
    if out.len() as u64 > MAX_CHUNK {
        return Err(invalid("chunk inflates too far"));
    }
    Ok(Some(out))
}

/// A cursor that walks NBT without building it.
struct Skip<'a> {
    b: &'a [u8],
    i: usize,
}

impl Skip<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.b.len() - self.i < n {
            return Err(invalid("NBT ends early"));
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn len(&mut self, elem: usize) -> io::Result<usize> {
        let b = self.take(4)?;
        let n = i32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        let n = usize::try_from(n).map_err(|_| invalid("negative length"))?;
        if n.saturating_mul(elem) > self.b.len() - self.i {
            return Err(invalid("length past the end"));
        }
        Ok(n)
    }
    fn string(&mut self) -> io::Result<&[u8]> {
        let b = self.take(2)?;
        let n = u16::from_be_bytes([b[0], b[1]]) as usize;
        self.take(n)
    }
    fn skip(&mut self, t: u8, depth: usize) -> io::Result<()> {
        if depth > nbt::MAX_DEPTH {
            return Err(invalid("nested too deep"));
        }
        match t {
            1 => drop(self.take(1)?),
            2 => drop(self.take(2)?),
            3 | 5 => drop(self.take(4)?),
            4 | 6 => drop(self.take(8)?),
            7 => {
                let n = self.len(1)?;
                self.take(n)?;
            }
            8 => drop(self.string()?),
            9 => {
                let et = self.u8()?;
                let n = self.len(if et == 0 { 0 } else { 1 })?;
                for _ in 0..n {
                    self.skip(et, depth + 1)?;
                }
            }
            10 => loop {
                let tt = self.u8()?;
                if tt == 0 {
                    break;
                }
                self.string()?;
                self.skip(tt, depth + 1)?;
            },
            11 => {
                let n = self.len(4)?;
                self.take(n * 4)?;
            }
            12 => {
                let n = self.len(8)?;
                self.take(n * 8)?;
            }
            _ => return Err(invalid(format!("tag type {t}"))),
        }
        Ok(())
    }
}

/// A chunk's `block_entities`, the rest skipped.
pub fn block_entities(chunk: &[u8]) -> io::Result<Vec<Tag>> {
    let mut c = Skip { b: chunk, i: 0 };
    if c.u8()? != 10 {
        return Err(invalid("chunk root is not a compound"));
    }
    c.string()?;
    loop {
        let t = c.u8()?;
        if t == 0 {
            return Ok(Vec::new());
        }
        let name = c.string()?;
        let want = t == 9 && name == b"block_entities";
        let start = c.i;
        c.skip(t, 1)?;
        if want {
            // A root compound holding just this list, for the checked reader.
            let mut buf = Vec::with_capacity(c.i - start + 24);
            buf.extend_from_slice(&[10, 0, 0, 9, 0, 14]);
            buf.extend_from_slice(b"block_entities");
            buf.extend_from_slice(&chunk[start..c.i]);
            buf.push(0);
            let root = nbt::read(&buf)?;
            return Ok(match root {
                Tag::Compound(mut v) => match v.pop() {
                    Some((_, Tag::List(l))) => l,
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            });
        }
    }
}

/// A chunk's key: dimension id and chunk coordinates.
type Key = (String, i32, i32);

#[derive(Default)]
struct State {
    /// Chunks read (with what they hold; most hold nothing of interest).
    things: HashMap<Key, Arc<Vec<Thing>>>,
    /// Chunks asked for and not read yet, the most urgent first.
    queue: VecDeque<Key>,
    queued: HashSet<Key>,
    /// Chunks that did not read.
    failed: usize,
    regions_read: u64,
    stop: bool,
}

/// What has been read of a world's block entities, and a worker reading more. The worker runs on
/// its own thread (at whatever priority the process has: the viewer's is idle) and reads one
/// region file at a time, the chunks the newest asks wanted first.
pub struct Survey {
    world: PathBuf,
    state: Arc<(Mutex<State>, Condvar)>,
    /// Bumped whenever chunks land, so a caller can tell when to redraw.
    version: Arc<AtomicU64>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Survey {
    /// Starts a survey of `world` (a world folder). `landed` is called on the worker thread after
    /// chunks land, at most every `every` (and once more when the queue runs dry).
    pub fn new(world: &Path, every: Duration, landed: Box<dyn Fn() + Send + Sync>) -> Survey {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let version = Arc::new(AtomicU64::new(0));
        let (st, ver, dir) = (Arc::clone(&state), Arc::clone(&version), world.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("create-survey".into())
            .spawn(move || work(&dir, &st, &ver, every, &*landed))
            .ok();
        Survey { world: world.to_path_buf(), state, version, thread }
    }

    pub fn world(&self) -> &Path {
        &self.world
    }

    /// Asks for chunks of `dim`; `urgent` ones go before everything asked before (the view's),
    /// the others after (a district's). Chunks read or asked for already are not asked again.
    pub fn want(&self, dim: &str, chunks: impl IntoIterator<Item = (i32, i32)>, urgent: bool) {
        let (m, cv) = &*self.state;
        let mut s = lock(m);
        let mut fresh: Vec<Key> = Vec::new();
        for (cx, cz) in chunks {
            let k = (dim.to_owned(), cx, cz);
            if s.things.contains_key(&k) {
                continue;
            }
            if urgent && s.queued.contains(&k) {
                // Move it to the front.
                if let Some(p) = s.queue.iter().position(|q| q == &k) {
                    s.queue.remove(p);
                }
                fresh.push(k);
                continue;
            }
            if s.queued.insert(k.clone()) {
                fresh.push(k);
            }
        }
        if fresh.is_empty() {
            return;
        }
        if urgent {
            for k in fresh.into_iter().rev() {
                s.queue.push_front(k);
            }
        } else {
            s.queue.extend(fresh);
        }
        cv.notify_all();
    }

    /// Whether a chunk has been read.
    pub fn has(&self, dim: &str, cx: i32, cz: i32) -> bool {
        lock(&self.state.0).things.contains_key(&(dim.to_owned(), cx, cz))
    }

    /// The things read in `dim`, chunk by chunk.
    pub fn all(&self, dim: &str) -> Vec<Arc<Vec<Thing>>> {
        lock(&self.state.0)
            .things
            .iter()
            .filter(|(k, v)| k.0 == dim && !v.is_empty())
            .map(|(_, v)| Arc::clone(v))
            .collect()
    }

    /// The things read in these chunks of `dim`, and how many of the chunks are still unread.
    pub fn in_chunks(&self, dim: &str, chunks: impl IntoIterator<Item = (i32, i32)>) -> (Vec<Thing>, usize) {
        let s = lock(&self.state.0);
        let mut out = Vec::new();
        let mut missing = 0;
        for (cx, cz) in chunks {
            match s.things.get(&(dim.to_owned(), cx, cz)) {
                Some(v) => out.extend(v.iter().cloned()),
                None => missing += 1,
            }
        }
        (out, missing)
    }

    /// The thing at a block, if its chunk has been read.
    pub fn at(&self, dim: &str, pos: [i32; 3]) -> Option<Thing> {
        let s = lock(&self.state.0);
        let v = s.things.get(&(dim.to_owned(), pos[0] >> 4, pos[2] >> 4))?;
        v.iter().find(|t| t.pos == pos).cloned()
    }

    /// Bumped whenever chunks land.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Chunks read, chunks waiting, chunks that did not read, region files read.
    pub fn stats(&self) -> (usize, usize, usize, u64) {
        let s = lock(&self.state.0);
        (s.things.len(), s.queue.len(), s.failed, s.regions_read)
    }

    /// Waits (for tests and tools) until nothing is queued, or `timeout` passes.
    pub fn settle(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            let busy = !lock(&self.state.0).queue.is_empty();
            if !busy && self.version.load(Ordering::Acquire) & 1 == 0 {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Survey {
    fn drop(&mut self) {
        {
            let (m, cv) = &*self.state;
            lock(m).stop = true;
            cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The worker: takes the first queued chunk, reads its region file for every queued chunk in the
/// same region and dimension, stores them, and tells the caller. `version` is odd while a region is
/// being read (so [`Survey::settle`] can wait out the read in flight) and moves on by two per
/// region.
fn work(
    world: &Path,
    state: &(Mutex<State>, Condvar),
    version: &AtomicU64,
    every: Duration,
    landed: &(dyn Fn() + Send + Sync),
) {
    let (m, cv) = state;
    let mut last = Instant::now() - every;
    let mut pending = false;
    loop {
        let (dim, rx, rz, chunks) = {
            let mut s = lock(m);
            loop {
                if s.stop {
                    return;
                }
                if !s.queue.is_empty() {
                    break;
                }
                if pending {
                    // Ran dry: say so once.
                    drop(s);
                    landed();
                    pending = false;
                    last = Instant::now();
                    s = lock(m);
                    continue;
                }
                s = cv.wait(s).unwrap_or_else(|p| p.into_inner());
            }
            let (dim, cx, cz) = s.queue.front().cloned().unwrap_or_default();
            let (rx, rz) = (cx >> 5, cz >> 5);
            let mut chunks = Vec::new();
            s.queue.retain(|k| {
                if k.0 == dim && k.1 >> 5 == rx && k.2 >> 5 == rz {
                    chunks.push((k.1, k.2));
                    false
                } else {
                    true
                }
            });
            for &(cx, cz) in &chunks {
                s.queued.remove(&(dim.clone(), cx, cz));
            }
            version.fetch_add(1, Ordering::AcqRel);
            (dim, rx, rz, chunks)
        };
        let dir = region_dir(world, &dim);
        let read = read_region(&dir, rx, rz, &chunks);
        {
            let mut s = lock(m);
            s.regions_read += 1;
            match read {
                Ok((got, bad)) => {
                    s.failed += bad;
                    let got_keys: HashSet<(i32, i32)> = got.iter().map(|(c, _)| *c).collect();
                    for (c, things) in got {
                        s.things.insert((dim.clone(), c.0, c.1), Arc::new(things));
                    }
                    // A chunk that did not parse is kept as read and empty, so it is not asked for
                    // again and again.
                    for c in chunks.iter().filter(|c| !got_keys.contains(c)) {
                        s.things.insert((dim.clone(), c.0, c.1), Arc::new(Vec::new()));
                    }
                }
                Err(_) => {
                    s.failed += chunks.len();
                    for c in &chunks {
                        s.things.insert((dim.clone(), c.0, c.1), Arc::new(Vec::new()));
                    }
                }
            }
        }
        version.fetch_add(1, Ordering::AcqRel);
        pending = true;
        if last.elapsed() >= every {
            landed();
            pending = false;
            last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mic_viewer_surveyor::nbt::write;

    fn c(v: Vec<(&str, Tag)>) -> Tag {
        Tag::Compound(v.into_iter().map(|(k, t)| (k.to_owned(), t)).collect())
    }
    fn st(v: &str) -> Tag {
        Tag::String(v.into())
    }
    fn be(id: &str, p: [i32; 3], more: Vec<(&str, Tag)>) -> Tag {
        let mut v = vec![("id", st(id)), ("x", Tag::Int(p[0])), ("y", Tag::Int(p[1])), ("z", Tag::Int(p[2]))];
        v.extend(more);
        c(v)
    }
    fn net(stress: f64, cap: f64, added: Option<(&str, f64)>) -> Tag {
        let mut v =
            vec![("Id", Tag::Long(7)), ("Stress", Tag::Float(stress as f32)), ("Capacity", Tag::Float(cap as f32))];
        if let Some((k, a)) = added {
            v.push((k, Tag::Float(a as f32)));
        }
        c(v)
    }

    /// A chunk compound with sections to skip and these block entities.
    fn chunk(bes: Vec<Tag>) -> Vec<u8> {
        let root = c(vec![
            ("DataVersion", Tag::Int(3955)),
            (
                "sections",
                Tag::List(vec![c(vec![
                    ("Y", Tag::Byte(4)),
                    ("block_states", c(vec![("data", Tag::LongArray(vec![1; 256]))])),
                ])]),
            ),
            ("block_entities", Tag::List(bes)),
            ("Heightmaps", c(vec![("WORLD_SURFACE", Tag::LongArray(vec![0; 37]))])),
        ]);
        write::root(&root)
    }

    /// A region file holding `chunks` (zlib), at their slots.
    fn region(chunks: &[((i32, i32), Vec<u8>)]) -> Vec<u8> {
        use std::io::Write;
        let mut out = vec![0u8; 8192];
        for ((cx, cz), body) in chunks {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            z.write_all(body).unwrap();
            let z = z.finish().unwrap();
            let sector = out.len() / 4096;
            let mut rec = (z.len() as u32 + 1).to_be_bytes().to_vec();
            rec.push(2);
            rec.extend(z);
            rec.resize(rec.len().div_ceil(4096) * 4096, 0);
            let k = ((cx & 31) + (cz & 31) * 32) as usize * 4;
            out[k] = (sector >> 16) as u8;
            out[k + 1] = (sector >> 8) as u8;
            out[k + 2] = sector as u8;
            out[k + 3] = (rec.len() / 4096) as u8;
            out.extend(rec);
        }
        out
    }

    #[test]
    fn block_entities_come_out_of_a_chunk_and_the_rest_is_skipped() {
        let bytes = chunk(vec![be(
            "create:mechanical_press",
            [1, 64, 2],
            vec![("Speed", Tag::Float(-32.0)), ("Network", net(10.0, 256.0, Some(("AddedStress", 8.0))))],
        )]);
        let bes = block_entities(&bytes).unwrap();
        assert_eq!(bes.len(), 1);
        let t = thing(&bes[0]).unwrap();
        assert_eq!((t.kind, t.pos, t.speed), (Kind::Machine, [1, 64, 2], -32.0));
        assert!(t.running());
        assert_eq!(t.name(), "Mechanical press");
    }

    #[test]
    fn kinds_follow_the_network_and_the_id() {
        let k = |id: &str, n: Option<Tag>| {
            let mut more = vec![("Speed", Tag::Float(16.0))];
            if let Some(n) = n {
                more.push(("Network", n));
            }
            thing(&be(id, [0, 0, 0], more)).unwrap().kind
        };
        assert_eq!(k("create:large_water_wheel", Some(net(0.0, 1536.0, Some(("AddedCapacity", 128.0))))), Kind::Power);
        assert_eq!(k("create:gearbox", Some(net(0.0, 1536.0, None))), Kind::Other);
        assert_eq!(k("othermod:rolling_mill", Some(net(4.0, 1536.0, Some(("AddedStress", 4.0))))), Kind::Machine);
        assert_eq!(k("create:spout", None), Kind::Machine);
        assert_eq!(k("minecraft:chest", None), Kind::Container);
        assert_eq!(k("create:white_postbox", None), Kind::Port);
        // Overstressed: the press keeps its saved speed but stands still.
        let t = thing(&be(
            "create:mechanical_press",
            [0, 0, 0],
            vec![("Speed", Tag::Float(64.0)), ("Network", net(99137.0, 12288.0, Some(("AddedStress", 8.0))))],
        ))
        .unwrap();
        assert!(!t.running());
    }

    #[test]
    fn chains_belts_ports_gauges_and_vaults_keep_what_the_map_needs() {
        let chain = thing(&be(
            "create:chain_conveyor",
            [10, 66, 10],
            vec![
                ("Connections", Tag::List(vec![Tag::IntArray(vec![0, 3, 14])])),
                (
                    "TravellingPackages",
                    Tag::List(vec![c(vec![
                        ("Target", Tag::IntArray(vec![0, 3, 14])),
                        (
                            "Packages",
                            Tag::List(vec![c(vec![
                                ("Position", Tag::Float(2.0)),
                                (
                                    "Item",
                                    c(vec![
                                        ("id", st("create:cardboard_package_12x12")),
                                        ("count", Tag::Int(1)),
                                        ("components", c(vec![("create:package_address", st("Tea"))])),
                                    ]),
                                ),
                            ])]),
                        ),
                    ])]),
                ),
                ("LoopingPackages", Tag::List(vec![])),
            ],
        ))
        .unwrap();
        match &chain.detail {
            Detail::Chain { links, travelling, addresses, .. } => {
                assert_eq!(links, &[[10, 69, 24]]);
                assert_eq!(travelling, &[([10, 69, 24], 1)]);
                assert_eq!(addresses, &["Tea".to_owned()]);
            }
            d => panic!("{d:?}"),
        }
        let frog = thing(&be(
            "create:package_frogport",
            [14, 65, 10],
            vec![("AddressFilter", st("Tea")), ("Target", c(vec![("relative_pos", Tag::IntArray(vec![-4, 1, 0]))]))],
        ))
        .unwrap();
        assert_eq!(frog.detail, Detail::Frogport { address: "Tea".into(), chain: Some([10, 66, 10]) });
        let belt = thing(&be(
            "create:belt",
            [3, 62, 5],
            vec![("Controller", Tag::IntArray(vec![3, 62, 2])), ("Index", Tag::Int(3)), ("Length", Tag::Int(4))],
        ))
        .unwrap();
        assert_eq!(belt.detail, Detail::Belt { controller: [3, 62, 2], index: 3, length: 4 });
        let gauge = thing(&be(
            "create:factory_panel",
            [0, 50, 0],
            vec![(
                "top_left",
                c(vec![
                    ("Filter", c(vec![("id", st("minecraft:charcoal")), ("count", Tag::Int(64))])),
                    ("RecipeAddress", st("CoalStarter")),
                    ("Satisfied", Tag::Byte(1)),
                ]),
            )],
        ))
        .unwrap();
        match &gauge.detail {
            Detail::Gauge(s) => assert_eq!(
                (s[0].item.as_str(), s[0].target, s[0].address.as_str(), s[0].satisfied),
                ("minecraft:charcoal", 64, "CoalStarter", true)
            ),
            d => panic!("{d:?}"),
        }
        let item = |id: &str, n: i32| c(vec![("id", st(id)), ("count", Tag::Int(n)), ("Slot", Tag::Byte(0))]);
        let vault = thing(&be(
            "create:item_vault",
            [5, 63, 5],
            vec![
                ("Controller", Tag::IntArray(vec![5, 63, 5])),
                (
                    "Inventory",
                    c(vec![
                        ("Size", Tag::Int(20)),
                        (
                            "Items",
                            Tag::List(vec![item("minecraft:cobblestone", 64), item("minecraft:cobblestone", 64)]),
                        ),
                    ]),
                ),
            ],
        ))
        .unwrap();
        match &vault.detail {
            Detail::Store { controller, items, capacity, .. } => {
                assert!(controller);
                assert_eq!((items["minecraft:cobblestone"], *capacity), (128, 20));
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn a_region_file_reads_only_the_asked_chunks_and_a_survey_fills_in() {
        let dir = std::env::temp_dir().join(format!("mvc-region-{}", std::process::id()));
        let reg = dir.join("region");
        std::fs::create_dir_all(&reg).unwrap();
        let a = chunk(vec![be("create:mechanical_press", [-16, 64, 0], vec![])]);
        let b = chunk(vec![be("create:belt", [-1, 64, 17], vec![])]);
        std::fs::write(reg.join("r.-1.0.mca"), region(&[((-1, 0), a), ((-1, 1), b)])).unwrap();
        let (got, bad) = read_region(&reg, -1, 0, &[(-1, 0), (-2, 0)]).unwrap();
        assert_eq!(bad, 0);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1[0].pos, [-16, 64, 0]);
        assert!(got[1].1.is_empty(), "a chunk the file lacks reads as empty");
        // A missing region file reads as empty chunks.
        assert!(read_region(&reg, 5, 5, &[(160, 160)]).unwrap().0[0].1.is_empty());

        let calls = Arc::new(AtomicU64::new(0));
        let c2 = Arc::clone(&calls);
        let sv = Survey::new(
            &dir,
            Duration::ZERO,
            Box::new(move || {
                c2.fetch_add(1, Ordering::AcqRel);
            }),
        );
        sv.want("minecraft:overworld", [(-1, 0), (-1, 1)], false);
        assert!(sv.settle(Duration::from_secs(10)));
        assert!(sv.has("minecraft:overworld", -1, 1));
        assert_eq!(sv.at("minecraft:overworld", [-1, 64, 17]).map(|t| t.kind), Some(Kind::Belt));
        let (things, missing) = sv.in_chunks("minecraft:overworld", [(-1, 0), (-1, 1), (0, 0)]);
        assert_eq!((things.len(), missing), (2, 1));
        assert!(calls.load(Ordering::Acquire) >= 1);
        drop(sv);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_furnace_works_when_lit_and_a_spout_has_no_state() {
        let lit = thing(&be("minecraft:furnace", [0, 0, 0], vec![("BurnTime", Tag::Int(120))])).unwrap();
        assert_eq!(lit.active, Some(true));
        assert!(lit.running() && !lit.idle());
        let cold = thing(&be("minecraft:furnace", [1, 0, 0], vec![("BurnTime", Tag::Int(0))])).unwrap();
        assert!(!cold.running() && cold.idle());
        let spout = thing(&be("create:spout", [2, 0, 0], vec![])).unwrap();
        assert_eq!(spout.active, None);
        assert!(spout.running() && !spout.idle());
        let still = thing(&be("create:mechanical_press", [3, 0, 0], vec![("Speed", Tag::Float(0.0))])).unwrap();
        assert!(still.idle());
    }
}
