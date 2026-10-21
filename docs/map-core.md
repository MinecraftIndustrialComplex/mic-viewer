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
  pyramid,             // docs/api.md §2.2, or null for no tiles
  background: '#04060a',
  minBpp: 1 / 4,       // the closest zoom, blocks per CSS pixel (default 1/4: 4x magnification)
  maxBpp: null,        // the farthest; default: what fits `extent` (or the projection's whole frame)
  extent: null,        // [x0, z0, x1, z1] that home() fits (a flat world's content bounds)
  onView: view => {},  // after every view change (debounce on your side)
  onHover: (x, z) => {}, // the block under the cursor, or (null, null) when it leaves
  onClick: (x, z, px, py, ev) => {}, // a click that was not a drag
  onKey: ev => false,  // keys the map did not use; return true when used
  overlays: [],        // functions (ctx, map) drawn after the tiles each frame, ctx in CSS px
});

map.setTiles([{ id, url: (z, x, y) => string, alpha: 1 }]);  // bottom first; redraws
map.setProjection(projection, pyramid, extent);               // a new frame or run
map.setView(cx, cz, bpp);      // centre (blocks) and blocks per CSS pixel; clamped
map.view                       // { cx, cz, bpp, w, h, dpr }
map.home();                    // fit the extent / the whole frame
map.zoomAt(px, py, factor);    // keep the block under (px, py) where it is
map.toWorld(px, py)            // -> [x, z] (unwrapped)
map.toScreen(x, z)             // -> [px, py] (unwrapped)
map.copies(x)                  // the x offsets (0, +-period, ...) at which a thing at x shows in view
map.tileZoom()                 // the tile zoom drawn now
map.invalidate();              // redraw on the next animation frame
map.start(); map.stop();       // the animation loop (stop while hidden)
map.destroy();                 // remove listeners
```

Keys on the focused canvas: arrows pan, `+`/`=` and `-` zoom, `0`/`Home` home; other keys go to
`onKey`. The map never changes the page outside its canvas.

## Pure helpers (tested under node)

`MicViewer.squarePyramid({min_zoom, max_zoom})`, `MicViewer.zoomFor(pyramid, blocksPerDevicePx)`,
`MicViewer.tileCover(pyramid, z, left, top, widthBlocks, heightBlocks)` (the tiles to draw, with their
block rectangles and wrapped indices), `MicViewer.wrap(x, period)`, `MicViewer.niceStep(v)`,
`MicViewer.legAt(mark, t)` (marks.js: the place on a leg at time t).
