// SPDX-License-Identifier: MPL-2.0
// Draws the marks of `/api/marks` (docs/api.md §4) on the map's canvas and hit-tests them: point
// glyphs (turned ones too: stations, signals), legs placed by game time, paths in a line style
// (rail, chain, belt, flow, dash, dot) with arrows and the things on them, edges, areas as outlines
// (a district's chunks), trains as their carriages, cell grids through a colour ramp, cluster
// bubbles and labels kept apart. Colours come from the mark (`c`), else the layer legend (`ci`),
// else the layer style; `ink`, `ink2`, `paper`, `bad`, `warn` and `off` are the page theme's. On a
// wrapping frame every mark is drawn at each of its copies in view. Nothing here knows a layer:
// everything comes from the descriptor (VIEWER.md §4.3). The key's swatches are drawn by the same
// code (`drawSwatch`). A classic script: adds to `globalThis.MicViewer`.
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const FONT = 'system-ui, -apple-system, "Segoe UI", Roboto, "DejaVu Sans", sans-serif';
  const SLOP = 4;   // CSS px of grace around a hit target
  const TAU = Math.PI * 2;
  /** Below this zoom (blocks per CSS px) track is drawn as two rails on sleepers. */
  const RAIL_DETAIL = 0.25;
  /** A train shorter than this on screen (CSS px) is drawn as a chevron. */
  const TRAIN_CHEVRON = 14;

  // ------------------------------------------------------------------------------------------------
  // Theme and colour
  // ------------------------------------------------------------------------------------------------

  const LIGHT = {
    dark: false, ink: '#1f2328', ink2: '#5b6470', paper: '#f1efe9', halo: 'rgba(255, 255, 255, 0.92)',
    bad: '#d1242f', warn: '#b7791f', off: '#8b949e',
  };
  const DARK = {
    dark: true, ink: '#ebe6dc', ink2: '#a59f94', paper: '#14171b', halo: 'rgba(14, 16, 19, 0.9)',
    bad: '#ff6b6b', warn: '#e3b341', off: '#8b949e',
  };
  const TOKENS = new Set(['ink', 'ink2', 'paper', 'halo', 'bad', 'warn', 'off']);
  let lastTheme = null, lastFull = LIGHT;
  /** The page's theme (app.js's `A.theme`) filled in from the defaults; light when none. */
  function themeOf(t) {
    if (!t) return LIGHT;
    if (t === lastTheme) return lastFull;
    lastTheme = t;
    lastFull = Object.assign({}, t.dark ? DARK : LIGHT);
    for (const k of Object.keys(t)) if (t[k] != null) lastFull[k] = t[k];
    return lastFull;
  }
  /** A colour with the theme's tokens (`ink`, `paper`, ...) put in. */
  function resolve(c, th) {
    return TOKENS.has(c) ? themeOf(th)[c] : c;
  }

  /** `[r, g, b]` of `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()` or `rgba()`; grey for anything else. */
  function parseColor(c) {
    const s = String(c || '').trim();
    let m = /^#([0-9a-f]{3})$/i.exec(s);
    if (m) return [...m[1]].map(h => parseInt(h + h, 16));
    m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(s);
    if (m) return [0, 2, 4].map(i => parseInt(m[1].slice(i, i + 2), 16));
    m = /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)/i.exec(s);
    if (m) return [1, 2, 3].map(i => Math.round(Number(m[i])));
    return [136, 136, 136];
  }
  const rgbaCache = new Map();
  /** A CSS colour with alpha `a`. */
  function rgba(c, a) {
    const key = c + '|' + a;
    let v = rgbaCache.get(key);
    if (!v) {
      const [r, g, b] = parseColor(c);
      v = `rgba(${r}, ${g}, ${b}, ${a})`;
      if (rgbaCache.size > 4000) rgbaCache.clear();
      rgbaCache.set(key, v);
    }
    return v;
  }
  /** `c` mixed toward white by `f`. */
  function lighten(c, f) {
    const [r, g, b] = parseColor(c);
    const m = v => Math.round(v + (255 - v) * f);
    return `rgb(${m(r)}, ${m(g)}, ${m(b)})`;
  }
  /** `a` mixed toward `b` by `f`. */
  function mix(a, b, f) {
    const p = parseColor(a), q = parseColor(b);
    const m = i => Math.round(p[i] + (q[i] - p[i]) * f);
    return `rgb(${m(0)}, ${m(1)}, ${m(2)})`;
  }

  /** The legend entry a mark's `ci` names, if any. */
  function legendOf(mark, desc) {
    const lg = desc && desc.legend;
    return mark.ci != null && lg && lg[mark.ci] ? lg[mark.ci] : null;
  }
  /** A mark's colour: its own `c`, else its legend entry's, else the layer style's (tokens kept). */
  function colorOf(mark, desc) {
    if (mark.c) return mark.c;
    const e = legendOf(mark, desc);
    if (e && e.color) return e.color;
    return (desc && desc.style && desc.style.color) || '#cccccc';
  }
  /** A point's or leg's glyph: its own `g`, else its legend entry's, else the layer style's. */
  function glyphOf(mark, desc) {
    if (mark.g) return mark.g;
    const e = legendOf(mark, desc);
    if (e && e.glyph) return e.glyph;
    return (desc && desc.style && desc.style.glyph) || 'circle';
  }
  /** A path's line style: its own `st`, else the layer style's `line`, else plain. */
  function lineOf(mark, desc) {
    return mark.st || (desc && desc.style && desc.style.line) || null;
  }

  /** Colour ramps for `cell` marks, as stops from 0 to 1. */
  const RAMPS = {
    heat: ['#313695', '#4575b4', '#74add1', '#abd9e9', '#e0f3f8', '#fee090', '#fdae61', '#f46d43', '#d73027', '#a50026'],
    viridis: ['#440154', '#482878', '#3e4989', '#31688e', '#26828e', '#1f9e89', '#35b779', '#6ece58', '#b5de2b', '#fde725'],
    density: ['#1b1d2a', '#3b2f5e', '#6b3a7a', '#a3407a', '#d85a5c', '#f58b3f', '#f9c73b', '#fcf7a8'],
    diverging: ['#2166ac', '#67a9cf', '#d1e5f0', '#f7f7f7', '#fddbc7', '#ef8a62', '#b2182b'],
    greys: ['#111111', '#eeeeee'],
  };
  const rampRgb = {};
  /** `[r, g, b]` at `f` (0 to 1) along ramp `name` (viridis when unknown). */
  function rampColor(name, f) {
    const key = RAMPS[name] ? name : 'viridis';
    const stops = rampRgb[key] || (rampRgb[key] = RAMPS[key].map(parseColor));
    const x = clamp(isFinite(f) ? f : 0, 0, 1) * (stops.length - 1);
    const i = Math.min(Math.floor(x), stops.length - 2), k = x - i;
    const a = stops[i], b = stops[i + 1];
    return [0, 1, 2].map(j => Math.round(a[j] + (b[j] - a[j]) * k));
  }

  /** A count for a bubble: 950, 1.2k, 12k, 3.4M. */
  function compact(n) {
    const a = Math.abs(n);
    const f = (v, s) => (v < 10 ? v.toFixed(1).replace(/\.0$/, '') : String(Math.round(v))) + s;
    if (a < 1000) return String(Math.round(n));
    if (a < 1e6) return f(n / 1e3, 'k');
    if (a < 1e9) return f(n / 1e6, 'M');
    return f(n / 1e9, 'G');
  }

  // ------------------------------------------------------------------------------------------------
  // Geometry
  // ------------------------------------------------------------------------------------------------

  /**
   * The place on a leg at time `t`: before `t0` its start, after `t1` its end, between them the
   * matching fraction of the pieces' total length. The same function as Rust's `api::leg_at`.
   */
  function legAt(mark, t) {
    const p = (mark && mark.p) || [];
    const t0 = mark.t0, t1 = mark.t1;
    if (!p.length) return [0, 0];
    const first = p[0], last = p[p.length - 1];
    if (t <= t0 || t1 <= t0) return t1 <= t0 && t >= t1 ? [last[2], last[3]] : [first[0], first[1]];
    if (t >= t1) return [last[2], last[3]];
    let total = 0;
    for (const q of p) total += Math.hypot(q[2] - q[0], q[3] - q[1]);
    if (total <= 0) return [first[0], first[1]];
    let want = total * ((t - t0) / (t1 - t0));
    for (const q of p) {
      const len = Math.hypot(q[2] - q[0], q[3] - q[1]);
      if (want <= len) {
        const f = len > 0 ? want / len : 0;
        return [q[0] + (q[2] - q[0]) * f, q[1] + (q[3] - q[1]) * f];
      }
      want -= len;
    }
    return [last[2], last[3]];
  }

  /** Distance from (px, py) to the segment (ax, ay)-(bx, by). */
  function distToSegment(px, py, ax, ay, bx, by) {
    const dx = bx - ax, dy = by - ay;
    const l2 = dx * dx + dy * dy;
    let f = l2 > 0 ? ((px - ax) * dx + (py - ay) * dy) / l2 : 0;
    f = clamp(f, 0, 1);
    return Math.hypot(px - (ax + f * dx), py - (ay + f * dy));
  }

  /** Distance from (px, py) to a polyline given flat (`[x0, y0, x1, y1, ...]`). */
  function distToFlat(px, py, pts) {
    let d = Infinity;
    if (pts.length === 2) return Math.hypot(px - pts[0], py - pts[1]);
    for (let i = 0; i + 3 < pts.length; i += 2) d = Math.min(d, distToSegment(px, py, pts[i], pts[i + 1], pts[i + 2], pts[i + 3]));
    return d;
  }

  /** Whether (px, py) is inside rings (flat arrays), even-odd. */
  function inRings(px, py, rings) {
    let inside = false;
    for (const r of rings) {
      for (let i = 0, j = r.length - 2; i < r.length; j = i, i += 2) {
        const xi = r[i], yi = r[i + 1], xj = r[j], yj = r[j + 1];
        if (yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) inside = !inside;
      }
    }
    return inside;
  }

  /** A flat polyline's length. */
  function flatLength(f) {
    let s = 0;
    for (let i = 2; i < f.length; i += 2) s += Math.hypot(f[i] - f[i - 2], f[i + 1] - f[i - 1]);
    return s;
  }

  /**
   * Calls `fn(x, y, ux, uy)` every `step` along a flat polyline, starting `first` in; `(ux, uy)`
   * is the unit direction there. At most `max` calls.
   */
  function walk(f, step, first, fn, max) {
    if (!(step > 0)) return;
    let want = first, done = 0;
    max = max || 4000;
    for (let i = 2; i < f.length; i += 2) {
      const ax = f[i - 2], ay = f[i - 1], dx = f[i] - ax, dy = f[i + 1] - ay;
      const len = Math.hypot(dx, dy);
      if (len <= 0) continue;
      const ux = dx / len, uy = dy / len;
      while (want <= len) {
        fn(ax + ux * want, ay + uy * want, ux, uy);
        if (++done >= max) return;
        want += step;
      }
      want -= len;
    }
  }

  /** A flat polyline without the points closer than `minD` to the last one kept (its ends kept). */
  function simplifyFlat(f, minD) {
    if (f.length <= 4) return f;
    const out = [f[0], f[1]];
    const n = f.length;
    for (let i = 2; i < n - 2; i += 2) {
      if (Math.hypot(f[i] - out[out.length - 2], f[i + 1] - out[out.length - 1]) >= minD) out.push(f[i], f[i + 1]);
    }
    const lx = f[n - 2], ly = f[n - 1];
    if (out.length > 2 && Math.hypot(lx - out[out.length - 2], ly - out[out.length - 1]) < minD) out.length -= 2;
    out.push(lx, ly);
    return out;
  }

  /** A flat polyline moved `d` px to its left (as it runs), corners mitred within reason. */
  function offsetFlat(f, d) {
    const n = f.length / 2, out = new Array(f.length);
    const nx = [], ny = [];
    for (let i = 0; i + 1 < n; i++) {
      const dx = f[2 * i + 2] - f[2 * i], dy = f[2 * i + 3] - f[2 * i + 1];
      const len = Math.hypot(dx, dy) || 1;
      nx.push(dy / len);
      ny.push(-dx / len);
    }
    for (let i = 0; i < n; i++) {
      const a = Math.max(0, i - 1), b = Math.min(n - 2, i);
      let mx = nx[a] + nx[b], my = ny[a] + ny[b];
      const ml = Math.hypot(mx, my);
      if (ml < 1e-6) { mx = nx[b]; my = ny[b]; } else { mx /= ml; my /= ml; }
      const cos = mx * nx[b] + my * ny[b];
      const k = d / Math.max(0.5, cos);
      out[2 * i] = f[2 * i] + mx * k;
      out[2 * i + 1] = f[2 * i + 1] + my * k;
    }
    return out;
  }

  /**
   * The target under (px, py) of those a draw recorded (screen CSS px), or null. Glyphs and trains
   * first (the one drawn last, so on top), then lines (the nearest), then areas (the smallest that
   * contains it), then cells (their value).
   */
  function pick(targets, px, py) {
    for (let i = targets.length - 1; i >= 0; i--) {
      const t = targets[i];
      if (t.kind === 'pt' && Math.hypot(px - t.x, py - t.y) <= t.r + SLOP) return t;
      if (t.kind === 'train' && distToFlat(px, py, t.pts) <= t.w / 2 + SLOP) return t;
    }
    let best = null, bestD = Infinity;
    for (const t of targets) {
      let d = Infinity;
      if (t.kind === 'seg') d = distToSegment(px, py, t.ax, t.ay, t.bx, t.by);
      else if (t.kind === 'poly') d = distToFlat(px, py, t.pts);
      else continue;
      if (d <= t.w / 2 + SLOP && d < bestD) { best = t; bestD = d; }
    }
    if (best) return best;
    let bestA = Infinity;
    for (const t of targets) {
      let a = Infinity;
      if (t.kind === 'disc' && Math.hypot(px - t.x, py - t.y) <= t.r) a = t.r * t.r * Math.PI;
      else if (t.kind === 'box' && px >= t.x0 && px <= t.x1 && py >= t.y0 && py <= t.y1) a = (t.x1 - t.x0) * (t.y1 - t.y0);
      else if (t.kind === 'rings' && px >= t.x0 && px <= t.x1 && py >= t.y0 && py <= t.y1 && inRings(px, py, t.rings)) a = t.area;
      else continue;
      if (a < bestA) { best = t; bestA = a; }
    }
    if (best) return best;
    for (let i = targets.length - 1; i >= 0; i--) {
      const t = targets[i];
      if (t.kind !== 'cell') continue;
      const i0 = Math.floor((px - t.x0) / t.cs), j0 = Math.floor((py - t.y0) / t.cs);
      if (i0 < 0 || j0 < 0 || i0 >= t.w || j0 >= t.h) continue;
      const v = t.v[j0 * t.w + i0];
      if (v == null) continue;
      return Object.assign({}, t, { value: v });
    }
    return null;
  }

  // ------------------------------------------------------------------------------------------------
  // Glyphs
  // ------------------------------------------------------------------------------------------------

  /** Builds the path of a plain glyph `g` of radius `r` at (x, y). */
  function glyphPath(ctx, g, x, y, r) {
    ctx.beginPath();
    switch (g) {
      case 'square':
        ctx.rect(x - r * 0.86, y - r * 0.86, r * 1.72, r * 1.72);
        break;
      case 'diamond':
        ctx.moveTo(x, y - r * 1.25);
        ctx.lineTo(x + r, y);
        ctx.lineTo(x, y + r * 1.25);
        ctx.lineTo(x - r, y);
        ctx.closePath();
        break;
      case 'triangle':
        ctx.moveTo(x, y - r * 1.2);
        ctx.lineTo(x + r * 1.1, y + r * 0.8);
        ctx.lineTo(x - r * 1.1, y + r * 0.8);
        ctx.closePath();
        break;
      case 'star': {
        const R = r * 1.3, q = R * 0.45;
        for (let i = 0; i < 10; i++) {
          const a = -Math.PI / 2 + (i * Math.PI) / 5, rr = i % 2 ? q : R;
          if (i) ctx.lineTo(x + rr * Math.cos(a), y + rr * Math.sin(a));
          else ctx.moveTo(x + rr * Math.cos(a), y + rr * Math.sin(a));
        }
        ctx.closePath();
        break;
      }
      case 'cross': {
        const a = r * 1.05, b = r * 0.36;
        ctx.moveTo(x - b, y - a); ctx.lineTo(x + b, y - a); ctx.lineTo(x + b, y - b); ctx.lineTo(x + a, y - b);
        ctx.lineTo(x + a, y + b); ctx.lineTo(x + b, y + b); ctx.lineTo(x + b, y + a); ctx.lineTo(x - b, y + a);
        ctx.lineTo(x - b, y + b); ctx.lineTo(x - a, y + b); ctx.lineTo(x - a, y - b); ctx.lineTo(x - b, y - b);
        ctx.closePath();
        break;
      }
      case 'dot':
        ctx.arc(x, y, r * 0.75, 0, TAU);
        break;
      default:
        ctx.arc(x, y, r, 0, TAU);
    }
  }

  function roundRectPath(ctx, x, y, w, h, r) {
    r = Math.min(r, w / 2, h / 2);
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  /** A state ring round a glyph: red for `bad`, amber for `warn`. */
  function stateRing(ctx, x, y, r, st, th) {
    if (st !== 'bad' && st !== 'warn') return;
    ctx.beginPath();
    ctx.arc(x, y, r + 2.75, 0, TAU);
    ctx.lineWidth = 4;
    ctx.strokeStyle = th.halo;
    ctx.stroke();
    ctx.lineWidth = 2;
    ctx.strokeStyle = st === 'bad' ? th.bad : th.warn;
    ctx.stroke();
  }

  /**
   * Draws glyph `g` of radius `r` at (x, y) in `color` (tokens allowed) on theme `theme`, in state
   * `st` (`off` hollow, `bad` a red ring, `warn` an amber one), turned to heading `a` (radians,
   * clockwise from east) where the glyph has a direction (`station`, `signal`).
   */
  function drawGlyph(ctx, g, x, y, r, color, theme, st, a) {
    const th = themeOf(theme);
    const c = resolve(color, th);
    a = a || 0;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    switch (g) {
      case 'ring':
        ctx.beginPath();
        ctx.arc(x, y, r, 0, TAU);
        ctx.lineWidth = Math.max(2, r * 0.5) + 2.5;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.lineWidth = Math.max(2, r * 0.5);
        ctx.strokeStyle = c;
        ctx.stroke();
        break;
      case 'station': {
        // A capsule along the track: paper inside an ink border (amber inside when a train stands).
        const L = r * 2.7, W = r * 1.45;
        ctx.save();
        ctx.translate(x, y);
        ctx.rotate(a);
        ctx.beginPath();
        roundRectPath(ctx, -L / 2, -W / 2, L, W, W / 2);
        ctx.lineWidth = 4;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.fillStyle = st === 'warn' ? th.warn : th.paper;
        ctx.fill();
        ctx.lineWidth = 1.6;
        ctx.strokeStyle = c;
        ctx.stroke();
        ctx.restore();
        if (st === 'bad') stateRing(ctx, x, y, r * 1.35, st, th);
        return;
      }
      case 'signal': {
        // A bar across the track with a lamp in the state's colour.
        const ux = Math.cos(a), uy = Math.sin(a), nx = -uy, ny = ux;
        const b = r * 1.5;
        ctx.beginPath();
        ctx.moveTo(x - nx * b, y - ny * b);
        ctx.lineTo(x + nx * b, y + ny * b);
        ctx.lineWidth = 3.5;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.lineWidth = 1.5;
        ctx.strokeStyle = th.ink;
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(x + nx * b, y + ny * b, r * 0.75, 0, TAU);
        ctx.fillStyle = c;
        ctx.fill();
        ctx.lineWidth = 1.25;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        return;
      }
      case 'gauge': {
        // A dial: paper face, coloured rim, ink needle.
        ctx.beginPath();
        ctx.arc(x, y, r, 0, TAU);
        ctx.fillStyle = th.paper;
        ctx.fill();
        ctx.lineWidth = 4;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.lineWidth = 2;
        ctx.strokeStyle = c;
        ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(x + r * 0.62, y - r * 0.62);
        ctx.lineWidth = 1.5;
        ctx.strokeStyle = th.ink;
        ctx.stroke();
        break;
      }
      case 'port': {
        // A parcel: a square in the colour with paper tape across it.
        const s = r * 0.9;
        ctx.beginPath();
        roundRectPath(ctx, x - s, y - s, 2 * s, 2 * s, s * 0.25);
        ctx.fillStyle = st === 'off' ? th.paper : c;
        ctx.fill();
        ctx.lineWidth = st === 'off' ? 1.75 : 1.25;
        ctx.strokeStyle = st === 'off' ? c : th.halo;
        ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(x, y - s);
        ctx.lineTo(x, y + s);
        ctx.moveTo(x - s, y - s * 0.15);
        ctx.lineTo(x + s, y - s * 0.15);
        ctx.lineWidth = Math.max(1, s * 0.3);
        ctx.strokeStyle = st === 'off' ? c : th.paper;
        ctx.lineCap = 'butt';
        ctx.stroke();
        break;
      }
      case 'train':
        drawTrainShape(ctx, [[x + r * 1.6, y, x - r * 0.2, y], [x - r * 0.9, y, x - r * 2.2, y]], c, th, st, 1, r * 0.55, r * 0.35);
        return;
      default:
        glyphPath(ctx, g, x, y, r);
        if (st === 'off') {
          ctx.fillStyle = th.paper;
          ctx.fill();
          ctx.lineWidth = 1.75;
          ctx.strokeStyle = c;
          ctx.stroke();
        } else {
          ctx.fillStyle = c;
          ctx.fill();
          ctx.lineWidth = 1.25;
          ctx.strokeStyle = th.halo;
          ctx.stroke();
        }
    }
    stateRing(ctx, x, y, r, st, th);
  }

  // ------------------------------------------------------------------------------------------------
  // Lines
  // ------------------------------------------------------------------------------------------------

  function trace(ctx, f) {
    ctx.beginPath();
    ctx.moveTo(f[0], f[1]);
    for (let i = 2; i < f.length; i += 2) ctx.lineTo(f[i], f[i + 1]);
  }

  /** A filled arrowhead at (x, y) pointing along (ux, uy). */
  function arrowHead(ctx, x, y, ux, uy, s, fill, edge) {
    const nx = -uy, ny = ux;
    ctx.beginPath();
    ctx.moveTo(x + ux * s, y + uy * s);
    ctx.lineTo(x - ux * s * 0.7 + nx * s * 0.75, y - uy * s * 0.7 + ny * s * 0.75);
    ctx.lineTo(x - ux * s * 0.35, y - uy * s * 0.35);
    ctx.lineTo(x - ux * s * 0.7 - nx * s * 0.75, y - uy * s * 0.7 - ny * s * 0.75);
    ctx.closePath();
    if (edge) {
      ctx.lineWidth = 2.5;
      ctx.strokeStyle = edge;
      ctx.stroke();
    }
    ctx.fillStyle = fill;
    ctx.fill();
  }

  /** Half the width, in CSS px, of track as drawn at zoom `bpp` (two rails on sleepers close in). */
  function railHalf(bpp, w) {
    w = w || 2;
    if (bpp <= RAIL_DETAIL) return Math.max(0.62 / bpp, 0.36 / bpp + Math.max(1.25, 0.13 / bpp) / 2 + 1);
    return ((bpp < 2 ? Math.max(w, Math.min(5, w * 1.6)) : w) + 3) / 2;
  }

  /** How wide a line of style `style` and width `w` is drawn at `bpp`, its halo included. */
  function drawnWidth(style, w, bpp) {
    switch (style) {
      case 'rail': return 2 * railHalf(bpp, w);
      case 'belt': return clamp(0.8 / bpp, w + 1.5, 16) + 2;
      case 'flow': return flowWidth(w) + 3;
      default: return w + 2.5;
    }
  }

  /** A goods flow's drawn width: the mark's own, never thinner than 2 px. */
  const flowWidth = w => Math.max(2, w || 2);

  /**
   * How far, in CSS px, a goods flow of width `w` is drawn to the right of the track it follows at
   * `bpp`: clear of the track's casing, so the two directions between a pair of stations part.
   */
  function flowOffset(bpp, w) {
    return railHalf(bpp) + flowWidth(w) / 2 + 2;
  }

  /** The halo as an opaque colour (the gap in a selection's casing must hide what is under it). */
  const solidHalo = th => rgba(th.halo, 1);

  /**
   * The casing that marks a selected (`h` 2) or hovered (`h` 1) line under it: an ink band round
   * the line's drawn width `dw`, parted from the line by a halo. The path must be built.
   */
  function casing(ctx, dw, h, th) {
    ctx.lineWidth = dw + (h > 1 ? 9 : 6);
    ctx.strokeStyle = h > 1 ? th.ink : rgba(th.ink, 0.55);
    ctx.stroke();
    ctx.lineWidth = dw + (h > 1 ? 4.5 : 3);
    ctx.strokeStyle = solidHalo(th);
    ctx.stroke();
  }

  /**
   * Strokes the flat screen polyline `f` in line style `o.line` (`rail`, `chain`, `belt`, `flow`,
   * `dash`, `dot`, or plain) of colour `o.color` and width `o.w`, at zoom `o.bpp`, with arrows
   * (`o.arrow`) and `o.n` things spread along it; `o.hi` 1 (hovered) or 2 (selected) puts a casing
   * under it.
   */
  function strokeLine(ctx, f, o) {
    const th = themeOf(o.theme);
    const c = resolve(o.color, th);
    const bpp = o.bpp || 1;
    let w = o.w || 2;
    if (f.length < 4) return;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.setLineDash([]);
    const style = o.line || '';
    if (style === 'flow') w = flowWidth(w);
    if (o.hi) {
      trace(ctx, f);
      casing(ctx, drawnWidth(style, w, bpp), o.hi, th);
    }
    switch (style) {
      case 'rail': {
        if (bpp <= RAIL_DETAIL) {
          // Sleepers every block, two rails 0.72 blocks apart.
          const half = 0.36 / bpp, sl = 0.62 / bpp;
          ctx.beginPath();
          walk(f, 1 / bpp, 0.5 / bpp, (x, y, ux, uy) => {
            ctx.moveTo(x + uy * sl, y - ux * sl);
            ctx.lineTo(x - uy * sl, y + ux * sl);
          }, 20000);
          ctx.lineCap = 'butt';
          ctx.lineWidth = Math.max(1, 0.24 / bpp);
          ctx.strokeStyle = rgba(th.ink2, 0.55);
          ctx.stroke();
          ctx.lineCap = 'round';
          const rw = Math.max(1.25, 0.13 / bpp);
          for (const side of [half, -half]) {
            trace(ctx, offsetFlat(f, side));
            ctx.lineWidth = rw + 2;
            ctx.strokeStyle = th.halo;
            ctx.stroke();
            ctx.lineWidth = rw;
            ctx.strokeStyle = c;
            ctx.stroke();
          }
        } else {
          if (bpp < 2) w = Math.max(w, Math.min(5, w * 1.6));
          trace(ctx, f);
          ctx.lineWidth = w + 3;
          ctx.strokeStyle = th.halo;
          ctx.stroke();
          ctx.lineWidth = w;
          ctx.strokeStyle = c;
          ctx.stroke();
        }
        break;
      }
      case 'chain': {
        trace(ctx, f);
        ctx.lineWidth = w + 2.5;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.setLineDash([4, 3]);
        ctx.lineCap = 'butt';
        ctx.lineWidth = w;
        ctx.strokeStyle = c;
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.lineCap = 'round';
        // The pulleys at its ends, close up.
        if (bpp <= 2) {
          for (const i of [0, f.length - 2]) {
            ctx.beginPath();
            ctx.arc(f[i], f[i + 1], Math.max(2.5, 0.45 / bpp), 0, TAU);
            ctx.fillStyle = th.paper;
            ctx.fill();
            ctx.lineWidth = 1.5;
            ctx.strokeStyle = c;
            ctx.stroke();
          }
        }
        break;
      }
      case 'belt': {
        const bw = clamp(0.8 / bpp, w + 1.5, 16);
        trace(ctx, f);
        ctx.lineCap = 'butt';
        ctx.lineWidth = bw + 2;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.lineWidth = bw;
        ctx.strokeStyle = c;
        ctx.stroke();
        ctx.lineCap = 'round';
        if (o.arrow) {
          const s = bw * 0.32;
          ctx.beginPath();
          walk(f, Math.max(9, bw * 1.6), Math.max(5, bw * 0.8), (x, y, ux, uy) => {
            const nx = -uy, ny = ux;
            ctx.moveTo(x - ux * s + nx * s, y - uy * s + ny * s);
            ctx.lineTo(x, y);
            ctx.lineTo(x - ux * s - nx * s, y - uy * s - ny * s);
          });
          ctx.lineWidth = Math.max(1, bw * 0.16);
          ctx.strokeStyle = th.paper;
          ctx.stroke();
        }
        break;
      }
      case 'flow': {
        // Goods on the move: solid colour on a halo (drawn beside the track by the caller).
        trace(ctx, f);
        ctx.lineWidth = w + 3;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.lineWidth = w;
        ctx.strokeStyle = c;
        ctx.stroke();
        break;
      }
      case 'dash':
      case 'dot': {
        trace(ctx, f);
        ctx.lineWidth = w + 2.5;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.setLineDash(style === 'dash' ? [6, 4] : [0.1, w + 3]);
        ctx.lineCap = style === 'dash' ? 'butt' : 'round';
        ctx.lineWidth = w;
        ctx.strokeStyle = c;
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.lineCap = 'round';
        break;
      }
      default: {
        trace(ctx, f);
        ctx.lineWidth = w + 2.5;
        ctx.strokeStyle = th.halo;
        ctx.stroke();
        ctx.lineWidth = w;
        ctx.strokeStyle = c;
        ctx.stroke();
      }
    }
    if (o.arrow && style !== 'belt') {
      const len = flatLength(f);
      const step = style === 'flow' ? 120 : 80;
      const s = style === 'flow' ? clamp(w * 1.1 + 3, 5, 10) : Math.max(4, Math.min(9, w * 1.8));
      walk(f, step, Math.min(step / 2, len / 2), (x, y, ux, uy) => arrowHead(ctx, x, y, ux, uy, s, c, th.halo));
    }
    if (o.n > 0) {
      // The things on it, spread along it (one dot per 7 px at most).
      const len = flatLength(f);
      const k = Math.min(o.n, Math.max(1, Math.floor(len / 7)));
      const step = len / (k + 1);
      ctx.beginPath();
      walk(f, step, step, (x, y) => {
        ctx.moveTo(x + 2.6, y);
        ctx.arc(x, y, 2.6, 0, TAU);
      }, k);
      ctx.fillStyle = c;
      ctx.fill();
      ctx.lineWidth = 1.25;
      ctx.strokeStyle = th.paper;
      ctx.stroke();
    }
  }

  // ------------------------------------------------------------------------------------------------
  // Trains
  // ------------------------------------------------------------------------------------------------

  /**
   * Draws carriages `cars` (screen px, `[x0, y0, x1, y1]` front bogey to back, leading carriage
   * first) as boxes `hw` px half wide reaching `ext` px past their bogeys, the leading one pointed.
   */
  function drawTrainShape(ctx, cars, color, th, st, hi, hw, ext) {
    const fill = st === 'bad' ? th.bad : st === 'off' ? th.off : color;
    ctx.lineJoin = 'round';
    cars.forEach((q, k) => {
      let dx = q[0] - q[2], dy = q[1] - q[3];
      let len = Math.hypot(dx, dy);
      if (len < 1e-6) { dx = 1; dy = 0; len = 1; }
      const ux = dx / len, uy = dy / len, nx = -uy, ny = ux;
      const fx = q[0] + ux * ext, fy = q[1] + uy * ext, bx = q[2] - ux * ext, by = q[3] - uy * ext;
      ctx.beginPath();
      if (k === 0) {
        const nose = Math.min(hw * 1.6, (len + 2 * ext) * 0.45);
        ctx.moveTo(fx, fy);
        ctx.lineTo(fx - ux * nose + nx * hw, fy - uy * nose + ny * hw);
        ctx.lineTo(bx + nx * hw, by + ny * hw);
        ctx.lineTo(bx - nx * hw, by - ny * hw);
        ctx.lineTo(fx - ux * nose - nx * hw, fy - uy * nose - ny * hw);
      } else {
        ctx.moveTo(fx + nx * hw, fy + ny * hw);
        ctx.lineTo(bx + nx * hw, by + ny * hw);
        ctx.lineTo(bx - nx * hw, by - ny * hw);
        ctx.lineTo(fx - nx * hw, fy - ny * hw);
      }
      ctx.closePath();
      ctx.lineWidth = 2.5;
      ctx.strokeStyle = th.halo;
      ctx.stroke();
      ctx.fillStyle = fill;
      ctx.fill();
    });
  }

  /** A chevron at (x, y) pointing along (ux, uy): a train seen from far out. */
  function drawChevron(ctx, x, y, ux, uy, s, color, th, st) {
    const fill = st === 'bad' ? th.bad : st === 'off' ? th.off : color;
    const nx = -uy, ny = ux;
    ctx.beginPath();
    ctx.moveTo(x + ux * s, y + uy * s);
    ctx.lineTo(x - ux * s + nx * s * 0.85, y - uy * s + ny * s * 0.85);
    ctx.lineTo(x - ux * s * 0.35, y - uy * s * 0.35);
    ctx.lineTo(x - ux * s - nx * s * 0.85, y - uy * s - ny * s * 0.85);
    ctx.closePath();
    ctx.lineJoin = 'round';
    ctx.lineWidth = 3;
    ctx.strokeStyle = th.halo;
    ctx.stroke();
    ctx.fillStyle = fill;
    ctx.fill();
  }

  // ------------------------------------------------------------------------------------------------
  // Marks
  // ------------------------------------------------------------------------------------------------

  /** A mark's east-west block extent `[x0, x1]` (for its copies across a wrap). */
  function xExtent(m) {
    switch (m.k) {
      case 'leg': {
        let a = Infinity, b = -Infinity;
        for (const q of m.p || []) { a = Math.min(a, q[0], q[2]); b = Math.max(b, q[0], q[2]); }
        return a <= b ? [a, b] : [0, 0];
      }
      case 'train': {
        let a = Infinity, b = -Infinity;
        for (const q of m.cars || []) { a = Math.min(a, q[0], q[2]); b = Math.max(b, q[0], q[2]); }
        return a <= b ? [a, b] : [0, 0];
      }
      case 'path': {
        let a = Infinity, b = -Infinity;
        for (const q of m.p || []) { a = Math.min(a, q[0]); b = Math.max(b, q[0]); }
        return a <= b ? [a, b] : [0, 0];
      }
      case 'edge': return [Math.min(m.a[0], m.b[0]), Math.max(m.a[0], m.b[0])];
      case 'area':
        if (m.box) return [m.box[0], m.box[2]];
        if (m.rings) {
          let a = Infinity, b = -Infinity;
          for (const r of m.rings) for (const q of r) { a = Math.min(a, q[0]); b = Math.max(b, q[0]); }
          if (a <= b) return [a, b];
        }
        return [m.x - (m.r || 0), m.x + (m.r || 0)];
      case 'cell': return [m.x0, m.x0 + m.size * m.w];
      default: return [m.x, m.x];
    }
  }

  function cellColors(m) {
    if (m._rgb) return m._rgb;
    const lo = m.lo != null ? m.lo : Math.min(...m.v.filter(v => v != null));
    const hi = m.hi != null ? m.hi : Math.max(...m.v.filter(v => v != null));
    const span = hi - lo || 1;
    m._rgb = m.v.map(v => {
      if (v == null) return null;
      const [r, g, b] = rampColor(m.ramp || 'viridis', (v - lo) / span);
      return `rgb(${r}, ${g}, ${b})`;
    });
    return m._rgb;
  }

  /** How much bigger glyphs are drawn close in: 1 at 2 blocks a pixel and out, up to 2. */
  function glyphScale(bpp) {
    return bpp >= 2 ? 1 : clamp(1 + 0.35 * Math.log2(2 / bpp), 1, 2);
  }

  /** The outline style of an area's `st`. */
  function outlineOf(st, c, th) {
    switch (st) {
      case 'dash': return { color: c, dash: [7, 4], w: 1.75 };
      case 'dot': return { color: th.off, dash: [0.1, 4], w: 2 };
      case 'bad': return { color: th.bad, dash: null, w: 2.25 };
      default: return { color: c, dash: null, w: 1.75 };
    }
  }

  /** How far past its radius a glyph `g` reaches (a star's points, a signal's lamp, a station). */
  const GLYPH_REACH = { station: 1.4, signal: 2.25, star: 1.3, diamond: 1.25, triangle: 1.2, cross: 1.05 };

  /**
   * Draws `list` (`[{ desc, data }]`, `data` a layer of `/api/marks`, bottom first) on `ctx` (CSS px)
   * for `map` at game time `opts.t` in theme `opts.theme` (app.js's: `ink`, `paper`, `halo`, ...).
   * `opts.selected` / `opts.hovered`: `{ layer, id }` drawn highlighted. Returns `{ targets, drawn,
   * tracks, seen, counts, clusters }`: what `pick` hit-tests, the marks drawn, the screen segments of
   * leg tracks (for checks), per layer id the legend entries drawn (a Set of indices, -1 for the
   * layer style), per layer id how many of its things are on screen (a cluster's count included),
   * and how many cluster bubbles were drawn.
   */
  function drawMarks(ctx, map, list, opts) {
    opts = opts || {};
    const th = themeOf(opts.theme);
    const v = map.view;
    const bpp = v.bpp, W = v.w, H = v.h;
    const left = v.cx - (W / 2) * bpp, top = v.cz - (H / 2) * bpp;
    const t = opts.t || 0;
    const sel = opts.selected, hov = opts.hovered;
    const targets = [], tracks = [], labels = [];
    const seen = new Map(), counts = new Map();
    let drawn = 0, clusters = 0;
    const X = x => (x - left) / bpp, Y = z => (z - top) / bpp;
    const margin = 64;
    const visible = (x0, y0, x1, y1) => x1 >= -margin && x0 <= W + margin && y1 >= -margin && y0 <= H + margin;
    const onScreen = (x0, y0, x1, y1) => x1 >= 0 && x0 <= W && y1 >= 0 && y0 <= H;
    const halos = [];
    const gscale = glyphScale(bpp);
    // The screen boxes of the symbols drawn: no label is put over one.
    const glyphs = boxGrid(W, H, 48);
    // Cluster bubbles placed so far (all layers): a new one is moved clear of them.
    const bubbles = [];

    let order = 0;
    for (const { desc, data } of list) {
      if (!data || !data.marks) continue;
      const style = desc.style || { color: '#ccc', glyph: 'circle', size: 6, width: 2 };
      const cards = desc.has_cards !== false;
      const lid = desc.id;
      const pri = desc.priority || 0;
      const lg = desc.legend || [];
      const showLabels = style.label_bpp != null && bpp <= style.label_bpp;
      let inView = 0, counted = false;
      // Counts a mark once (whichever copy is on screen first), `n` things for a cluster.
      const tally = (x0, y0, x1, y1, n) => {
        if (counted || !onScreen(x0, y0, x1, y1)) return;
        counted = true;
        inView += n || 1;
      };
      let s = seen.get(lid);
      const note = (m, extra) => {
        if (!s) seen.set(lid, (s = new Set()));
        if (m.ci != null && lg[m.ci]) s.add(m.ci);
        else s.add(-1);
        if (m.g) {
          const i = lg.findIndex(e => e.glyph === m.g && !e.st);
          if (i >= 0) s.add(i);
        }
        if (extra) {
          const i = lg.findIndex(e => e.glyph === extra);
          if (i >= 0) s.add(i);
        }
      };
      const hiOf = m => {
        if (m.id == null) return 0;
        const id = String(m.id);
        if (sel && sel.layer === lid && String(sel.id) === id) return 2;
        if (hov && hov.layer === lid && String(hov.id) === id) return 1;
        return 0;
      };
      for (const m of data.marks) {
        const [ex0, ex1] = xExtent(m);
        const offs = map.copies((ex0 + ex1) / 2, (ex1 - ex0) / 2 + margin * bpp);
        counted = m.k === 'cell';
        for (const off of offs) {
          const o = off / bpp;
          order++;
          switch (m.k) {
            case 'point': {
              const x = X(m.x) + o, y = Y(m.z);
              if (!visible(x, y, x, y)) break;
              const r = Math.max(2.5, ((style.size || 6) * (m.s || 1) * gscale) / 2);
              const g = glyphOf(m, desc);
              drawGlyph(ctx, g, x, y, r, colorOf(m, desc), th, m.st, m.a);
              if (m.ring) {
                ctx.beginPath();
                ctx.arc(x, y, r + 3.5, 0, TAU);
                ctx.lineWidth = 1.5;
                ctx.strokeStyle = th.ink;
                ctx.stroke();
              }
              const reach = m.ring || m.st === 'bad' || m.st === 'warn' ? r + 5 : r * (GLYPH_REACH[g] || 1) + 1;
              glyphs.add([x - reach, y - reach, x + reach, y + reach]);
              const h = hiOf(m);
              if (h) halos.push([x, y, g === 'station' ? r * 1.5 : r, h]);
              if (m.id != null && cards) targets.push({ kind: 'pt', layer: lid, id: String(m.id), label: m.l, x, y, r: g === 'station' ? r * 1.35 : r, order });
              if (m.l && showLabels) labels.push({ kind: 'pt', x, y, r: Math.max(reach - 1, g === 'station' ? r * 1.4 : r), text: m.l, pri });
              tally(x, y, x, y);
              note(m);
              drawn++;
              break;
            }
            case 'leg': {
              const c = resolve(colorOf(m, desc), th);
              ctx.beginPath();
              for (const q of m.p || []) {
                const ax = X(q[0]) + o, ay = Y(q[1]), bx = X(q[2]) + o, by = Y(q[3]);
                ctx.moveTo(ax, ay);
                ctx.lineTo(bx, by);
                if (tracks.length < 5000) tracks.push([lid, m.id, ax, ay, bx, by]);
              }
              ctx.lineWidth = 1.5;
              ctx.lineCap = 'round';
              ctx.strokeStyle = rgba(c, 0.32);
              ctx.stroke();
              const [px, pz] = legAt(m, t);
              const x = X(px) + o, y = Y(pz);
              if (!visible(x, y, x, y)) break;
              const r = Math.max(2.5, (style.size || 6) / 2 + 0.5);
              drawGlyph(ctx, glyphOf(m, desc), x, y, r, c, th);
              glyphs.add([x - r - 1, y - r - 1, x + r + 1, y + r + 1]);
              const h = hiOf(m);
              if (h) halos.push([x, y, r, h]);
              if (m.id != null && cards) targets.push({ kind: 'pt', layer: lid, id: String(m.id), label: m.l, x, y, r, order });
              if (m.l && showLabels) labels.push({ kind: 'pt', x, y, r, text: m.l, pri });
              tally(x, y, x, y);
              note(m);
              drawn++;
              break;
            }
            case 'path':
            case 'edge': {
              const pts = m.k === 'edge' ? (m.a && m.b ? [m.a, m.b] : []) : m.p || [];
              if (pts.length < 2) break;
              let f = new Array(pts.length * 2);
              let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
              for (let i = 0; i < pts.length; i++) {
                const x = X(pts[i][0]) + o, y = Y(pts[i][1]);
                f[2 * i] = x;
                f[2 * i + 1] = y;
                if (x < x0) x0 = x;
                if (x > x1) x1 = x;
                if (y < y0) y0 = y;
                if (y > y1) y1 = y;
              }
              if (!visible(x0, y0, x1, y1)) break;
              let w = m.w || style.width || 2;
              const line = m.k === 'edge' ? style.line || null : lineOf(m, desc);
              if (line === 'flow') {
                // A goods flow runs beside the track it follows, on its right as it goes (first point
                // to last), so the two directions between a pair of stations show apart.
                w = flowWidth(w);
                f = simplifyFlat(f, 1.5);
                if (f.length >= 4) f = offsetFlat(f, -flowOffset(bpp, w));
              }
              const arrow = m.arrow != null ? !!m.arrow : line === 'flow';
              strokeLine(ctx, f, { line, color: colorOf(m, desc), w, bpp, theme: th, arrow, n: m.n, hi: hiOf(m) });
              const tw = line === 'belt' ? clamp(0.8 / bpp, w + 1.5, 16) : w;
              if (m.id != null && cards) {
                if (m.k === 'edge') targets.push({ kind: 'seg', layer: lid, id: String(m.id), label: m.l, ax: f[0], ay: f[1], bx: f[f.length - 2], by: f[f.length - 1], w: Math.max(tw, 4), order });
                else targets.push({ kind: 'poly', layer: lid, id: String(m.id), label: m.l, pts: f, w: Math.max(tw, 4), order });
              }
              if (m.l && showLabels) labels.push(lineLabel(f, m.l, pri, tw));
              tally(x0, y0, x1, y1);
              note(m, m.n > 0 ? 'dot' : null);
              drawn++;
              break;
            }
            case 'train': {
              const cars = (m.cars || []).map(q => [X(q[0]) + o, Y(q[1]), X(q[2]) + o, Y(q[3])]);
              if (!cars.length) break;
              let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity, len = 0;
              for (const q of cars) {
                x0 = Math.min(x0, q[0], q[2]); x1 = Math.max(x1, q[0], q[2]);
                y0 = Math.min(y0, q[1], q[3]); y1 = Math.max(y1, q[1], q[3]);
                len += Math.hypot(q[2] - q[0], q[3] - q[1]);
              }
              if (!visible(x0, y0, x1, y1)) break;
              const c = resolve(colorOf(m, desc), th);
              const h = hiOf(m);
              const hw = Math.max(3, 1.15 / bpp), ext = Math.max(1, 0.9 / bpp);
              const front = cars[0];
              let ux = front[0] - front[2], uy = front[1] - front[3];
              const ul = Math.hypot(ux, uy);
              if (ul > 1e-6) { ux /= ul; uy /= ul; } else { ux = 1; uy = 0; }
              const flat = [];
              for (const q of cars) flat.push(q[0], q[1], q[2], q[3]);
              let r;
              if (len + 2 * ext * cars.length < TRAIN_CHEVRON) {
                r = 7;
                if (h) halos.push([front[0], front[1], r, h]);
                drawChevron(ctx, front[0], front[1], ux, uy, r, c, th, m.st);
                glyphs.add([front[0] - r - 1.5, front[1] - r - 1.5, front[0] + r + 1.5, front[1] + r + 1.5]);
                if (m.id != null && cards) targets.push({ kind: 'pt', layer: lid, id: String(m.id), label: m.l, x: front[0], y: front[1], r, order });
              } else {
                r = hw;
                // The carriages from nose to tail, for the casing round a selected train.
                const back = cars[cars.length - 1];
                let bx = back[2] - back[0], by = back[3] - back[1];
                const bl = Math.hypot(bx, by);
                if (bl > 1e-6) { bx /= bl; by /= bl; } else { bx = -ux; by = -uy; }
                const body = [front[0] + ux * ext, front[1] + uy * ext].concat(flat, [back[2] + bx * ext, back[3] + by * ext]);
                if (h) {
                  trace(ctx, body);
                  ctx.lineCap = 'round';
                  ctx.lineJoin = 'round';
                  casing(ctx, 2 * hw + 2.5, h, th);
                }
                drawTrainShape(ctx, cars, c, th, m.st, h, hw, ext);
                // Its boxes for the labels: a box every carriage width along it.
                walk(body, Math.max(2, hw * 1.5), 0, (x, y) => glyphs.add([x - hw - 1, y - hw - 1, x + hw + 1, y + hw + 1]), 400);
                glyphs.add([body[0] - hw - 1, body[1] - hw - 1, body[0] + hw + 1, body[1] + hw + 1]);
                if (m.id != null && cards) targets.push({ kind: 'train', layer: lid, id: String(m.id), label: m.l, pts: flat, w: 2 * hw, x: front[0], y: front[1], order });
              }
              tally(x0, y0, x1, y1);
              // Ahead of the train first, so the name never covers its carriages.
              const ahead = Math.abs(ux) >= Math.abs(uy) ? (ux > 0 ? 0 : 1) : (uy > 0 ? 2 : 3);
              if (m.l && showLabels) labels.push({ kind: 'pt', x: front[0] + ux * ext, y: front[1] + uy * ext, r: r + 2, text: m.l, pri: pri + 1, prefer: ahead });
              note(m);
              drawn++;
              break;
            }
            case 'area': {
              const c = resolve(colorOf(m, desc), th);
              const h = hiOf(m);
              const ol = outlineOf(m.st, c, th);
              const edge = (fillA) => {
                ctx.fillStyle = rgba(ol.color, h ? fillA * 2.2 : fillA);
                ctx.fill('evenodd');
                ctx.lineJoin = 'miter';
                // A selected or hovered area: a casing under its outline.
                if (h) casing(ctx, ol.w + 3.75, h, th);
                ctx.lineWidth = (h ? ol.w + 1.25 : ol.w) + 2.5;
                ctx.strokeStyle = th.halo;
                ctx.stroke();
                if (ol.dash) ctx.setLineDash(ol.dash);
                ctx.lineCap = ol.dash && ol.dash[0] < 1 ? 'round' : 'butt';
                ctx.lineWidth = h ? ol.w + 1.25 : ol.w;
                ctx.strokeStyle = ol.color;
                ctx.stroke();
                ctx.setLineDash([]);
              };
              if (m.rings && m.rings.length) {
                const rings = [];
                let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity, area = 0;
                for (const ring of m.rings) {
                  const f = [];
                  for (const q of ring) {
                    const x = X(q[0]) + o, y = Y(q[1]);
                    f.push(x, y);
                    x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y);
                  }
                  for (let i = 0, j = f.length - 2; i < f.length; j = i, i += 2) area += (f[j] + f[i]) * (f[j + 1] - f[i + 1]);
                  rings.push(f);
                }
                if (!visible(x0, y0, x1, y1)) break;
                ctx.beginPath();
                for (const f of rings) {
                  ctx.moveTo(f[0], f[1]);
                  for (let i = 2; i < f.length; i += 2) ctx.lineTo(f[i], f[i + 1]);
                  ctx.closePath();
                }
                edge(0.06);
                if (m.id != null && cards) targets.push({ kind: 'rings', layer: lid, id: String(m.id), label: m.l, rings, x0, y0, x1, y1, area: Math.abs(area / 2), order });
                if (m.l && x1 - x0 > 50) labels.push(areaLabel(rings, x0, y0, x1, y1, m.l, c, pri, th));
                tally(x0, y0, x1, y1);
              } else if (m.box) {
                let x0 = X(m.box[0]) + o, y0 = Y(m.box[1]), x1 = X(m.box[2]) + o, y1 = Y(m.box[3]);
                if (!visible(x0, y0, x1, y1)) break;
                if (x1 - x0 < 3) { const mx = (x0 + x1) / 2; x0 = mx - 1.5; x1 = mx + 1.5; }
                if (y1 - y0 < 3) { const my = (y0 + y1) / 2; y0 = my - 1.5; y1 = my + 1.5; }
                ctx.beginPath();
                ctx.rect(x0, y0, x1 - x0, y1 - y0);
                edge(0.06);
                if (m.id != null && cards) targets.push({ kind: 'box', layer: lid, id: String(m.id), label: m.l, x0, y0, x1, y1, order });
                if (m.l && x1 - x0 > 50) labels.push({ kind: 'area', x: (x0 + x1) / 2, y: (y0 + y1) / 2, text: m.l, color: mix(c, th.ink, 0.45), pri });
                tally(x0, y0, x1, y1);
              } else {
                const x = X(m.x) + o, y = Y(m.z), r = Math.max(1.5, (m.r || 0) / bpp);
                if (!visible(x - r, y - r, x + r, y + r)) break;
                ctx.beginPath();
                ctx.arc(x, y, r, 0, TAU);
                edge(0.07);
                if (m.id != null && cards) targets.push({ kind: 'disc', layer: lid, id: String(m.id), label: m.l, x, y, r, order });
                if (m.l && r > 30) labels.push({ kind: 'area', x, y, text: m.l, color: mix(c, th.ink, 0.45), pri });
                tally(x - r, y - r, x + r, y + r);
              }
              note(m);
              drawn++;
              break;
            }
            case 'cell': {
              const cs = m.size / bpp;
              const x0 = X(m.x0) + o, y0 = Y(m.z0);
              if (!visible(x0, y0, x0 + cs * m.w, y0 + cs * m.h)) break;
              const cols = cellColors(m);
              ctx.globalAlpha = 0.5;
              const pad = cs > 3 ? 0.5 : 0;
              for (let j = 0; j < m.h; j++) {
                const y = y0 + j * cs;
                if (y + cs < 0 || y > H) continue;
                for (let i = 0; i < m.w; i++) {
                  const col = cols[j * m.w + i];
                  if (!col) continue;
                  const x = x0 + i * cs;
                  if (x + cs < 0 || x > W) continue;
                  ctx.fillStyle = col;
                  ctx.fillRect(x, y, cs + pad, cs + pad);
                }
              }
              ctx.globalAlpha = 1;
              targets.push({ kind: 'cell', layer: lid, id: null, label: desc.label, x0, y0, cs, w: m.w, h: m.h, v: m.v, order });
              note(m);
              drawn++;
              break;
            }
            case 'cluster': {
              const x0 = X(m.x) + o, y0 = Y(m.z);
              if (!visible(x0, y0, x0, y0)) break;
              const r = clamp(9 + 3.4 * Math.log10(Math.max(1, m.n)), 10, 22);
              // Moved clear of the bubbles already drawn, so no count hides another.
              const [x, y] = clearOf(bubbles, x0, y0, r);
              bubbles.push([x, y, r]);
              glyphs.add([x - r - 1, y - r - 1, x + r + 1, y + r + 1]);
              tally(x0, y0, x0, y0, m.n);
              clusters++;
              ctx.beginPath();
              ctx.arc(x, y, r, 0, TAU);
              ctx.fillStyle = th.paper;
              ctx.fill();
              const by = m.by && Object.keys(m.by).length ? m.by : null;
              ctx.lineWidth = 3;
              if (by) {
                const sum = Object.values(by).reduce((a, b) => a + b, 0) || 1;
                let a0 = -Math.PI / 2;
                for (const [k, n] of Object.entries(by)) {
                  const a1 = a0 + (n / sum) * TAU;
                  const e = lg[Number(k)];
                  ctx.beginPath();
                  ctx.arc(x, y, r - 1.5, a0, a1);
                  ctx.strokeStyle = resolve((e && e.color) || style.color, th);
                  ctx.stroke();
                  a0 = a1;
                  if (e) {
                    if (!s) seen.set(lid, (s = new Set()));
                    s.add(Number(k));
                  }
                }
              } else {
                ctx.beginPath();
                ctx.arc(x, y, r - 1.5, 0, TAU);
                ctx.strokeStyle = resolve(style.color, th);
                ctx.stroke();
                if (!s) seen.set(lid, (s = new Set()));
                s.add(-1);
              }
              ctx.font = `600 ${r > 14 ? 12 : 11}px ${FONT}`;
              ctx.textAlign = 'center';
              ctx.textBaseline = 'middle';
              ctx.fillStyle = th.ink;
              ctx.fillText(compact(m.n), x, y + 0.5);
              targets.push({ kind: 'pt', layer: lid, id: null, cluster: true, n: m.n, wx: m.x + off, wz: m.z, label: null, x, y, r, order });
              drawn++;
              break;
            }
            case 'label': {
              const x = X(m.x) + o, y = Y(m.z);
              if (!visible(x, y, x, y)) break;
              labels.push({ kind: 'text', x, y, text: m.text, pri: pri + 1000 });
              tally(x, y, x, y);
              drawn++;
              break;
            }
            default:
              break;
          }
        }
      }
      counts.set(lid, inView);
    }

    // Highlights of the selected and hovered things, over every mark.
    for (const [x, y, r, h] of halos) {
      ctx.beginPath();
      ctx.arc(x, y, r + 5, 0, TAU);
      ctx.lineWidth = 4.5;
      ctx.strokeStyle = th.halo;
      ctx.stroke();
      ctx.lineWidth = h > 1 ? 2.25 : 1.5;
      ctx.strokeStyle = h > 1 ? th.ink : rgba(th.ink, 0.6);
      ctx.stroke();
    }

    placeLabels(ctx, labels, W, H, th, glyphs);
    return { targets, drawn, tracks, seen, counts, clusters };
  }

  /** (x, y) moved the least way that keeps a circle of radius `r` there clear of `placed`
   *  (`[x, y, r]` each), a few rounds at most. */
  function clearOf(placed, x, y, r) {
    for (let round = 0; round < 6; round++) {
      let moved = false;
      for (const [bx, by, br] of placed) {
        const need = r + br + 2;
        let dx = x - bx, dy = y - by;
        const d = Math.hypot(dx, dy);
        if (d >= need) continue;
        if (d < 0.01) { dx = 1; dy = 0.35; }
        const l = Math.hypot(dx, dy);
        x = bx + (dx / l) * need;
        y = by + (dy / l) * need;
        moved = true;
      }
      if (!moved) break;
    }
    return [x, y];
  }

  /**
   * Screen boxes `[x0, y0, x1, y1]` in a grid of `cell` px over a `W` x `H` screen, for asking
   * whether a box meets any of them. Boxes wholly off screen are not kept.
   */
  function boxGrid(W, H, cell) {
    const nx = Math.max(1, Math.ceil(W / cell)), ny = Math.max(1, Math.ceil(H / cell));
    const cells = new Array(nx * ny);
    const ci = v => clamp(Math.floor(v / cell), 0, nx - 1), cj = v => clamp(Math.floor(v / cell), 0, ny - 1);
    let size = 0;
    return {
      get size() { return size; },
      add(b) {
        if (b[2] < 0 || b[3] < 0 || b[0] > W || b[1] > H) return;
        size++;
        for (let j = cj(b[1]), j1 = cj(b[3]); j <= j1; j++) {
          for (let i = ci(b[0]), i1 = ci(b[2]); i <= i1; i++) {
            const k = j * nx + i;
            (cells[k] || (cells[k] = [])).push(b);
          }
        }
      },
      hits(b) {
        for (let j = cj(b[1]), j1 = cj(b[3]); j <= j1; j++) {
          for (let i = ci(b[0]), i1 = ci(b[2]); i <= i1; i++) {
            const list = cells[j * nx + i];
            if (!list) continue;
            for (const o of list) if (b[0] < o[2] && b[2] > o[0] && b[1] < o[3] && b[3] > o[1]) return true;
          }
        }
        return false;
      },
    };
  }

  /** A label along the longest piece of a path, kept upright. */
  function lineLabel(f, text, pri, w) {
    let best = 2, bestL = -1;
    for (let i = 2; i < f.length; i += 2) {
      const l = Math.hypot(f[i] - f[i - 2], f[i + 1] - f[i - 1]);
      if (l > bestL) { bestL = l; best = i; }
    }
    let a = Math.atan2(f[best + 1] - f[best - 1], f[best] - f[best - 2]);
    if (a > Math.PI / 2) a -= Math.PI;
    if (a < -Math.PI / 2) a += Math.PI;
    return { kind: 'line', x: (f[best] + f[best - 2]) / 2, y: (f[best + 1] + f[best - 1]) / 2, a, room: bestL, off: w / 2 + 7, text, pri };
  }

  /** A district's name: in the middle of its bounds when that is inside it, else of its biggest ring. */
  function areaLabel(rings, x0, y0, x1, y1, text, c, pri, th) {
    let x = (x0 + x1) / 2, y = (y0 + y1) / 2;
    if (!inRings(x, y, rings)) {
      let big = rings[0], bigA = -1;
      for (const f of rings) {
        let a = 0;
        for (let i = 0, j = f.length - 2; i < f.length; j = i, i += 2) a += (f[j] + f[i]) * (f[j + 1] - f[i + 1]);
        if (Math.abs(a) > bigA) { bigA = Math.abs(a); big = f; }
      }
      let sx = 0, sy = 0;
      for (let i = 0; i < big.length; i += 2) { sx += big[i]; sy += big[i + 1]; }
      x = sx / (big.length / 2);
      y = sy / (big.length / 2);
    }
    return { kind: 'area', x, y, text, color: mix(c, th.ink, 0.45), pri };
  }

  /** How far apart, in CSS px, two labels with the same text must be for both to show. */
  const REPEAT_PX = 160;

  /**
   * Draws labels on top without overlaps, higher priority first: a point's beside it (right, left,
   * below, above, then the corners, the first that is free), an area's in italics at its middle, a
   * path's along it. A label goes only where it is wholly on screen and covers no other label and
   * none of the symbols in `glyphs` (a `boxGrid`).
   */
  function placeLabels(ctx, labels, W, H, th, glyphs) {
    labels.sort((a, b) => b.pri - a.pri);
    const boxes = boxGrid(W, H, 64);
    const free = b => b[0] >= 0 && b[2] <= W && b[1] >= 0 && b[3] <= H && !boxes.hits(b) && !(glyphs && glyphs.hits(b));
    ctx.lineJoin = 'round';
    ctx.textBaseline = 'middle';
    let n = 0;
    // A name already on the map close by is not said again (a yard of "Creative motor"s, a row of
    // trains all called "Retired"): the first one placed stands for them.
    const said = new Map();
    const near = (L, text) => (said.get(text) || []).some(([x, y]) => Math.hypot(x - L.x, y - L.y) < REPEAT_PX);
    for (const L of labels) {
      if (n > 600) break;
      let text = String(L.text);
      if (text.length > 48) text = text.slice(0, 47) + '…';
      if (L.kind !== 'line' && near(L, text)) continue;
      ctx.font = L.kind === 'area' ? `italic 600 12.5px ${FONT}` : L.kind === 'line' ? `500 11px ${FONT}` : `500 12px ${FONT}`;
      const tw = ctx.measureText(text).width, th2 = 14;
      let at = null;
      if (L.kind === 'pt') {
        const g = L.r + 4, d = g * 0.75;
        const cands = [
          [L.x + g, L.y, 'left'], [L.x - g, L.y, 'right'], [L.x, L.y + g + th2 / 2, 'center'], [L.x, L.y - g - th2 / 2, 'center'],
          [L.x + d, L.y - d - th2 / 2, 'left'], [L.x + d, L.y + d + th2 / 2, 'left'],
          [L.x - d, L.y - d - th2 / 2, 'right'], [L.x - d, L.y + d + th2 / 2, 'right'],
        ];
        // `prefer`: the side tried first (0 right, 1 left, 2 below, 3 above).
        if (L.prefer > 0) cands.unshift(cands.splice(L.prefer, 1)[0]);
        for (const [x, y, align] of cands) {
          const x0 = align === 'left' ? x : align === 'right' ? x - tw : x - tw / 2;
          const b = [x0 - 2, y - th2 / 2, x0 + tw + 2, y + th2 / 2];
          if (free(b)) { at = { x, y, align, b }; break; }
        }
      } else if (L.kind === 'line') {
        if (tw > L.room * 0.9) continue;
        const c = Math.cos(L.a), s = Math.sin(L.a);
        const x = L.x - s * L.off, y = L.y + c * L.off;
        const hw = (Math.abs(c) * tw + Math.abs(s) * th2) / 2, hh = (Math.abs(s) * tw + Math.abs(c) * th2) / 2;
        const b = [x - hw, y - hh, x + hw, y + hh];
        if (free(b)) at = { x, y, align: 'center', b, a: L.a };
      } else {
        // An area's name (or a placed name) may move up or down a line or two to let a symbol or a
        // point's label be.
        for (const dy of [0, -18, 18, -34, 34]) {
          const y = L.y + dy;
          const b = [L.x - tw / 2 - 3, y - th2 / 2 - 1, L.x + tw / 2 + 3, y + th2 / 2 + 1];
          if (free(b)) { at = { x: L.x, y, align: 'center', b }; break; }
        }
      }
      if (!at) continue;
      boxes.add(at.b);
      if (!said.has(text)) said.set(text, []);
      said.get(text).push([L.x, L.y]);
      n++;
      ctx.save();
      ctx.translate(at.x, at.y);
      if (at.a) ctx.rotate(at.a);
      ctx.textAlign = at.align;
      ctx.lineWidth = 3.5;
      ctx.strokeStyle = th.halo;
      ctx.strokeText(text, 0, 0);
      ctx.fillStyle = L.color || th.ink;
      ctx.fillText(text, 0, 0);
      ctx.restore();
    }
  }

  // ------------------------------------------------------------------------------------------------
  // Swatches
  // ------------------------------------------------------------------------------------------------

  /**
   * Draws the swatch of layer `desc` (`index` null) or of its legend entry `index` into a `w` x `h`
   * CSS px box of `ctx`, with the same code that draws the map, in theme `theme`.
   */
  function drawSwatch(ctx, desc, index, w, h, theme) {
    const th = themeOf(theme);
    desc = desc || {};
    const style = desc.style || {};
    const e = index != null && desc.legend ? desc.legend[index] : null;
    const kind = (desc.kinds && desc.kinds[0]) || 'point';
    const color = resolve((e && e.color) || style.color || '#888888', th);
    const st = (e && e.st) || null;
    const glyph = e ? e.glyph : null;
    const line = e ? e.line : style.line;
    const cy = h / 2;
    if (glyph === 'train' || (!glyph && !line && kind === 'train')) {
      const L = Math.min(w - 4, 20);
      const x0 = (w + L) / 2;
      drawTrainShape(ctx, [[x0 - 1, cy, x0 - L * 0.45, cy], [x0 - L * 0.55 - 1, cy, x0 - L + 1, cy]], color, th, st, 0, Math.min(3.5, h / 3), 1);
      return;
    }
    if (kind === 'cluster') {
      // A count bubble: paper inside a ring, a number in it.
      const r = Math.min(h / 2, 7.5), x = w / 2;
      ctx.beginPath();
      ctx.arc(x, cy, r, 0, TAU);
      ctx.fillStyle = th.paper;
      ctx.fill();
      ctx.beginPath();
      ctx.arc(x, cy, r - 1, 0, TAU);
      ctx.lineWidth = 2;
      ctx.strokeStyle = color;
      ctx.stroke();
      ctx.font = `700 ${Math.round(r * 1.05)}px ${FONT}`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillStyle = th.ink;
      ctx.fillText(String((e && e.n) || style.n || 12), x, cy + 0.5);
      return;
    }
    if (line || (!glyph && (kind === 'path' || kind === 'edge' || kind === 'leg'))) {
      const f = line === 'rail' || line === 'chain' ? [2, cy + 2.5, w / 2, cy - 2.5, w - 2, cy + 1.5] : [2, cy, w - 2, cy];
      const lw = line === 'flow' ? 3 : Math.min(style.width || 2, 3);
      strokeLine(ctx, f, { line, color, w: lw, bpp: line === 'belt' ? 0.5 : 4, theme: th, arrow: line === 'belt' || line === 'flow' });
      return;
    }
    if (!glyph && kind === 'area') {
      ctx.beginPath();
      ctx.rect(3, 2.5, w - 6, h - 5);
      const ol = outlineOf(st, color, th);
      ctx.fillStyle = rgba(ol.color, 0.1);
      ctx.fill();
      if (ol.dash) ctx.setLineDash(ol.dash);
      ctx.lineWidth = ol.w;
      ctx.strokeStyle = ol.color;
      ctx.stroke();
      ctx.setLineDash([]);
      return;
    }
    if (!glyph && kind === 'label') {
      ctx.font = `600 11px ${FONT}`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.lineJoin = 'round';
      ctx.lineWidth = 3;
      ctx.strokeStyle = th.halo;
      ctx.strokeText('Aa', w / 2, cy + 0.5);
      ctx.fillStyle = th.ink;
      ctx.fillText('Aa', w / 2, cy + 0.5);
      return;
    }
    if (!glyph && kind === 'cell') {
      const n = 4, s = Math.min((w - 4) / n, h - 4);
      for (let i = 0; i < n; i++) {
        const [r, g, b] = rampColor('heat', (i + 0.5) / n);
        ctx.fillStyle = `rgb(${r}, ${g}, ${b})`;
        ctx.fillRect(2 + i * s, (h - s) / 2, s - 0.5, s);
      }
      return;
    }
    const g = glyph || style.glyph || 'circle';
    const r = Math.min(h / 2 - (st === 'bad' || st === 'warn' ? 3.5 : 1.5), g === 'station' ? 4.2 : 5);
    drawGlyph(ctx, g, w / 2, cy, Math.max(2.5, r), color, th, st, 0);
  }

  Object.assign(MV, {
    legAt, parseColor, rgba, lighten, mix, colorOf, glyphOf, lineOf, legendOf, RAMPS, rampColor, compact,
    distToSegment, distToFlat, inRings, offsetFlat, simplifyFlat, walk, flatLength, pick, glyphPath, drawGlyph,
    drawMarks, drawSwatch, strokeLine, xExtent, themeOf, resolve, railHalf, flowOffset, clearOf, boxGrid,
    THEMES: { light: LIGHT, dark: DARK },
  });
  if (typeof module === 'object' && module.exports) module.exports = MV;
})();
