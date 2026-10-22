// SPDX-License-Identifier: MPL-2.0
// The page's and the timeline's pure parts: game time and wall dates, axis ticks, stacked
// histogram buckets, deep links, zoom ranges, marks requests, stream versions and freshness.
//
//     nix develop -c node --test 'tests/web/*.test.js'
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const web = f => path.join(__dirname, '..', '..', 'crates', 'mic-viewer', 'web', f);
require(web('timeline.js'));
require(web('marks.js'));
require(web('card.js'));
const MV = require(web('app.js'));   // boot() needs a document: under node only the helpers load

const local = (mo, d, h, mi) => new Date(2026, mo, d, h, mi).getTime();

test('game day and clock: tick 0 is day 0, 06:00', () => {
  assert.equal(MV.gameDay(0), 0);
  assert.equal(MV.gameDay(23999), 0);
  assert.equal(MV.gameDay(960000), 40);
  assert.equal(MV.gameClock(0), '06:00');
  assert.equal(MV.gameClock(6000), '12:00');
  assert.equal(MV.gameClock(18000), '00:00');
  assert.equal(MV.gameClock(23999), '05:59');
  assert.equal(MV.gameClock(-1000), '05:00');
  assert.equal(MV.fmtGameTime(960000), 'Day 40, 06:00');
  assert.equal(MV.fmtGameTime(960000 + 15500), 'Day 40, 21:30');
  assert.equal(MV.fmtGameTime(1200, 2400), 'Day 0, 18:00');   // a short day
});

test('fmtSpan', () => {
  assert.equal(MV.fmtSpan(240000), '10 days');
  assert.equal(MV.fmtSpan(72000), '3 days');
  assert.equal(MV.fmtSpan(28000), '1 day 4 h');
  assert.equal(MV.fmtSpan(24000), '1 day');
  assert.equal(MV.fmtSpan(1000), '1 h');
  assert.equal(MV.fmtSpan(1750), '1 h 45 min');
  assert.equal(MV.fmtSpan(750), '45 min');
  assert.equal(MV.fmtSpan(-750), '45 min');
});

test('wallAt: linear between pairs, the nearest slope outside, 50 ms a tick with one pair', () => {
  assert.equal(MV.wallAt(null, 5), null);
  assert.equal(MV.wallAt([], 5), null);
  assert.equal(MV.wallAt([[100, 1000]], 120), 2000);
  const two = [[0, 0], [100, 1000]];
  assert.equal(MV.wallAt(two, 50), 500);
  assert.equal(MV.wallAt(two, 200), 2000);
  assert.equal(MV.wallAt(two, -100), -1000);
  const three = [[0, 0], [100, 1000], [200, 3000]];
  assert.equal(MV.wallAt(three, 150), 2000);
  assert.equal(MV.wallAt(three, 300), 5000);
  assert.equal(MV.wallAt([[10, 7], [10, 9]], 10), 7);   // a stalled clock
});

test('fmtWall and wallLabels', () => {
  assert.equal(MV.fmtWall(local(9, 21, 14, 5), true), 'Oct 21 14:05');
  assert.equal(MV.fmtWall(local(9, 21, 14, 5), false), 'Oct 21');
  assert.equal(MV.fmtWall(null), '');
  // Hours apart: the date once, then clocks, the date again past midnight.
  assert.deepEqual(MV.wallLabels([local(9, 20, 20, 40), local(9, 20, 22, 20), null, local(9, 21, 0, 0), local(9, 21, 1, 40)]),
    ['Oct 20 20:40', '22:20', '', 'Oct 21 00:00', '01:40']);
  // Days apart: dates, each once.
  assert.deepEqual(MV.wallLabels([local(9, 20, 6, 0), local(9, 21, 6, 0), local(9, 22, 6, 0)]), ['Oct 20', 'Oct 21', 'Oct 22']);
  assert.deepEqual(MV.wallLabels([local(9, 20, 0, 0), local(9, 20, 21, 0), local(9, 21, 18, 0)]), ['Oct 20', '', 'Oct 21']);
  assert.deepEqual(MV.wallLabels([local(9, 20, 9, 0)]), ['Oct 20']);
  assert.deepEqual(MV.wallLabels([]), []);
});

test('axisTicks picks the first step that leaves room', () => {
  const a = MV.axisTicks(0, 240000, 1 / 1000, 84, 24000);   // 24 px a day
  assert.equal(a.step, 120000);
  assert.deepEqual(a.ticks, [0, 120000, 240000]);
  const b = MV.axisTicks(500, 5500, 0.1, 84, 24000);         // 100 px a game hour
  assert.equal(b.step, 1000);
  assert.deepEqual(b.ticks, [1000, 2000, 3000, 4000, 5000]);
  assert.equal(MV.axisTicks(0, 1e9, 1e-9, 84, 24000).step, 24000000);   // the largest step
});

