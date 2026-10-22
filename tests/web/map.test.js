// SPDX-License-Identifier: MPL-2.0
// The map core's and the projections' pure parts: zoom choice, tile cover with wrap and negative
// indices, the wrap itself, scale-bar steps, copies across a seam, frames and pyramids.
//
//     nix develop -c node --test 'tests/web/*.test.js'
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const web = f => path.join(__dirname, '..', '..', 'crates', 'mic-viewer', 'web', f);
require(web('map.js'));
const MV = require(web('flat.js'));

const plain = a => a.map(v => v + 0);   // -0 to 0

/** Deep Time's 2:1 pyramid of a planet with circumference `C` (docs/map-core.md). */
function deepTime(C, maxZoom) {
  return {
    T: 256,
    minZoom: 0,
    maxZoom,
    pixel: z => C / (2 * 256 * Math.pow(2, z)),
    originX: -C / 2,
    originZ: -C / 4,
    cols: z => 2 << z,
    rows: z => [0, Math.pow(2, z) - 1],
  };
}

test('wrap folds x into [-period/2, period/2)', () => {
  assert.equal(MV.wrap(5, 0), 5);
  assert.equal(MV.wrap(-90000, 0), -90000);
  assert.equal(MV.wrap(2049, 4096), -2047);
  assert.equal(MV.wrap(2048, 4096), -2048);
  assert.equal(MV.wrap(-2048, 4096), -2048);
  assert.equal(MV.wrap(-6000, 4096), -1904);
  assert.equal(MV.wrap(4096 * 7 + 12, 4096), 12);
});

test('fitView fits a box into what the panels leave and centres it there', () => {
  // No panels: the box's middle in the view's, 6% of room.
  const a = MV.fitView([0, 0, 100, 50], 200, 100, null);
  assert.deepEqual([a.cx, a.cz], [50, 25]);
  assert.ok(Math.abs(a.bpp - 0.53) < 1e-9);
  // A 100 px key on the left: the box fits the 100 px right of it, its middle 50 px right of the view's.
  const b = MV.fitView([0, 0, 100, 50], 200, 100, { left: 100 });
  assert.ok(Math.abs(b.bpp - 1.06) < 1e-9);
  const mid = 100 + (50 - b.cx) / b.bpp;   // the box's middle, px from the view's left edge
  assert.ok(Math.abs(mid - 150) < 1e-9, `middle at ${mid}`);
  // A pad that leaves too little is not honoured past 40% of the view.
  const c = MV.fitView([0, 0, 100, 50], 200, 100, { left: 190 });
  assert.ok(Math.abs(c.bpp - 100 / 80 * 1.06) < 1e-9);
  assert.equal(c.cx, 50);
});

test('niceStep is 1, 2 or 5 times a power of ten, not above v', () => {
  const cases = [[0.7, 0.5], [1, 1], [1.9, 1], [2, 2], [7, 5], [999, 500], [1000, 1000], [2500, 2000], [0.03, 0.02]];
  for (const [v, want] of cases) assert.ok(Math.abs(MV.niceStep(v) - want) < 1e-12, `niceStep(${v})`);
  assert.equal(MV.niceStep(0), 1);
  assert.equal(MV.niceStep(-3), 1);
  assert.equal(MV.niceStep(Infinity), 1);
});

test('fmtInt groups thousands', () => {
  assert.equal(MV.fmtInt(12345), '12,345');
  assert.equal(MV.fmtInt(-3), '-3');
  assert.equal(MV.fmtInt(-1234.6), '-1,235');
  assert.equal(MV.fmtInt(999), '999');
  assert.equal(MV.fmtInt(1000000), '1,000,000');
});

