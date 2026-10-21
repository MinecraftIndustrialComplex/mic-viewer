# The map core (`web/map.js`)

Deep Time's workbench "Final world" view (`crates/deeptime-viewer/assets/worldmap.js` at Deep Time
master `30c4d664`), made generic (VIEWER.md §3.1, §14 item 1). It is a canvas 2D slippy map in block
coordinates over 256 px tiles: pan, wheel zoom anchored at the cursor, pinch, double click, keys,
east-west wrap, a tile LRU of 420 with 8 in flight and ancestor fallback up to 5 levels, and
magnification past the finest zoom. It knows no project: the projection, the tile pyramid, the tile
layers, what is drawn over the tiles and the readout are the caller's.

Every web file is a classic script (no build step, no modules, so Deep Time's workbench loads it with
a plain `<script>`) that adds to `globalThis.MicViewer` and, under node, to `module.exports`, so the
pure functions are testable with `node --test`.

## API

```js
const map = MicViewer.createMap({
  canvas,              // a <canvas>; the map sizes its backing store to the CSS box and devicePixelRatio
  projection,          // docs/api.md §2.1: { period, zRange, latLon?, grid? }
  pyramid,             // docs/api.md §2.2, or null for no tiles (tile layers may bring their own)
  background: '#04060a',
  minBpp: 1 / 4,       // the closest zoom, blocks per CSS pixel (default 1/4: 4x magnification)
  maxBpp: null,        // the farthest; default: the period (a wrapping frame), 2x what fits `extent`
                       // (a flat world), else 4096
  extent: null,        // [x0, z0, x1, z1] that home() fits (a flat world's content bounds)
  onView: view => {},  // after every view change (debounce on your side)
  onHover: (x, z, px, py) => {}, // the block under the cursor and where it is, or (null, null) on leaving
  onClick: (x, z, px, py, ev) => {}, // a click that was not a drag
  onKey: ev => false,  // keys the map did not use; return true when used
  overlays: [],        // functions (ctx, map) drawn after the tiles each frame, ctx in CSS px
  scaleBar: { right: 12, bottom: 12 }, // where the scale bar sits (CSS px from the corner), or false
  view: null,          // { cx, cz, bpp } to start at instead of home()
  start: true,         // false: call map.start() yourself (after wiring what the overlays need)
});

map.setTiles([{ id, url: (z, x, y) => string, alpha: 1,
                pyramid: null,   // this layer's own pyramid (default: the map's)
                bounds: null,    // [x0, z0, x1, z1]: tiles outside are never asked for
                visible: true }]);                        // bottom first; redraws
map.setProjection(projection, pyramid, extent);           // a new frame or run; keeps the view,
                               // re-clamped, and reports that through onView: read any view
                               // you saved for the new frame before calling it
map.setBackground(css); map.setScaleBar({ right, bottom, ink, halo } | false);
map.setView(cx, cz, bpp);      // centre (blocks) and blocks per CSS pixel; clamped
map.view                       // { cx, cz, bpp, w, h, dpr }
map.projection; map.pyramid    // as set
map.cursor                     // [x, z] under the pointer, or null
map.home();                    // fit the extent / the whole frame
map.zoomAt(px, py, factor);    // keep the block under (px, py) where it is
map.toWorld(px, py)            // -> [x, z] (unwrapped)
map.toScreen(x, z)             // -> [px, py] (unwrapped)
map.copies(x, pad)             // the x offsets (0, +-period, ...) at which a thing spanning
                               // x - pad .. x + pad shows in view; [0] without wrap
map.minBpp(); map.maxBpp()     // the zoom range now
map.tileZoom()                 // the tile zoom drawn now (of the first visible layer)
map.stats()                    // { tiles, wanted, inflight, cached, frames } of the last frame
map.idle()                     // nothing loading, nothing wanted, nothing to redraw (for tests)
map.invalidate();              // redraw on the next animation frame
map.redraw();                  // draw now
map.start(); map.stop();       // the animation loop (stop while hidden)
map.destroy();                 // remove listeners
```

The map draws only when something changed: an overlay that animates (legs moving with game time)
calls `map.invalidate()` itself.

Keys on the focused canvas: arrows pan, `+`/`=` and `-` zoom, `0`/`Home` home; other keys go to
`onKey`. The map never changes the page outside its canvas. A mouse press focuses the canvas; a
finger does not force it (a tap focuses natively), so a phone shows no focus ring on the map.

Tiles are fetched (same-origin GETs) and decoded off the main thread. A 200 is drawn; a 204 is
empty, drawn as nothing and never asked again (no stand-in); a 404 is missing, never asked again,
and an ancestor stands in; any other failure is asked again after 4 s. Drawn tiles go through an LRU
of 420 with at most 8 loading; a tile not yet loaded borrows the nearest loaded ancestor up to 5
levels up. Smoothing is off once a tile pixel covers 1.75 device pixels (magnified: crisp blocks).

The zoom drawn is the first (coarsest first) whose tile pixel is no wider than a device pixel times
2^0.3, clamped to the pyramid; past the finest zoom the tiles are magnified (down to `minBpp`).

## Pure helpers (tested under node)

`tests/web/*.test.js`, run with `nix develop -c node --test 'tests/web/*.test.js'`:

- map.js: `zoomFor(pyramid, blocksPerDevicePx)`, `tileCover(pyramid, z, left, top, widthBlocks,
  heightBlocks)` (the tiles to draw, with their block rectangles, wrapped indices `x` and unwrapped
  columns `k`), `tileInBounds(pyramid, z, x, y, bounds)`, `copiesIn(x, pad, left, right, period)`,
  `wrap(x, period)`, `niceStep(v)`, `fmtInt(v)`.
- flat.js: `projectionFor(frame)`, `squarePyramid({min_zoom, max_zoom})`, `pyramidFor(tileLayer)`
  (square, or a kind a plug-in put in `MicViewer.pyramidKinds`), `tileUrl(template)`, and
  `loadProjection(frame)` (runs a `plugin` frame's script and factory).
- marks.js: `legAt(mark, t)` (the place on a leg at time t; the same cases as Rust's `leg_at`),
  `colorOf`, `glyphOf`, `rampColor`, `xExtent`, `distToSegment`, `pick(targets, px, py)`.
- card.js: `fmtNum`, `fmtValue`, `sparkPath`.
- timeline.js: `gameDay`, `gameClock`, `fmtGameTime`, `fmtSpan`, `wallAt`, `fmtWall`, `wallLabels`,
  `axisTicks`, `stackBuckets`.
- app.js: `parseHash`, `buildHash`, `inZoom`, `zoomHint`, `viewBox`, `marksURL`, `changedLayers`,
  `isStale`, `tickAt`, `fmtAge`.

Deep Time's 2:1 pyramid (`pixel(z) = C / (2 * 256 * 2^z)`, origin `(-C/2, -C/4)`, `cols(z) = 2 << z`
wrapping, `rows(z) = [0, 2^z - 1]`, period `C`, z range `[-C/4, C/4]`) is one of the test cases:
the core needs nothing from it beyond the pyramid object.