test('stackBuckets stacks shown kinds in order', () => {
  const series = { a: [1, 0, 2], b: [0, 3, 1], c: [5] };
  const s = MV.stackBuckets(series, ['b', 'a'], new Set(['c']));
  assert.deepEqual(s.stacks, [[['a', 1]], [['b', 3]], [['b', 1], ['a', 2]]]);
  assert.deepEqual(s.totals, [1, 3, 3]);
  assert.equal(s.max, 3);
  const all = MV.stackBuckets(series, ['b', 'a'], null);   // a kind not in the order goes last
  assert.deepEqual(all.stacks[0], [['a', 1], ['c', 5]]);
  assert.equal(all.max, 6);
  assert.deepEqual(MV.stackBuckets({}, ['a'], null), { stacks: [], totals: [], max: 0 });
});

test('parseHash and buildHash', () => {
  const h = '#frame=minecraft:overworld&x=1000&z=-500&bpp=2&layers=settle.villages,travel.roads&t=960000&card=settle.villages/v%2F12';
  const s = MV.parseHash(h);
  assert.deepEqual(s, {
    frame: 'minecraft:overworld', x: 1000, z: -500, bpp: 2, layers: ['settle.villages', 'travel.roads'], t: 960000,
    card: { layer: 'settle.villages', id: 'v/12' },
  });
  assert.equal(MV.buildHash(s), h);
  assert.deepEqual(MV.parseHash(''), { frame: null, x: null, z: null, bpp: null, layers: null, t: null, card: null });
  const bad = MV.parseHash('x=abc&z=&bpp=-1&t=1e3&card=nolayer&%E0%A4%A=1&layers=');
  assert.equal(bad.x, null);
  assert.equal(bad.z, null);
  assert.equal(bad.bpp, null);
  assert.equal(bad.t, 1000);
  assert.equal(bad.card, null);
  assert.deepEqual(bad.layers, []);
  assert.equal(MV.buildHash({ frame: 'demo:ring', x: 2048.4, z: -0.6, bpp: 1.23456, layers: ['a', 'b'], t: null, card: null }),
    '#frame=demo:ring&x=2048&z=-1&bpp=1.235&layers=a,b');
});

test('inZoom and zoomHint follow min_bpp and max_bpp', () => {
  const names = { min_bpp: 1, max_bpp: 6 };
  assert.equal(MV.inZoom(names, 6), true);
  assert.equal(MV.inZoom(names, 6.1), false);
  assert.equal(MV.inZoom(names, 0.5), false);
  assert.equal(MV.inZoom({}, 1e6), true);
  assert.equal(MV.zoomHint(names, 10), 'in');
  assert.equal(MV.zoomHint(names, 0.5), 'out');
  assert.equal(MV.zoomHint(names, 3), null);
});

test('viewBox and marksURL', () => {
  const v = { cx: 0, cz: 0, bpp: 2, w: 100, h: 50 };
  assert.deepEqual(MV.viewBox(v, 0), [-100, -50, 100, 50]);
  assert.deepEqual(MV.viewBox(v, 0.25), [-150, -75, 150, 75]);
  assert.equal(MV.marksURL('demo:ring', [-1.5, 2.5, 10.2, 20], 1.23456789, ['a.b', 'c'], null),
    'api/marks?frame=demo%3Aring&x0=-2&z0=2&x1=11&z1=20&bpp=1.23457&layers=a.b,c');
  assert.ok(MV.marksURL('f', [0, 0, 1, 1], 1, ['a'], 960000.4).endsWith('&t=960000'));
});

test('changedLayers: held layers whose version moved on in this frame', () => {
  const held = { a: { version: 1 }, b: { version: 1 }, c: { version: 1 } };
  assert.deepEqual(MV.changedLayers({ frames: { f: { a: 2, b: 1 } } }, 'f', held), ['a']);
  assert.deepEqual(MV.changedLayers({ frames: { g: { a: 2 } } }, 'f', held), []);
  assert.deepEqual(MV.changedLayers({}, 'f', held), []);
  assert.deepEqual(MV.changedLayers({ frames: { f: { z: 9 } } }, 'f', held), []);
});

test('rowsForFrame: stream rows go to their frame, a row without one to the first', () => {
  const rows = [{ t: 1 }, { t: 2, frame: 'ring' }, { t: 3, frame: 'over' }];
  assert.deepEqual(MV.rowsForFrame(rows, 'over', 'over').map(r => r.t), [1, 3]);
  assert.deepEqual(MV.rowsForFrame(rows, 'ring', 'over').map(r => r.t), [2]);
  assert.deepEqual(MV.rowsForFrame(rows, 'nether', 'over'), []);
  assert.deepEqual(MV.rowsForFrame(undefined, 'over', 'over'), []);
});

