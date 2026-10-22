// SPDX-License-Identifier: MPL-2.0
// Marks and cards: legAt against Rust's leg_at cases, colours, theme tokens and glyphs, ramps, hit
// testing (trains and outlines too), line geometry, a draw on a recording canvas (what the key is
// told was drawn), swatches, number formatting and sparklines.
//
//     nix develop -c node --test 'tests/web/*.test.js'
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const web = f => path.join(__dirname, '..', '..', 'crates', 'mic-viewer', 'web', f);
require(web('marks.js'));
const MV = require(web('card.js'));

test('legAt agrees with Rust leg_at (api.rs leg_positions)', () => {
  // Two pieces of length 10 and 30: at a quarter of the time, at the end of the first.
  const p = [[0, 0, 10, 0], [10, 0, 10, 30]];
  const leg = (t0, t1) => ({ k: 'leg', p, t0, t1 });
  assert.deepEqual(MV.legAt(leg(100, 500), 50), [0, 0]);
  assert.deepEqual(MV.legAt(leg(100, 500), 200), [10, 0]);
  assert.deepEqual(MV.legAt(leg(100, 500), 300), [10, 10]);
  assert.deepEqual(MV.legAt(leg(100, 500), 900), [10, 30]);
  assert.deepEqual(MV.legAt({ k: 'leg', p: [], t0: 0, t1: 1 }, 0), [0, 0]);
  assert.deepEqual(MV.legAt(leg(100, 100), 100), [10, 30]);
});

test('legAt beyond the Rust cases', () => {
  const p = [[0, 0, 10, 0], [10, 0, 10, 30]];
  assert.deepEqual(MV.legAt({ p, t0: 100, t1: 500 }, 100), [0, 0]);      // at t0: the start
  assert.deepEqual(MV.legAt({ p, t0: 100, t1: 500 }, 500), [10, 30]);    // at t1: the end
  assert.deepEqual(MV.legAt({ p, t0: 100, t1: 100 }, 99), [0, 0]);       // instant leg, before
  assert.deepEqual(MV.legAt({ p: [[5, 5, 5, 5]], t0: 0, t1: 10 }, 5), [5, 5]);   // no length
  const [x, z] = MV.legAt({ p: [[0, 0, 30, 40]], t0: 0, t1: 10 }, 3);
  assert.ok(Math.abs(x - 9) < 1e-9 && Math.abs(z - 12) < 1e-9);
});

test('parseColor, rgba, lighten', () => {
  assert.deepEqual(MV.parseColor('#abc'), [170, 187, 204]);
  assert.deepEqual(MV.parseColor('#112233'), [17, 34, 51]);
  assert.deepEqual(MV.parseColor('#112233ff'), [17, 34, 51]);
  assert.deepEqual(MV.parseColor('red'), [136, 136, 136]);
  assert.deepEqual(MV.parseColor('rgba(14, 16, 19, 0.9)'), [14, 16, 19]);
  assert.deepEqual(MV.parseColor('rgb(1,2,3)'), [1, 2, 3]);
  assert.equal(MV.mix('#000000', '#ffffff', 0.5), 'rgb(128, 128, 128)');
  assert.equal(MV.rgba('#ff0000', 0.5), 'rgba(255, 0, 0, 0.5)');
  assert.equal(MV.lighten('#000000', 0.5), 'rgb(128, 128, 128)');
});

test('colour and glyph: the mark, else its legend entry, else the layer style', () => {
  const desc = { style: { color: '#123456', glyph: 'diamond' }, legend: [{ color: '#aaaaaa', glyph: 'star' }, { color: '#bbbbbb' }] };
  assert.equal(MV.colorOf({ c: '#111111', ci: 0 }, desc), '#111111');
  assert.equal(MV.colorOf({ ci: 1 }, desc), '#bbbbbb');
  assert.equal(MV.colorOf({ ci: 9 }, desc), '#123456');
  assert.equal(MV.colorOf({}, desc), '#123456');
  assert.equal(MV.colorOf({}, {}), '#cccccc');
  assert.equal(MV.glyphOf({ g: 'square', ci: 0 }, desc), 'square');
  assert.equal(MV.glyphOf({ ci: 0 }, desc), 'star');
  assert.equal(MV.glyphOf({ ci: 1 }, desc), 'diamond');
  assert.equal(MV.glyphOf({}, null), 'circle');
});