test('zoomFor picks the first zoom whose pixel is within 2^0.3 device pixels', () => {
  const sq = MV.squarePyramid({});           // pixel(z) = 2^-z, zooms -8 .. 0
  assert.equal(MV.zoomFor(sq, 1), 0);
  assert.equal(MV.zoomFor(sq, 0.25), 0);       // magnified past the finest zoom
  assert.equal(MV.zoomFor(sq, 2.5), -1);       // 2 <= 2.5 * 1.23
  assert.equal(MV.zoomFor(sq, 3.3), -2);       // 4 <= 3.3 * 1.23 = 4.06
  assert.equal(MV.zoomFor(sq, 3.2), -1);       // 4 > 3.94
  assert.equal(MV.zoomFor(sq, 1e6), -8);       // clamped to the coarsest
  // worldmap.js's closed form, ceil(log2(s0 / dev) - 0.3), on Deep Time's pyramid.
  const C = 40000;
  const dt = deepTime(C, 9);
  const s0 = dt.pixel(0);
  for (const dev of [s0, s0 / 3, s0 / 10.5, s0 / 100, s0 / 511]) {
    const want = Math.min(9, Math.max(0, Math.ceil(Math.log2(s0 / dev) - 0.3)));
    assert.equal(MV.zoomFor(dt, dev), want, `dev ${dev}`);
  }
});

test('tileCover gives negative indices on the square pyramid', () => {
  const sq = MV.squarePyramid({});
  const tiles = MV.tileCover(sq, 0, -300, -10, 100, 100);
  assert.deepEqual(tiles.map(t => [t.x, t.y]), [[-2, -1], [-1, -1], [-2, 0], [-1, 0]]);
  assert.deepEqual(tiles.map(t => t.k), [-2, -1, -2, -1]);
  assert.deepEqual(tiles[0], { z: 0, x: -2, y: -1, k: -2, bx: -512, bz: -256, size: 256 });
  // Coarser: zoom -2 tiles are 1,024 blocks.
  const coarse = MV.tileCover(sq, -2, -1500, 0, 3000, 10);
  assert.deepEqual(coarse.map(t => t.x), [-2, -1, 0, 1]);
  assert.equal(coarse[0].size, 1024);
});

test('tileCover wraps columns and keeps rows inside the 2:1 pyramid', () => {
  const C = 4096;
  const dt = deepTime(C, 4);
  // Zoom 0: two tiles of C/2 blocks, one row.
  const all = MV.tileCover(dt, 0, -C / 2, -C / 4, C, C / 2);
  assert.deepEqual(all.map(t => [t.x, t.y]), [[0, 0], [1, 0]]);
  // Across the seam at x = C/2: the second column is column 0 again, drawn east of the seam.
  const seam = MV.tileCover(dt, 0, C / 2 - 100, -100, 200, 200);
  assert.deepEqual(seam.map(t => [t.x, t.k]), [[1, 1], [0, 2]]);
  assert.equal(seam[1].bx, C / 2);
  // West of the seam: negative unwrapped columns ask for the last ones.
  const west = MV.tileCover(dt, 2, -C / 2 - 300, 10, 200, 20);
  assert.deepEqual(west.map(t => [t.x, t.k, t.y]), [[7, -1, 2]]);
  // Rows past the poles are left out.
  const tall = MV.tileCover(dt, 1, 10, -C, 20, 2 * C);
  assert.deepEqual(tall.map(t => t.y), [0, 1]);
  // A view of the whole circumference several times over stays bounded.
  assert.equal(MV.tileCover(dt, 0, -3 * C, 0, 6 * C, 1).length, 12);   // C / 2 a tile
});

test('tileInBounds keeps tiles that meet the bounds', () => {
  const sq = MV.squarePyramid({});
  const b = [-6144, -6144, 6144, 6144];
  assert.equal(MV.tileInBounds(sq, 0, 0, 0, b), true);
  assert.equal(MV.tileInBounds(sq, 0, -24, -24, b), true);
  assert.equal(MV.tileInBounds(sq, 0, 24, 0, b), false);
  assert.equal(MV.tileInBounds(sq, 0, -25, 0, b), false);
  assert.equal(MV.tileInBounds(sq, 0, 99, 99, null), true);
});