test('emptyReason: only a shown layer in zoom with no marks at all', () => {
  const why = { total: 0, shown: 0, marks: [], empty: 'no ships module in this save' };
  assert.equal(MV.emptyReason(true, null, why), 'no ships module in this save');
  assert.equal(MV.emptyReason(false, null, why), null);
  assert.equal(MV.emptyReason(true, 'in', why), null);
  assert.equal(MV.emptyReason(true, null, { ...why, total: 3 }), null);
  assert.equal(MV.emptyReason(true, null, { total: 0 }), null);
  assert.equal(MV.emptyReason(true, null, undefined), null);
});

test('tilesShown: saved choice, then default_on, then the first base map and every overlay', () => {
  const tiles = [{ id: 'a' }, { id: 'b' }, { id: 'o', overlay: true }, { id: 'p', overlay: true, default_on: false },
    { id: 'c', default_on: true }];
  assert.deepEqual(MV.tilesShown(tiles, {}), { a: true, b: false, o: true, p: false, c: true });
  assert.deepEqual(MV.tilesShown(tiles, { a: false, p: true, c: false }), { a: false, b: false, o: true, p: true, c: false });
  assert.deepEqual(MV.tilesShown([{ id: 'a', default_on: false }, { id: 'b' }], null), { a: false, b: false });
  assert.deepEqual(MV.tilesShown(undefined, {}), {});
});

test('isStale, tickAt, fmtAge', () => {
  assert.equal(MV.isStale(null, 1e9), false);
  assert.equal(MV.isStale({ age_ms: 500 }, 1e9), false);   // no cycle: never stale
  assert.equal(MV.isStale({ cycle_ms: 1000, age_ms: 500 }, 1600), true);
  assert.equal(MV.isStale({ cycle_ms: 1000, age_ms: 500 }, 1400), false);
  const live = { tick: 100, tps: 20, running: true };
  assert.equal(MV.tickAt(live, 1500, false), 130);
  assert.equal(MV.tickAt(live, 1500, true), 100);
  assert.equal(MV.tickAt(live, -50, false), 100);
  assert.equal(MV.tickAt({ tick: 960000, tps: 0, running: false }, 5000, false), 960000);
  assert.equal(MV.tickAt(null, 10, false), 0);
  assert.equal(MV.fmtAge(1000), 'just now');
  assert.equal(MV.fmtAge(12000), '12 s ago');
  assert.equal(MV.fmtAge(240000), '4 min ago');
  assert.equal(MV.fmtAge(3 * 3600e3), '3 h ago');
  assert.equal(MV.fmtAge(2 * 86400e3), '2 days ago');
  assert.equal(MV.fmtAge(null), '');
});

test('fmtDayAt and fmtSourceTime: the header line', () => {
  assert.equal(MV.fmtDayAt(960000, 24000), 'day 40 at 06:00');
  assert.equal(MV.fmtSourceTime('demo', 960000, 24000), 'Demo, day 40 at 06:00');
  assert.equal(MV.fmtSourceTime('save', 3467 * 24000 + 5000, 24000), 'Save, day 3,467 at 11:00');
  assert.equal(MV.fmtSourceTime('live', null, 24000), 'Live');
  assert.equal(MV.fmtSourceTime('replay', null, 24000), 'Replay');
  assert.equal(MV.fmtSourceTime(undefined, null, 24000), 'Save');
});

test('fmtStatus: the header line as sentences, the source not said twice', () => {
  const tick = 3467 * 24000 + 5000;
  assert.deepEqual(MV.fmtStatus('save', tick, 24000, 'a save: nothing moves'), ['Save, day 3,467 at 11:00.', 'Nothing moves.']);
  assert.deepEqual(MV.fmtStatus('save', tick, 24000, 'nothing moves'), ['Save, day 3,467 at 11:00.', 'Nothing moves.']);
  assert.deepEqual(MV.fmtStatus('live', tick, 24000, 'updated 3 s ago'), ['Live, day 3,467 at 11:00.', 'Updated 3 s ago.']);
  assert.deepEqual(MV.fmtStatus('live', null, 24000, 'reconnecting'), ['Live.', 'Reconnecting.']);
  assert.deepEqual(MV.fmtStatus('save', tick, 24000, ''), ['Save, day 3,467 at 11:00', '']);
  assert.deepEqual(MV.fmtStatus('save', tick, 24000, 'Saved by hand!'), ['Save, day 3,467 at 11:00.', 'Saved by hand!']);
  assert.equal(MV.sentence('  '), '');
});