test('theme tokens resolve against the page theme, light by default', () => {
  assert.equal(MV.resolve('ink', null), '#1f2328');
  assert.equal(MV.resolve('paper', { dark: true }), '#14171b');
  assert.equal(MV.resolve('ink', { dark: true, ink: '#eeeeee' }), '#eeeeee');
  assert.equal(MV.resolve('#123456', { dark: true }), '#123456');
  const th = { dark: false, ink: '#000001' };
  assert.equal(MV.themeOf(th), MV.themeOf(th));            // cached per theme object
  assert.equal(MV.themeOf(th).halo, MV.THEMES.light.halo); // filled in from the defaults
});

test('line style: the mark, else the layer', () => {
  const desc = { style: { line: 'rail' } };
  assert.equal(MV.lineOf({}, desc), 'rail');
  assert.equal(MV.lineOf({ st: 'flow' }, desc), 'flow');
  assert.equal(MV.lineOf({}, { style: {} }), null);
});

test('rampColor runs along the stops', () => {
  assert.deepEqual(MV.rampColor('viridis', 0), [68, 1, 84]);
  assert.deepEqual(MV.rampColor('viridis', 1), [253, 231, 37]);
  assert.deepEqual(MV.rampColor('greys', 0.5), [128, 128, 128]);
  assert.deepEqual(MV.rampColor('no such ramp', 0), [68, 1, 84]);
  assert.deepEqual(MV.rampColor('heat', NaN), MV.rampColor('heat', 0));
  assert.deepEqual(MV.rampColor('heat', 7), MV.rampColor('heat', 1));
});

test('compact counts for cluster bubbles', () => {
  assert.equal(MV.compact(950), '950');
  assert.equal(MV.compact(1000), '1k');
  assert.equal(MV.compact(1234), '1.2k');
  assert.equal(MV.compact(12345), '12k');
  assert.equal(MV.compact(3.4e6), '3.4M');
  assert.equal(MV.compact(-1500), '-1.5k');
});

test('xExtent spans what a mark covers east to west', () => {
  assert.deepEqual(MV.xExtent({ k: 'pt', x: 4, z: 1 }), [4, 4]);
  assert.deepEqual(MV.xExtent({ k: 'leg', p: [[0, 0, 10, 0], [10, 0, -5, 3]] }), [-5, 10]);
  assert.deepEqual(MV.xExtent({ k: 'path', p: [[3, 0], [-2, 1], [8, 2]] }), [-2, 8]);
  assert.deepEqual(MV.xExtent({ k: 'edge', a: [9, 0], b: [1, 0] }), [1, 9]);
  assert.deepEqual(MV.xExtent({ k: 'area', x: 10, z: 0, r: 5 }), [5, 15]);
  assert.deepEqual(MV.xExtent({ k: 'area', box: [1, 2, 3, 4] }), [1, 3]);
  assert.deepEqual(MV.xExtent({ k: 'cell', x0: 100, size: 16, w: 4 }), [100, 164]);
  assert.deepEqual(MV.xExtent({ k: 'train', cars: [[10, 0, 4, 0], [3, 0, -6, 1]] }), [-6, 10]);
  assert.deepEqual(MV.xExtent({ k: 'area', rings: [[[0, 0], [16, 0], [16, 16], [0, 16]]] }), [0, 16]);
});

test('distToSegment', () => {
  assert.equal(MV.distToSegment(5, 5, 0, 0, 10, 0), 5);
  assert.equal(MV.distToSegment(-3, 4, 0, 0, 10, 0), 5);
  assert.equal(MV.distToSegment(13, 4, 0, 0, 10, 0), 5);
  assert.equal(MV.distToSegment(3, 4, 0, 0, 0, 0), 5);
});

