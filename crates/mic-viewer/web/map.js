// SPDX-License-Identifier: MPL-2.0
// mic-viewer's map core (docs/map-core.md): a canvas 2D slippy map in block coordinates over 256 px
// tiles. Pan (drag), wheel zoom anchored at the cursor, pinch, double click, keys, east-west wrap,
// a tile LRU with a flight cap and ancestor fallback, magnification past the finest zoom, a scale
// bar. Adapted from Deep Time's workbench "Final world" view (crates/deeptime-viewer/assets/
// worldmap.js at 30c4d664, NOTICE.md) and made generic: the projection, the tile pyramid, the tile
// layers, what is drawn over the tiles and the readout are the caller's. It knows no project.
//
// A classic script (no modules, no build step): it adds to `globalThis.MicViewer` and, under node,
// to `module.exports`, so the pure helpers at the top are testable with `node --test`.
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});

  const T = 256;              // tile edge, px
  const MAX_FLIGHT = 8;       // tiles loading at once
  const KEEP_TILES = 420;     // tiles kept (decoded, empty or failed)
  const ANCESTORS = 5;        // levels up a missing tile looks for a stand-in
  const RETRY_MS = 4000;      // a failed tile is asked again after this
  const ZOOM_BIAS = 0.3;      // a tile zoom is drawn up to 2^0.3 device pixels a tile pixel
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

  // ------------------------------------------------------------------------------------------------
  // Pure helpers (tested under node)
  // ------------------------------------------------------------------------------------------------

  /** `x` folded into `[-period/2, period/2)`; unchanged when `period` is 0 (no wrap). */
  function wrap(x, period) {
    if (!(period > 0)) return x;
    const h = period / 2;
    return ((((x + h) % period) + period) % period) - h;
  }

  /** 1, 2 or 5 times a power of ten, the largest not above `v`. */
  function niceStep(v) {
    if (!(v > 0) || !isFinite(v)) return 1;
    const e = Math.pow(10, Math.floor(Math.log10(v)));
    const m = v / e;
    // Guard against log10 rounding (log10(1000) can land a hair under 3).
    if (m >= 9.999999) return 10 * e;
    return (m >= 5 ? 5 : m >= 2 ? 2 : 1) * e;
  }

  /** A whole number with thousands separators ("12,345", "-3"). */
  function fmtInt(v) {
    const n = Math.round(v);
    const s = String(Math.abs(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return n < 0 ? '-' + s : s;
  }

  /**
   * The tile zoom for `blocksPerDevicePx`: the first zoom (coarsest first) whose tile pixel is no
   * wider than a device pixel times 2^0.3, clamped to the pyramid (worldmap.js's rule: with
   * `pixel(z) = s0 / 2^z` it is `ceil(log2(s0 / dev) - 0.3)`).
   */
  function zoomFor(pyramid, blocksPerDevicePx) {
    const lim = blocksPerDevicePx * Math.pow(2, ZOOM_BIAS);
    for (let z = pyramid.minZoom; z <= pyramid.maxZoom; z++) {
      if (pyramid.pixel(z) <= lim * (1 + 1e-9)) return z;
    }
    return pyramid.maxZoom;
  }

  /**
   * The tiles of zoom `z` that cover the block rectangle from (`left`, `top`), `widthBlocks` east and
   * `heightBlocks` south. Each: `{ z, x, y, k, bx, bz, size }` where `x` is the tile index to ask
   * for (wrapped into `[0, cols)` when the pyramid wraps), `k` the unwrapped column, `y` the row, and
   * (`bx`, `bz`) the block corner where it is drawn (unwrapped), `size` its edge in blocks. Rows
   * outside `rows(z)` are left out.
   */
  function tileCover(pyramid, z, left, top, widthBlocks, heightBlocks) {
    const S = (pyramid.T || T) * pyramid.pixel(z);
    const ox = pyramid.originX || 0, oz = pyramid.originZ || 0;
    const k0 = Math.floor((left - ox) / S);
    const k1 = Math.ceil((left + widthBlocks - ox) / S) - 1;
    let j0 = Math.floor((top - oz) / S);
    let j1 = Math.ceil((top + heightBlocks - oz) / S) - 1;
    const rows = pyramid.rows ? pyramid.rows(z) : null;
    if (rows) {
      j0 = Math.max(j0, rows[0]);
      j1 = Math.min(j1, rows[1]);
    }
    const cols = pyramid.cols ? pyramid.cols(z) : 0;
    const out = [];
    // A degenerate request (a huge view of a tiny pyramid) is cut rather than looping forever.
    if (k1 - k0 > 4096 || j1 - j0 > 4096) return out;
    for (let j = j0; j <= j1; j++) {
      for (let k = k0; k <= k1; k++) {
        const x = cols > 0 ? ((k % cols) + cols) % cols : k;
        out.push({ z, x, y: j, k, bx: ox + k * S, bz: oz + j * S, size: S });
      }
    }
    return out;
  }

  /** Whether the tile at wrapped index `x`, row `y` of zoom `z` meets `bounds` `[x0, z0, x1, z1]`. */
  function tileInBounds(pyramid, z, x, y, bounds) {
    if (!bounds) return true;
    const S = (pyramid.T || T) * pyramid.pixel(z);
    const bx = (pyramid.originX || 0) + x * S, bz = (pyramid.originZ || 0) + y * S;
    return bx < bounds[2] && bx + S > bounds[0] && bz < bounds[3] && bz + S > bounds[1];
  }

  /**
   * The x offsets (0, +-period, ...) at which a thing spanning `x - pad .. x + pad` shows in the
   * east-west block range `[left, right]`. `[0]` without wrap (whether or not it is in view).
   */
  function copiesIn(x, pad, left, right, period) {
    if (!(period > 0)) return [0];
    const out = [];
    const n0 = Math.ceil((left - pad - x) / period), n1 = Math.floor((right + pad - x) / period);
    for (let n = n0; n <= n1 && out.length < 64; n++) out.push(n * period);
    return out;
  }

  // ------------------------------------------------------------------------------------------------
  // The map
  // ------------------------------------------------------------------------------------------------

  function createMap(opts) {
    const canvas = opts.canvas;
    const ctx = canvas.getContext('2d');
    let projection = opts.projection || { period: 0, zRange: null };
    let pyramid = opts.pyramid || null;
    let extent = opts.extent || null;
    let background = opts.background || '#04060a';
    const minBppOpt = opts.minBpp != null ? opts.minBpp : 1 / 4;
    const maxBppOpt = opts.maxBpp != null ? opts.maxBpp : null;
    const onView = opts.onView || null, onHover = opts.onHover || null;
    const onClick = opts.onClick || null, onKey = opts.onKey || null;
    const overlays = opts.overlays || [];
    let scaleBar = opts.scaleBar === false ? null : Object.assign({ right: 12, bottom: 12 }, opts.scaleBar || {});
    let layers = [];

    const V = { cx: 0, cz: 0, bpp: 1, w: 0, h: 0, dpr: 1 };
    let dirty = true, raf = 0, running = false, destroyed = false;
    const tiles = new Map();   // url -> { img, state: 'queued' | 'loading' | 'ok' | 'empty' | 'missing' | 'err', t }
    let inflight = 0, clock = 0;
    const pointers = new Map();
    let pinch = null, moved = 0, cursor = null;
    const stats = { tiles: 0, wanted: 0, frames: 0 };

    if (!canvas.hasAttribute('tabindex')) canvas.tabIndex = 0;
    canvas.style.touchAction = 'none';

    const P = () => projection.period || 0;
    const ext = () => extent || null;

    function fitBpp(e) {
      return Math.max((e[2] - e[0]) / Math.max(V.w, 1), (e[3] - e[1]) / Math.max(V.h, 1));
    }
    function minBpp() { return minBppOpt; }
    function maxBpp() {
      if (maxBppOpt) return maxBppOpt;
      const zr = projection.zRange;
      if (P() > 0) return Math.max(P() / Math.max(V.w, 1), zr ? (zr[1] - zr[0]) / Math.max(V.h, 1) : 0, minBppOpt);
      const e = ext();
      // A flat world: room to see its content with some of the nothing around it.
      if (e) return Math.max(fitBpp(e) * 2, minBppOpt);
      return 4096;
    }

    function setView(cx, cz, bpp) {
      if (!isFinite(cx) || !isFinite(cz) || !isFinite(bpp)) return;
      bpp = clamp(bpp, minBpp(), maxBpp());
      const halfH = (V.h * bpp) / 2;
      const zr = projection.zRange, e = ext();
      if (zr) {
        // Vertically the view stays on the frame (or centred when it is taller than the frame).
        cz = 2 * halfH >= zr[1] - zr[0] ? (zr[0] + zr[1]) / 2 : clamp(cz, zr[0] + halfH, zr[1] - halfH);
      } else if (e) {
        cz = clamp(cz, e[1], e[3]);
      }
      if (P() > 0) cx = wrap(cx, P());
      else if (e) cx = clamp(cx, e[0], e[2]);
      if (cx === V.cx && cz === V.cz && bpp === V.bpp) return;
      V.cx = cx; V.cz = cz; V.bpp = bpp;
      dirty = true;
      if (onView) onView(view());
    }

    const toWorld = (px, py) => [V.cx + (px - V.w / 2) * V.bpp, V.cz + (py - V.h / 2) * V.bpp];
    const toScreen = (bx, bz) => [V.w / 2 + (bx - V.cx) / V.bpp, V.h / 2 + (bz - V.cz) / V.bpp];

    function zoomAt(px, py, factor) {
      // Keep the block under (px, py) where it is.
      const [bx, bz] = toWorld(px, py);
      const bpp = clamp(V.bpp / factor, minBpp(), maxBpp());
      setView(bx - (px - V.w / 2) * bpp, bz - (py - V.h / 2) * bpp, bpp);
    }

    function home() {
      const e = ext();
      if (P() > 0) {
        const cx = e ? (e[0] + e[2]) / 2 : 0;
        const zr = projection.zRange;
        setView(cx, zr ? (zr[0] + zr[1]) / 2 : e ? (e[1] + e[3]) / 2 : 0, maxBpp());
      } else if (e) {
        setView((e[0] + e[2]) / 2, (e[1] + e[3]) / 2, fitBpp(e) * 1.06);
      } else {
        setView(0, 0, 8);
      }
    }

    function view() {
      return { cx: V.cx, cz: V.cz, bpp: V.bpp, w: V.w, h: V.h, dpr: V.dpr };
    }

    function copies(x, pad) {
      const left = V.cx - (V.w / 2) * V.bpp, right = V.cx + (V.w / 2) * V.bpp;
      return copiesIn(x, pad || 0, left, right, P());
    }

    // ----------------------------------------------------------------------------------------------
    // Tiles
    // ----------------------------------------------------------------------------------------------
    function tileAt(url, start) {
      let t = tiles.get(url);
      if (t) { t.t = ++clock; return t; }
      if (!start) return null;
      t = { url, img: null, state: 'queued', t: ++clock };
      tiles.set(url, t);
      return t;
    }

    function load(t) {
      t.state = 'loading';
      inflight++;
      fetch(t.url, { credentials: 'same-origin' })
        .then(r => {
          if (r.status === 204) return null;            // nothing there
          if (r.status === 404) return false;           // no such tile: never asked again
          if (!r.ok) throw new Error(String(r.status));
          return r.blob().then(b => createImageBitmap(b));
        })
        .then(img => {
          t.img = img || null;
          t.state = img ? 'ok' : img === false ? 'missing' : 'empty';
        }, () => {
          t.state = 'err';
          setTimeout(() => { if (tiles.get(t.url) === t && t.state === 'err') tiles.delete(t.url); }, RETRY_MS);
        })
        .then(() => {
          inflight--;
          if (!tiles.has(t.url) && t.img && t.img.close) t.img.close();
          dirty = true;
        });
    }

    function pump(want) {
      // Start the nearest wanted tiles that are not loading yet; the rest wait for the next draw.
      want.sort((a, b) => a.dist - b.dist);
      for (const w of want) {
        if (inflight >= MAX_FLIGHT) break;
        if (w.t.state === 'queued') load(w.t);
      }
      if (tiles.size > KEEP_TILES) {
        const old = [...tiles.values()].filter(t => t.state !== 'loading').sort((a, b) => a.t - b.t);
        for (const t of old.slice(0, tiles.size - KEEP_TILES)) {
          tiles.delete(t.url);
          if (t.img && t.img.close) t.img.close();
        }
      }
    }

    function layerZoom(L) {
      const pyr = L.pyramid || pyramid;
      return pyr ? zoomFor(pyr, V.bpp / V.dpr) : null;
    }

    function drawLayer(L, want) {
      const pyr = L.pyramid || pyramid;
      if (!pyr) return;
      const z = zoomFor(pyr, V.bpp / V.dpr);
      const d = V.dpr;
      const left = V.cx - (V.w / 2) * V.bpp, top = V.cz - (V.h / 2) * V.bpp;
      const cover = tileCover(pyr, z, left, top, V.w * V.bpp, V.h * V.bpp);
      const px = ((pyr.T || T) * pyr.pixel(z)) / V.bpp;        // CSS px a tile
      const edge = Math.ceil(px * d + 0.5);
      ctx.globalAlpha = L.alpha != null ? L.alpha : 1;
      // Pixelated once a tile pixel is more than a screen pixel and a half.
      ctx.imageSmoothingEnabled = (px * d) / T < 1.75;
      for (const c of cover) {
        if (!tileInBounds(pyr, z, c.x, c.y, L.bounds)) continue;
        const sx = (c.bx - left) / V.bpp, sy = (c.bz - top) / V.bpp;
        const dx = Math.round(sx * d), dy = Math.round(sy * d);
        const t = tileAt(L.url(z, c.x, c.y), true);
        if (t.state === 'ok') {
          ctx.drawImage(t.img, dx, dy, edge, edge);
          stats.tiles++;
          continue;
        }
        if (t.state === 'empty') continue;
        if (t.state === 'queued' || t.state === 'loading') want.push({ t, dist: Math.hypot(sx + px / 2 - V.w / 2, sy + px / 2 - V.h / 2) });
        // Meanwhile: the nearest ancestor that is in, cropped and magnified.
        for (let up = 1; up <= ANCESTORS && z - up >= pyr.minZoom; up++) {
          const a = tileAt(L.url(z - up, c.x >> up, c.y >> up), false);
          if (a && a.state === 'ok') {
            const n = T >> up, ox = (c.x & ((1 << up) - 1)) * n, oy = (c.y & ((1 << up) - 1)) * n;
            ctx.drawImage(a.img, ox, oy, n, n, dx, dy, edge, edge);
            break;
          }
          if (a && a.state === 'empty') break;
        }
      }
      ctx.globalAlpha = 1;
    }

    function drawScaleBar() {
      if (!scaleBar) return;
      const n = niceStep(96 * V.bpp);                      // blocks in about 96 px
      const px = n / V.bpp;
      const x1 = V.w - scaleBar.right, x0 = x1 - px, y = V.h - scaleBar.bottom;
      ctx.save();
      ctx.lineCap = 'square';
      ctx.font = '500 11px ui-monospace, "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace';
      ctx.textAlign = 'right';
      ctx.textBaseline = 'bottom';
      const label = (n >= 1 ? fmtInt(n) : String(n)) + (n === 1 ? ' block' : ' blocks');
      for (const [colour, width, off] of [['rgba(0,0,0,.55)', 4, 0.5], ['#ffffff', 2, 0]]) {
        ctx.strokeStyle = colour;
        ctx.lineWidth = width;
        ctx.beginPath();
        ctx.moveTo(x0 + off, y - 6 + off);
        ctx.lineTo(x0 + off, y + off);
        ctx.lineTo(x1 + off, y + off);
        ctx.lineTo(x1 + off, y - 6 + off);
        ctx.stroke();
      }
      ctx.lineWidth = 3;
      ctx.strokeStyle = 'rgba(0,0,0,.6)';
      ctx.strokeText(label, x1, y - 5);
      ctx.fillStyle = '#ffffff';
      ctx.fillText(label, x1, y - 5);
      ctx.restore();
    }

    function resize() {
      const r = canvas.getBoundingClientRect();
      const dpr = globalThis.devicePixelRatio || 1;
      const w = Math.max(1, Math.round(r.width)), h = Math.max(1, Math.round(r.height));
      if (w === V.w && h === V.h && dpr === V.dpr) return;
      const first = V.w === 0;
      V.w = w; V.h = h; V.dpr = dpr;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      if (!first) {
        // Re-clamp for the new size (and tell the caller: its box changed).
        const keep = [V.cx, V.cz, V.bpp];
        V.bpp = -1;
        setView(keep[0], keep[1], keep[2]);
      }
      dirty = true;
    }

    function draw() {
      resize();
      const d = V.dpr;
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.globalAlpha = 1;
      ctx.fillStyle = background;
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      stats.tiles = 0;
      const want = [];
      for (const L of layers) if (L.visible !== false) drawLayer(L, want);
      stats.wanted = want.length;
      pump(want);
      ctx.setTransform(d, 0, 0, d, 0, 0);
      ctx.imageSmoothingEnabled = true;
      if (projection.grid) {
        ctx.save();
        projection.grid(ctx, view(), api);
        ctx.restore();
      }
      for (const f of overlays) {
        ctx.save();
        try { f(ctx, api); } finally { ctx.restore(); }
      }
      drawScaleBar();
      stats.frames++;
    }

    function frame() {
      raf = 0;
      if (!running || destroyed) return;
      if (dirty) {
        dirty = false;
        draw();
      }
      raf = requestAnimationFrame(frame);
    }

    // ----------------------------------------------------------------------------------------------
    // Pointer, wheel, keys
    // ----------------------------------------------------------------------------------------------
    const rel = ev => {
      const r = canvas.getBoundingClientRect();
      return [ev.clientX - r.left, ev.clientY - r.top];
    };
    const hover = (px, py) => {
      if (!onHover) return;
      if (px == null) onHover(null, null, null, null);
      else {
        const [x, z] = toWorld(px, py);
        onHover(x, z, px, py);
      }
    };
    const L = [];
    const listen = (target, type, fn, o) => {
      target.addEventListener(type, fn, o);
      L.push([target, type, fn, o]);
    };

    listen(canvas, 'pointerdown', ev => {
      if (ev.button !== undefined && ev.button > 0 && ev.pointerType === 'mouse') return;
      // Keys need the focus; a finger gets it natively on a tap, and a forced focus would ring the map.
      if (ev.pointerType !== 'touch') canvas.focus({ preventScroll: true });
      try { canvas.setPointerCapture(ev.pointerId); } catch (e) { /* synthetic events */ }
      const [x, y] = rel(ev);
      pointers.set(ev.pointerId, { x, y, sx: x, sy: y });
      if (pointers.size === 1) moved = 0;
      if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        pinch = { d: Math.hypot(a.x - b.x, a.y - b.y) };
        moved = 1e9;   // a pinch is never a click
      }
      canvas.classList.add('drag');
    });
    listen(canvas, 'pointermove', ev => {
      const [x, y] = rel(ev);
      const p = pointers.get(ev.pointerId);
      if (!p) {
        cursor = [x, y];
        hover(x, y);
        return;
      }
      if (pointers.size === 1) {
        moved = Math.max(moved, Math.hypot(x - p.sx, y - p.sy));
        if (moved >= 4) setView(V.cx - (x - p.x) * V.bpp, V.cz - (y - p.y) * V.bpp, V.bpp);
        else return;   // below the click slop: keep the start as the reference
      } else if (pointers.size === 2 && pinch) {
        const other = [...pointers.entries()].find(([id]) => id !== ev.pointerId)[1];
        const prev = Math.hypot(p.x - other.x, p.y - other.y), now = Math.hypot(x - other.x, y - other.y);
        const mx = (x + other.x) / 2, my = (y + other.y) / 2;
        const pmx = (p.x + other.x) / 2, pmy = (p.y + other.y) / 2;
        // Zoom about the midpoint, and follow it as it moves.
        const [bx, bz] = toWorld(pmx, pmy);
        const bpp = clamp(V.bpp * (prev / Math.max(now, 1)), minBpp(), maxBpp());
        setView(bx - (mx - V.w / 2) * bpp, bz - (my - V.h / 2) * bpp, bpp);
      }
      p.x = x; p.y = y;
      cursor = [x, y];
      hover(x, y);
    });
    const lift = (ev, click) => {
      const p = pointers.get(ev.pointerId);
      if (!p) return;
      pointers.delete(ev.pointerId);
      if (pointers.size < 2) pinch = null;
      if (!pointers.size) canvas.classList.remove('drag');
      if (click && !pointers.size && moved < 4 && onClick) {
        const [x, y] = rel(ev);
        const [bx, bz] = toWorld(x, y);
        onClick(bx, bz, x, y, ev);
      }
    };
    listen(canvas, 'pointerup', ev => lift(ev, true));
    listen(canvas, 'pointercancel', ev => lift(ev, false));
    listen(canvas, 'pointerleave', () => {
      if (!pointers.size) {
        cursor = null;
        hover(null, null);
      }
    });
    listen(canvas, 'wheel', ev => {
      ev.preventDefault();
      const [x, y] = rel(ev);
      const dy = ev.deltaMode === 1 ? ev.deltaY * 16 : ev.deltaMode === 2 ? ev.deltaY * 400 : ev.deltaY;
      zoomAt(x, y, Math.exp(-clamp(dy, -240, 240) * (ev.ctrlKey ? 0.01 : 0.0018)));
      hover(x, y);
    }, { passive: false });
    listen(canvas, 'dblclick', ev => {
      const [x, y] = rel(ev);
      zoomAt(x, y, ev.shiftKey ? 0.5 : 2);
    });
    listen(canvas, 'keydown', ev => {
      if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
      const k = ev.key, step = 0.2 * V.bpp;
      let used = true;
      if (k === 'ArrowLeft') setView(V.cx - V.w * step, V.cz, V.bpp);
      else if (k === 'ArrowRight') setView(V.cx + V.w * step, V.cz, V.bpp);
      else if (k === 'ArrowUp') setView(V.cx, V.cz - V.h * step, V.bpp);
      else if (k === 'ArrowDown') setView(V.cx, V.cz + V.h * step, V.bpp);
      else if (k === '+' || k === '=') zoomAt(V.w / 2, V.h / 2, 1.6);
      else if (k === '-' || k === '_') zoomAt(V.w / 2, V.h / 2, 1 / 1.6);
      else if (k === '0' || k === 'Home') home();
      else used = onKey ? !!onKey(ev) : false;
      if (used) {
        ev.preventDefault();
        ev.stopPropagation();
      }
    });
    let ro = null;
    if (typeof ResizeObserver === 'function') {
      ro = new ResizeObserver(() => { dirty = true; });
      ro.observe(canvas);
    }
    listen(globalThis, 'resize', () => { dirty = true; });

    // ----------------------------------------------------------------------------------------------
    // The handle
    // ----------------------------------------------------------------------------------------------
    const api = {
      setTiles(list) {
        layers = (list || []).map(l => Object.assign({ alpha: 1 }, l));
        dirty = true;
      },
      setProjection(proj, pyr, ex) {
        projection = proj || { period: 0, zRange: null };
        pyramid = pyr || null;
        extent = ex || null;
        resize();
        const keep = [V.cx, V.cz, V.bpp];
        V.bpp = -1;
        setView(keep[0], keep[1], keep[2] > 0 ? keep[2] : 1);
        dirty = true;
      },
      setBackground(c) {
        background = c;
        dirty = true;
      },
      setScaleBar(o) {
        scaleBar = o === false ? null : Object.assign({ right: 12, bottom: 12 }, o || {});
        dirty = true;
      },
      setView(cx, cz, bpp) {
        resize();
        setView(cx, cz, bpp);
      },
      get view() { return view(); },
      get projection() { return projection; },
      get pyramid() { return pyramid; },
      get cursor() { return cursor ? cursor.slice() : null; },
      home() {
        resize();
        home();
      },
      zoomAt,
      toWorld,
      toScreen,
      copies,
      minBpp,
      maxBpp,
      tileZoom() {
        const L0 = layers.find(l => l.visible !== false);
        return L0 ? layerZoom(L0) : pyramid ? zoomFor(pyramid, V.bpp / V.dpr) : null;
      },
      /** Tiles drawn in the last frame, tiles still wanted, tiles loading. */
      stats() {
        return { tiles: stats.tiles, wanted: stats.wanted, inflight, cached: tiles.size, frames: stats.frames };
      },
      /** Nothing loading and nothing missing in the last frame. */
      idle() { return inflight === 0 && stats.wanted === 0 && !dirty; },
      invalidate() { dirty = true; },
      redraw() { draw(); dirty = false; },
      start() {
        if (running || destroyed) return;
        running = true;
        dirty = true;
        raf = requestAnimationFrame(frame);
      },
      stop() {
        running = false;
        if (raf) cancelAnimationFrame(raf);
        raf = 0;
      },
      destroy() {
        api.stop();
        destroyed = true;
        for (const [t, type, fn, o] of L) t.removeEventListener(type, fn, o);
        if (ro) ro.disconnect();
        for (const t of tiles.values()) if (t.img && t.img.close) t.img.close();
        tiles.clear();
      },
    };
    resize();
    if (opts.view) setView(opts.view.cx, opts.view.cz, opts.view.bpp);
    else home();
    if (opts.start !== false) api.start();
    return api;
  }

  Object.assign(MV, { T, wrap, niceStep, fmtInt, zoomFor, tileCover, tileInBounds, copiesIn, createMap });
  if (typeof module === 'object' && module.exports) module.exports = MV;
})();
