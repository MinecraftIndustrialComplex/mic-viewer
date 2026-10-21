// SPDX-License-Identifier: MPL-2.0
// Marks and cards: legAt against Rust's leg_at cases, colours and glyphs, ramps, hit testing,
// number formatting and sparklines.
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