test('pick: glyphs on top, then the nearest line, then the smallest area, then cells', () => {
  const targets = [
    { kind: 'box', x0: 0, y0: 0, x1: 100, y1: 100, id: 'big' },
    { kind: 'box', x0: 40, y0: 30, x1: 60, y1: 60, id: 'small' },
    { kind: 'disc', x: 200, y: 200, r: 30, id: 'disc' },
    { kind: 'seg', ax: 0, ay: 50, bx: 100, by: 50, w: 2, id: 'road' },
    { kind: 'poly', pts: [0, 80, 50, 80, 100, 90], w: 4, id: 'route' },
    { kind: 'pt', x: 50, y: 50, r: 5, id: 'a' },
    { kind: 'pt', x: 52, y: 50, r: 5, id: 'b' },
    { kind: 'cell', x0: 300, y0: 0, cs: 10, w: 2, h: 2, v: [1, null, 3, 4], id: 'heat' },
  ];
  const id = (x, y) => { const t = MV.pick(targets, x, y); return t && t.id; };
  assert.equal(id(50, 50), 'b');            // the one drawn last
  assert.equal(id(20, 53), 'road');         // within half its width and the slop
  assert.equal(id(45, 40), 'small');        // nine px off the road: the smaller box
  assert.equal(id(10, 10), 'big');
  assert.equal(id(25, 83), 'route');
  assert.equal(id(210, 210), 'disc');
  assert.equal(id(150, 150), null);
  assert.equal(id(315, 5), null);           // an empty cell
  const cell = MV.pick(targets, 305, 15);
  assert.equal(cell.id, 'heat');
  assert.equal(cell.value, 3);
});

test('pick: a train over the track it stands on, an outline by its inside', () => {
  const targets = [
    { kind: 'poly', pts: [0, 50, 200, 50], w: 3, id: 'track' },
    { kind: 'train', pts: [80, 50, 60, 50, 58, 50, 40, 50], w: 6, id: 'train' },
    // An L of two chunks with a hole-free outline, and a square inside a bigger one (even-odd).
    { kind: 'rings', rings: [[300, 0, 340, 0, 340, 20, 320, 20, 320, 40, 300, 40]], x0: 300, y0: 0, x1: 340, y1: 40, area: 1200, id: 'L' },
  ];
  const id = (x, y) => { const t = MV.pick(targets, x, y); return t && t.id; };
  assert.equal(id(70, 51), 'train');
  assert.equal(id(150, 51), 'track');
  assert.equal(id(335, 10), 'L');
  assert.equal(id(335, 35), null);        // in the bounds, outside the L
  assert.ok(MV.inRings(5, 5, [[0, 0, 10, 0, 10, 10, 0, 10]]));
  assert.ok(!MV.inRings(5, 5, [[0, 0, 10, 0, 10, 10, 0, 10], [2, 2, 8, 2, 8, 8, 2, 8]]));
});

test('walk, offsetFlat and flatLength follow a polyline', () => {
  const f = [0, 0, 10, 0, 10, 10];
  assert.equal(MV.flatLength(f), 20);
  const at = [];
  MV.walk(f, 5, 2.5, (x, y, ux, uy) => at.push([x, y, ux, uy]));
  assert.deepEqual(at, [[2.5, 0, 1, 0], [7.5, 0, 1, 0], [10, 2.5, 0, 1], [10, 7.5, 0, 1]]);
  const capped = [];
  MV.walk(f, 1, 0, () => capped.push(1), 3);
  assert.equal(capped.length, 3);
  // Left of the way it runs (y down): up for an eastward run; the corner mitred to (9, -1)... for
  // d = 1 the corner of an east-then-south run sits at (11, -1).
  const off = MV.offsetFlat(f, 1);
  assert.deepEqual(off.slice(0, 2), [0, -1]);
  assert.ok(Math.abs(off[2] - 11) < 1e-9 && Math.abs(off[3] + 1) < 1e-9);
  assert.deepEqual(off.slice(4), [11, 10]);
});

/** A canvas 2D context that takes every call and remembers the text drawn. */
function recordingCtx() {
  const texts = [];
  const target = { texts, measureText: s => ({ width: String(s).length * 6 }), fillText: s => texts.push(String(s)) };
  return new Proxy(target, {
    get: (o, k) => (k in o ? o[k] : () => {}),
    set: (o, k, v) => { o[k] = v; return true; },
  });
}