test('layerCount: what is on screen, the total for cells, nothing before a draw', () => {
  const pts = { kinds: ['point'] }, cells = { kinds: ['cell'] };
  assert.deepEqual(MV.layerCount(pts, { total: 900, shown: 900 }, 12), { text: '12', title: '12 on screen' });
  assert.equal(MV.layerCount(pts, { total: 900, shown: 900, clustered: true }, 1234).title, '1,234 on screen, grouped where they crowd');
  assert.match(MV.layerCount(pts, { total: 900, shown: 500 }, 40).title, /not every one is drawn/);
  assert.deepEqual(MV.layerCount(pts, { total: 900, shown: 900 }, undefined), { text: '', title: '' });
  assert.deepEqual(MV.layerCount(pts, null, 5), { text: '', title: '' });
  assert.deepEqual(MV.layerCount(cells, { total: 77 }, 0), { text: '77', title: '77 in view' });
});

test('keyGroups: layers in panel order, legend order, the layer style last', () => {
  const layers = [
    { id: 'a', label: 'Villages', legend: [{ label: 'Human' }, { label: 'Ribbit' }, { label: 'Haven' }] },
    { id: 'b', label: 'Roads' },
    { id: 'c', label: 'Herds', legend: [{ label: 'Bison' }] },
    { id: 'd', label: 'Off' },
  ];
  const seen = new Map([['c', new Set([-1, 0])], ['a', new Set([2, 0, 7, -1])], ['b', new Set([-1])], ['d', new Set()]]);
  const g = MV.keyGroups(seen, layers);
  assert.deepEqual(g.map(x => x.layer.id), ['a', 'b', 'c']);
  assert.deepEqual(g[0].entries, [{ index: 0, label: 'Human' }, { index: 2, label: 'Haven' }, { index: -1, label: 'Other' }]);
  assert.deepEqual(g[1].entries, [{ index: -1, label: 'Roads' }]);
  assert.deepEqual(g[2].entries, [{ index: 0, label: 'Bison' }, { index: -1, label: 'Other' }]);
  assert.deepEqual(MV.keyGroups({ b: [-1] }, layers).map(x => x.entries[0].label), ['Roads']);
  assert.deepEqual(MV.keyGroups(null, layers), []);
  // Count bubbles drawn: a last row saying what one is, drawn as a bubble.
  const withBubbles = MV.keyGroups(seen, layers, { clusters: 3 });
  const last = withBubbles[withBubbles.length - 1];
  assert.deepEqual(last.layer.kinds, ['cluster']);
  assert.deepEqual(last.entries, [{ index: -1, label: last.layer.label }]);
  assert.equal(MV.keyGroups(seen, layers, { clusters: 0 }).length, 3);
});

test('seenFromAnswers: legend indices from the marks held, clusters by their counts', () => {
  const seen = MV.seenFromAnswers([
    { desc: { id: 'a' }, data: { marks: [{ k: 'point', ci: 1 }, { k: 'point' }, { k: 'cluster', by: { 0: 3, 2: 1 } }] } },
    { desc: { id: 'b' }, data: { marks: [] } },
    { desc: { id: 'c' }, data: null },
  ]);
  assert.deepEqual([...seen.keys()], ['a']);
  assert.deepEqual([...seen.get('a')].sort(), [-1, 0, 1, 2]);
});

test('findBpp, baseShown, fmtCount', () => {
  assert.equal(MV.findBpp(null, 40), 2);
  assert.equal(MV.findBpp(null, 0.5), 0.5);
  assert.equal(MV.findBpp({ max_bpp: 1 }, 40), 1);
  assert.equal(MV.findBpp({ min_bpp: 4 }, 40), 4);
  const tiles = [{ id: 'grid', overlay: true }, { id: 'terrain' }, { id: 'habitat' }];
  assert.equal(MV.baseShown(tiles, new Set(['grid', 'habitat'])), 'habitat');
  assert.equal(MV.baseShown(tiles, new Set(['grid'])), null);
  assert.equal(MV.fmtCount(0), '0');
  assert.equal(MV.fmtCount(9999), '9,999');
  assert.equal(MV.fmtCount(123456), MV.compact(123456));
  assert.equal(MV.fmtCount(null), '');
});

test('isCompact: many short rows read as a table', () => {
  const rows = n => Array.from({ length: n }, (_, i) => ({ label: `Stop ${i}`, value: i * 10, unit: 's' }));
  assert.equal(MV.isCompact(rows(5)), false);
  assert.equal(MV.isCompact(rows(6)), true);
  assert.equal(MV.isCompact([...rows(6), { label: 'State', value: 'Replaying since day 3,402, matched' }]), false);
  assert.equal(MV.isCompact([...rows(6), { label: 'A label far too long to sit in a table cell', value: 1 }]), false);
  assert.equal(MV.isCompact(null), false);
});