test('copiesIn gives the offsets at which a thing shows across the wrap', () => {
  assert.deepEqual(MV.copiesIn(2000, 50, 1900, 2300, 0), [0]);
  assert.deepEqual(plain(MV.copiesIn(2000, 50, 1900, 2300, 4096)), [0]);
  assert.deepEqual(MV.copiesIn(-2000, 50, 1900, 2300, 4096), [4096]);
  // A view wider than the period sees several copies.
  assert.deepEqual(plain(MV.copiesIn(0, 10, -5000, 5000, 4096)), [-4096, 0, 4096]);
  // Out of view: none.
  assert.deepEqual(MV.copiesIn(0, 10, 100, 200, 4096), []);
});

test('projectionFor reads flat, xwrap and plugin frames', () => {
  assert.deepEqual(MV.projectionFor({ projection: { kind: 'flat' } }),
    { kind: 'flat', period: 0, zRange: null, latLon: null, grid: null });
  assert.equal(MV.projectionFor({}).kind, 'flat');
  assert.equal(MV.projectionFor(null).period, 0);
  const ring = MV.projectionFor({ projection: { kind: 'xwrap', period_x: 4096, z_range: [-1024, 1024] } });
  assert.equal(ring.period, 4096);
  assert.deepEqual(ring.zRange, [-1024, 1024]);
  // A flat frame never wraps, whatever it carries.
  assert.equal(MV.projectionFor({ projection: { kind: 'flat', period_x: 4096 } }).period, 0);
  const plug = MV.projectionFor({ projection: { kind: 'plugin', period_x: 40000, z_range: [-10000, 10000], script: 'x.js' } });
  assert.equal(plug.kind, 'plugin');
  assert.equal(plug.period, 40000);
});

test('squarePyramid: a block a pixel at zoom 0, unbounded', () => {
  const p = MV.squarePyramid({ min_zoom: -7, max_zoom: 0 });
  assert.equal(p.minZoom, -7);
  assert.equal(p.maxZoom, 0);
  assert.equal(p.pixel(0), 1);
  assert.equal(p.pixel(-7), 128);
  assert.equal(p.cols(0), 0);
  assert.equal(p.rows(0), null);
  const d = MV.squarePyramid({});
  assert.equal(d.minZoom, -8);
  assert.equal(d.maxZoom, 0);
});

test('pyramidFor knows square and registered kinds', () => {
  assert.equal(MV.pyramidFor({ pyramid: { kind: 'square', min_zoom: -3, max_zoom: 1 } }).minZoom, -3);
  assert.equal(MV.pyramidFor({ pyramid: { kind: 'nope' } }), null);
  MV.pyramidKinds.test2to1 = (d, layer) => Object.assign(deepTime(d.circumference, d.max_zoom), { layer: layer.id });
  const p = MV.pyramidFor({ id: 'relief', pyramid: { kind: 'test2to1', circumference: 4096, max_zoom: 3 } });
  assert.equal(p.cols(1), 4);
  assert.equal(p.layer, 'relief');
  delete MV.pyramidKinds.test2to1;
});

test('tileUrl fills a template', () => {
  assert.equal(MV.tileUrl('tiles/terrain/{z}/{x}/{y}.png')(-3, -2, 5), 'tiles/terrain/-3/-2/5.png');
});

test('tileFilterFor: the layer\'s own filter, else the map\'s; false or blank is none', () => {
  assert.equal(MV.tileFilterFor({}, null), 'none');
  assert.equal(MV.tileFilterFor({}, 'saturate(0.3)'), 'saturate(0.3)');
  assert.equal(MV.tileFilterFor({ filter: null }, ' grayscale(1) '), 'grayscale(1)');
  assert.equal(MV.tileFilterFor({ filter: false }, 'saturate(0.3)'), 'none');
  assert.equal(MV.tileFilterFor({ filter: 'blur(1px)' }, 'saturate(0.3)'), 'blur(1px)');
  assert.equal(MV.tileFilterFor({ filter: '' }, 'saturate(0.3)'), 'none');
  assert.equal(MV.tileFilterFor(null, ''), 'none');
});