test('drawMarks draws every kind, reports the legend entries drawn and places labels', () => {
  const map = { view: { cx: 0, cz: 0, bpp: 0.2, w: 800, h: 600 }, copies: () => [0] };
  const track = { id: 'rail.track', style: { color: '#2f6db5', width: 2, line: 'rail' }, legend: [{ label: 'Network 1', color: '#2f6db5', line: 'rail' }] };
  const stations = { id: 'rail.stations', style: { color: 'ink', glyph: 'station', size: 7, label_bpp: 24 }, legend: [{ label: 'Station', color: 'ink', glyph: 'station' }], priority: 85 };
  const trains = { id: 'rail.trains', kinds: ['train'], style: { color: 'ink', label_bpp: 4 }, legend: [{ label: 'Running', color: 'ink' }, { label: 'Standing', color: 'ink', st: 'off' }] };
  const chains = { id: 'logi.chains', style: { color: '#b7791f', width: 1.5, line: 'chain' }, legend: [{ label: 'Chain', color: '#b7791f', line: 'chain' }, { label: 'Packages', color: '#b7791f', glyph: 'dot' }] };
  const districts = { id: 'industry.districts', style: { color: '#2a7f7a' }, legend: [] };
  const list = [
    { desc: districts, data: { marks: [{ k: 'area', id: 'd1', box: [-40, -40, 40, 40], rings: [[[-40, -40], [40, -40], [40, 40], [-40, 40]]], l: 'Iron works', st: 'dash' }] } },
    { desc: track, data: { marks: [{ k: 'path', id: '0:0', ci: 0, p: [[-50, 0], [0, 0], [30, 20]] }] } },
    { desc: chains, data: { marks: [{ k: 'path', id: '1,2,3', p: [[-10, -10], [10, -10]], n: 3 }] } },
    { desc: stations, data: { marks: [{ k: 'point', id: 's', x: 0, z: 0, a: 0, l: 'Center 5', ci: 0 }] } },
    { desc: trains, data: { marks: [{ k: 'train', id: 't', cars: [[20, 0, 12, 0], [11, 0, 3, 0]], l: 'Ore runner', ci: 1, st: 'off' }] } },
  ];
  const ctx = recordingCtx();
  const out = MV.drawMarks(ctx, map, list, { theme: { dark: true }, selected: { layer: 'rail.trains', id: 't' } });
  assert.equal(out.drawn, 5);
  assert.deepEqual([...out.seen.get('rail.track')], [0]);
  assert.deepEqual([...out.seen.get('logi.chains')].sort(), [-1, 1]);   // no ci: the style; packages: the dot entry
  assert.deepEqual([...out.seen.get('rail.trains')], [1]);
  assert.ok(ctx.texts.includes('Center 5'));
  assert.ok(ctx.texts.includes('Ore runner'));
  assert.ok(ctx.texts.includes('Iron works'));
  const kinds = out.targets.map(t => t.kind).sort();
  assert.deepEqual(kinds, ['poly', 'poly', 'pt', 'rings', 'train']);
  // Far out the train is a chevron, a point target, and labels give way to the zoom.
  map.view.bpp = 10;
  const far = recordingCtx();
  const out2 = MV.drawMarks(far, map, list, {});
  assert.equal(out2.targets.find(t => t.id === 't').kind, 'pt');
  assert.ok(!far.texts.includes('Ore runner'));
  assert.ok(far.texts.includes('Center 5'));
});

test('a train line runs right of the way it goes, clear of the track it follows', () => {
  // Track drawn from (-50, 0) east; a line the same way sits below it (right of an eastward run,
  // y down), one the other way above it, and both clear of the track's drawn half width.
  const map = { view: { cx: 0, cz: 0, bpp: 0.5, w: 400, h: 300 }, copies: () => [0] };
  const lines = { id: 'industry.lines', style: { color: '#2a9d8f', line: 'flow', width: 3 }, legend: [] };
  const list = [{ desc: lines, data: { marks: [
    { k: 'path', id: 'east', p: [[-50, 0], [50, 0]], w: 3 },
    { k: 'path', id: 'west', p: [[50, 0], [-50, 0]], w: 3 },
  ] } }];
  const out = MV.drawMarks(recordingCtx(), map, list, {});
  const y = id => out.targets.find(t => t.id === id).pts[1] - 150;
  const clear = MV.railHalf(0.5) + 1.5;
  assert.ok(y('east') > clear, `east line at ${y('east')}`);
  assert.ok(y('west') < -clear, `west line at ${y('west')}`);
  assert.equal(MV.flowOffset(0.5, 3), MV.railHalf(0.5) + 1.5 + 2);
});

test('simplifyFlat drops crowded points and keeps both ends', () => {
  assert.deepEqual(MV.simplifyFlat([0, 0, 0.5, 0, 1, 0, 3, 0, 3.2, 0], 1.5), [0, 0, 3.2, 0]);
  assert.deepEqual(MV.simplifyFlat([0, 0, 2, 0, 4, 0, 6, 0], 1.5), [0, 0, 2, 0, 4, 0, 6, 0]);
  assert.deepEqual(MV.simplifyFlat([0, 0, 9, 9], 50), [0, 0, 9, 9]);
});

test('boxGrid finds overlaps and forgets boxes off screen', () => {
  const g = MV.boxGrid(200, 100, 32);
  g.add([10, 10, 40, 30]);
  g.add([-50, 10, -5, 30]);          // off the left edge: not kept
  assert.equal(g.size, 1);
  assert.ok(g.hits([35, 25, 60, 50]));
  assert.ok(!g.hits([41, 10, 80, 30]));
  assert.ok(!g.hits([-40, 10, -10, 30]));
});

test('clearOf moves a circle the least way out of the others', () => {
  assert.deepEqual(MV.clearOf([], 5, 5, 3), [5, 5]);
  const [x, y] = MV.clearOf([[0, 0, 10]], 4, 0, 10);
  assert.ok(Math.abs(x - 22) < 1e-9 && Math.abs(y) < 1e-9);
  const at = MV.clearOf([[0, 0, 8], [40, 30, 8]], 6, 6, 8);
  for (const [bx, by, br] of [[0, 0, 8], [40, 30, 8]]) assert.ok(Math.hypot(at[0] - bx, at[1] - by) >= 8 + br + 2 - 1e-6);
});

test('labels stay wholly on screen and off the symbols, and the draw counts what it showed', () => {
  // A station hard against the right edge with a machine on its left: its name goes where it is
  // wholly on screen and clear of the machine.
  const map = { view: { cx: 0, cz: 0, bpp: 1, w: 300, h: 200 }, copies: () => [0] };
  const stations = { id: 'rail.stations', style: { color: 'ink', glyph: 'station', size: 7, label_bpp: 24 }, legend: [], priority: 85 };
  const machines = { id: 'industry.machines', style: { color: 'ink', glyph: 'circle', size: 6 }, legend: [] };
  const ctx = recordingCtx();
  let t = [0, 0];
  const placed = [];
  ctx.translate = (x, y) => { t = [x, y]; };
  ctx.fillText = s => { placed.push({ s: String(s), x: t[0], y: t[1], align: ctx.textAlign }); };
  const list = [
    { desc: machines, data: { marks: [{ k: 'point', id: 'm', x: 120, z: 0 }, { k: 'point', id: 'far', x: 4000, z: 0 }] } },
    { desc: stations, data: { marks: [{ k: 'point', id: 's', x: 140, z: 0, l: 'Edge town' }] } },
  ];
  const out = MV.drawMarks(ctx, map, list, { theme: null });
  const at = placed.find(p => p.s === 'Edge town');
  assert.ok(at, 'the name is placed');
  const tw = 'Edge town'.length * 6;
  const x0 = at.align === 'left' ? at.x : at.align === 'right' ? at.x - tw : at.x - tw / 2;
  assert.ok(x0 >= 0 && x0 + tw <= 300 && at.y >= 7 && at.y <= 193, `name at ${x0}, ${at.y}`);
  assert.ok(!(x0 < 274 && x0 + tw > 266 && at.y - 7 < 104 && at.y + 7 > 96), `name over the machine at ${x0}, ${at.y}`);
  // A panel over everything left of the station: no room for the name anywhere.
  placed.length = 0;
  MV.drawMarks(ctx, map, list, { covered: [[0, 0, 284, 200]] });
  assert.ok(!placed.some(p => p.s === 'Edge town'), 'no name under a panel');
  assert.equal(out.counts.get('industry.machines'), 1);   // the far one is not on screen
  assert.equal(out.counts.get('rail.stations'), 1);
  assert.equal(out.clusters, 0);
  // Two count bubbles on one spot are pushed apart, and counted.
  const groups = { id: 'industry.machines', style: { color: 'ink' }, legend: [] };
  const two = MV.drawMarks(recordingCtx(), map, [{ desc: groups, data: { marks: [
    { k: 'cluster', id: 'c1', x: 0, z: 0, n: 40 }, { k: 'cluster', id: 'c2', x: 1, z: 0, n: 7 },
  ] } }], {});
  assert.equal(two.clusters, 2);
  assert.equal(two.counts.get('industry.machines'), 47);
  const [a, b] = two.targets.filter(t => t.cluster);
  assert.ok(Math.hypot(a.x - b.x, a.y - b.y) > 12, 'bubbles apart');
});

test('drawSwatch draws every legend entry and layer without throwing', () => {
  const descs = [
    { kinds: ['path'], style: { color: '#2f6db5', line: 'rail', width: 2 }, legend: [{ label: 'n', color: '#2f6db5', line: 'rail' }] },
    { kinds: ['path'], style: { color: 'ink', line: 'belt' }, legend: [{ label: 'b', color: 'ink', line: 'belt' }] },
    { kinds: ['train'], style: { color: 'ink' }, legend: [{ label: 'r', color: 'ink', glyph: 'train' }, { label: 'd', color: '#d1242f', glyph: 'train', st: 'bad' }] },
    { kinds: ['point'], style: { color: 'ink', glyph: 'station' }, legend: [{ label: 's', color: 'ink', glyph: 'station', st: 'warn' }, { label: 'g', color: '#2da44e', glyph: 'gauge' }, { label: 'p', color: '#b7791f', glyph: 'port' }, { label: 'x', color: 'ink', glyph: 'circle', st: 'off' }, { label: 'sig', color: '#d1242f', glyph: 'signal' }] },
    { kinds: ['area'], style: { color: '#2a7f7a' }, legend: [{ label: 'a', color: '#2a7f7a', st: 'dot' }] },
    { kinds: ['cell'], style: { color: '#888' } },
    { kinds: ['label'], style: { color: '#888' } },
    { kinds: ['cluster'], style: { color: 'ink' }, legend: [{ label: 'c', color: 'ink', n: 5 }] },
  ];
  for (const d of descs) {
    MV.drawSwatch(recordingCtx(), d, null, 22, 14, { dark: true });
    (d.legend || []).forEach((_, i) => MV.drawSwatch(recordingCtx(), d, i, 22, 14, null));
  }
  const names = recordingCtx();
  MV.drawSwatch(names, { kinds: ['label'], style: {} }, null, 22, 14, null);
  assert.deepEqual(names.texts, ['Aa']);
});

test('fmtNum: thousands, decimals by size, exponents at the extremes', () => {
  const cases = [
    [1234567, '1,234,567'], [1234.5, '1,235'], [123.44, '123.4'], [12.3456, '12.35'], [1.04, '1.04'],
    [1.5, '1.5'], [2, '2'], [0.012345, '0.0123'], [-0.4, '-0.4'], [0, '0'], [-0, '0'],
    [-1234567, '-1,234,567'], [2e15, '2.00e15'], [-1e-7, '-1.00e-7'], ['42', '42'],
  ];
  for (const [v, want] of cases) assert.equal(MV.fmtNum(v), want, `fmtNum(${v})`);
  assert.equal(MV.fmtNum(NaN), '-');
  assert.equal(MV.fmtNum(Infinity), 'inf');
  assert.equal(MV.fmtNum(-Infinity), '-inf');
});

test('fmtValue puts units after numbers', () => {
  assert.equal(MV.fmtValue(12.5, '%'), '12.5%');
  assert.equal(MV.fmtValue(566, 'coins'), '566 coins');
  assert.equal(MV.fmtValue(0.92, 'x anchor'), '0.92 x anchor');
  assert.equal(MV.fmtValue(1200, null), '1,200');
  assert.equal(MV.fmtValue('Holtdun Clan'), 'Holtdun Clan');
  assert.equal(MV.fmtValue(null), '');
});

test('sparkPath scales points into the box', () => {
  const s = MV.sparkPath([[2, 5], [0, 0], [1, 10]], 100, 20, 0);
  assert.equal(s.line, 'M0 20 L50 0 L100 10');
  assert.equal(s.area, 'M0 20 L50 0 L100 10 L100 20 L0 20 Z');
  assert.equal(s.min, 0);
  assert.equal(s.max, 10);
  assert.deepEqual(s.first, [0, 0]);
  assert.deepEqual(s.last, [2, 5]);
  assert.equal(MV.sparkPath([[0, 3], [1, 3]], 100, 20, 0).line, 'M0 10 L100 10');   // flat: the middle
  assert.equal(MV.sparkPath([], 100, 20).line, '');
  assert.equal(MV.sparkPath([[0, NaN], [1, 2]], 10, 10, 0).line, 'M5 5');          // one point
});
