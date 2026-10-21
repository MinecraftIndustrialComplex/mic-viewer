// SPDX-License-Identifier: MPL-2.0
// Draws the marks of `/api/marks` (docs/api.md §4) on the map's canvas and hit-tests them: point
// glyphs, legs placed by game time over a faint track, paths, edges, areas, cell grids through a
// colour ramp, cluster bubbles and labels. Colours come from the mark (`c`), else the layer
// legend (`ci`), else the layer style. On a wrapping frame every mark is drawn at each of its copies
// in view. Nothing here knows a layer: everything comes from the descriptor (VIEWER.md §4.3).
// A classic script: adds to `globalThis.MicViewer`.
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const FONT = 'system-ui, -apple-system, "Segoe UI", Roboto, "DejaVu Sans", sans-serif';
  const OUTLINE = 'rgba(8, 10, 14, 0.82)';
  const SLOP = 4;   // CSS px of grace around a hit target

  // ------------------------------------------------------------------------------------------------
  // Pure helpers
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

  /** `[r, g, b]` of `#rgb`, `#rrggbb` or `#rrggbbaa`; grey for anything else. */
  function parseColor(c) {
    const s = String(c || '').trim();
    let m = /^#([0-9a-f]{3})$/i.exec(s);
    if (m) return [...m[1]].map(h => parseInt(h + h, 16));
    m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(s);
    if (m) return [0, 2, 4].map(i => parseInt(m[1].slice(i, i + 2), 16));
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

  /** The legend entry a mark's `ci` names, if any. */
  function legendOf(mark, desc) {
    const lg = desc && desc.legend;
    return mark.ci != null && lg && lg[mark.ci] ? lg[mark.ci] : null;
  }
  /** A mark's colour: its own `c`, else its legend entry's, else the layer style's. */
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

  /** Distance from (px, py) to the segment (ax, ay)-(bx, by). */
  function distToSegment(px, py, ax, ay, bx, by) {
    const dx = bx - ax, dy = by - ay;
    const l2 = dx * dx + dy * dy;
    let f = l2 > 0 ? ((px - ax) * dx + (py - ay) * dy) / l2 : 0;
    f = clamp(f, 0, 1);
    return Math.hypot(px - (ax + f * dx), py - (ay + f * dy));
  }

  /**
   * The target under (px, py) of those a draw recorded (screen CSS px), or null. Glyphs first (the
   * one drawn last, so on top), then lines (the nearest), then areas (the smallest that contains
   * it), then cells (their value).
   */
  function pick(targets, px, py) {
    for (let i = targets.length - 1; i >= 0; i--) {
      const t = targets[i];
      if (t.kind === 'pt' && Math.hypot(px - t.x, py - t.y) <= t.r + SLOP) return t;
    }
    let best = null, bestD = Infinity;
    for (const t of targets) {
      let d = Infinity;
      if (t.kind === 'seg') d = distToSegment(px, py, t.ax, t.ay, t.bx, t.by);
      else if (t.kind === 'poly') {
        for (let i = 0; i + 3 < t.pts.length; i += 2) d = Math.min(d, distToSegment(px, py, t.pts[i], t.pts[i + 1], t.pts[i + 2], t.pts[i + 3]));
      } else continue;
      if (d <= t.w / 2 + SLOP && d < bestD) { best = t; bestD = d; }
    }
    if (best) return best;
    let bestA = Infinity;
    for (const t of targets) {
      let a = Infinity;
      if (t.kind === 'disc' && Math.hypot(px - t.x, py - t.y) <= t.r) a = t.r * t.r * Math.PI;
      else if (t.kind === 'box' && px >= t.x0 && px <= t.x1 && py >= t.y0 && py <= t.y1) a = (t.x1 - t.x0) * (t.y1 - t.y0);
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
  // Drawing
  // ------------------------------------------------------------------------------------------------

  /** Builds the path of glyph `g` of radius `r` at (x, y). */
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
      default:
        ctx.arc(x, y, r, 0, Math.PI * 2);
    }
  }

  /** Draws glyph `g` filled with `color` and outlined; `ring` glyphs are a stroked circle. */
  function drawGlyph(ctx, g, x, y, r, color) {
    if (g === 'ring') {
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.lineWidth = Math.max(2, r * 0.5) + 2;
      ctx.strokeStyle = OUTLINE;
      ctx.stroke();
      ctx.lineWidth = Math.max(2, r * 0.5);
      ctx.strokeStyle = color;
      ctx.stroke();
      return;
    }
    glyphPath(ctx, g, x, y, r);
    ctx.fillStyle = color;
    ctx.fill();
    ctx.lineWidth = 1.25;
    ctx.strokeStyle = OUTLINE;
    ctx.stroke();
  }

  /** A mark's east-west block extent `[x0, x1]` (for its copies across a wrap). */
  function xExtent(m) {
    switch (m.k) {
      case 'leg': {
        let a = Infinity, b = -Infinity;
        for (const q of m.p || []) { a = Math.min(a, q[0], q[2]); b = Math.max(b, q[0], q[2]); }
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

  /**
   * Draws `list` (`[{ desc, data }]`, `data` a layer of `/api/marks`, bottom first) on `ctx` (CSS px)
   * for `map` at game time `opts.t`. `opts.selected` / `opts.hovered`: `{ layer, id }` drawn with a
   * halo. Returns `{ targets, drawn, tracks }`: what `pick` hit-tests, the marks drawn, and the
   * screen segments of leg tracks (for checks).
   */
  function drawMarks(ctx, map, list, opts) {
    opts = opts || {};
    const v = map.view;
    const bpp = v.bpp, W = v.w, H = v.h;
    const left = v.cx - (W / 2) * bpp, top = v.cz - (H / 2) * bpp;
    const t = opts.t || 0;
    const sel = opts.selected, hov = opts.hovered;
    const targets = [], tracks = [], labels = [];
    let drawn = 0;
    const X = x => (x - left) / bpp, Y = z => (z - top) / bpp;
    const margin = 64;
    const visible = (x0, y0, x1, y1) => x1 >= -margin && x0 <= W + margin && y1 >= -margin && y0 <= H + margin;
    const halos = [];

    let order = 0;
    for (const { desc, data } of list) {
      if (!data || !data.marks) continue;
      const style = desc.style || { color: '#ccc', glyph: 'circle', size: 6, width: 2 };
      const cards = desc.has_cards !== false;
      const lid = desc.id;
      const isSel = m => sel && sel.layer === lid && m.id != null && String(m.id) === String(sel.id);
      const isHov = m => hov && hov.layer === lid && m.id != null && String(m.id) === String(hov.id);
      for (const m of data.marks) {
        const [ex0, ex1] = xExtent(m);
        const offs = map.copies((ex0 + ex1) / 2, (ex1 - ex0) / 2 + margin * bpp);
        for (const off of offs) {
          const o = off / bpp;
          order++;
          switch (m.k) {
            case 'point': {
              const x = X(m.x) + o, y = Y(m.z);
              if (!visible(x, y, x, y)) break;
              const r = Math.max(2.5, ((style.size || 6) * (m.s || 1)) / 2);
              const c = colorOf(m, desc);
              drawGlyph(ctx, glyphOf(m, desc), x, y, r, c);
              if (m.ring) {
                ctx.beginPath();
                ctx.arc(x, y, r + 3.5, 0, Math.PI * 2);
                ctx.lineWidth = 1.5;
                ctx.strokeStyle = 'rgba(255, 255, 255, 0.9)';
                ctx.stroke();
              }
              if (isSel(m) || isHov(m)) halos.push([x, y, r, isSel(m)]);
              if (m.id != null && cards) targets.push({ kind: 'pt', layer: lid, id: String(m.id), label: m.l, x, y, r, order });
              drawn++;
              break;
            }
            case 'leg': {
              const c = colorOf(m, desc);
              ctx.beginPath();
              for (const q of m.p || []) {
                const ax = X(q[0]) + o, ay = Y(q[1]), bx = X(q[2]) + o, by = Y(q[3]);
                ctx.moveTo(ax, ay);
                ctx.lineTo(bx, by);
                if (tracks.length < 5000) tracks.push([lid, m.id, ax, ay, bx, by]);
              }
              ctx.lineWidth = 1.5;
              ctx.strokeStyle = rgba(c, 0.32);
              ctx.stroke();
              const [px, pz] = legAt(m, t);
              const x = X(px) + o, y = Y(pz);
              if (!visible(x, y, x, y)) break;
              const r = Math.max(2.5, (style.size || 6) / 2 + 0.5);
              drawGlyph(ctx, glyphOf(m, desc), x, y, r, c);
              if (isSel(m) || isHov(m)) halos.push([x, y, r, isSel(m)]);
              if (m.id != null && cards) targets.push({ kind: 'pt', layer: lid, id: String(m.id), label: m.l, x, y, r, order });
              drawn++;
              break;
            }
            case 'path': {
              const pts = m.p || [];
              if (pts.length < 2) break;
              const c = colorOf(m, desc), w = m.w || style.width || 2;
              const flat = [];
              ctx.beginPath();
              pts.forEach((q, i) => {
                const x = X(q[0]) + o, y = Y(q[1]);
                flat.push(x, y);
                if (i) ctx.lineTo(x, y);
                else ctx.moveTo(x, y);
              });
              ctx.lineJoin = 'round';
              ctx.lineCap = 'round';
              ctx.lineWidth = w;
              ctx.strokeStyle = rgba(c, isSel(m) ? 1 : 0.8);
              ctx.stroke();
              if (m.id != null && cards) targets.push({ kind: 'poly', layer: lid, id: String(m.id), label: m.l, pts: flat, w, order });
              drawn++;
              break;
            }
            case 'edge': {
              const ax = X(m.a[0]) + o, ay = Y(m.a[1]), bx = X(m.b[0]) + o, by = Y(m.b[1]);
              if (!visible(Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by))) break;
              const c = colorOf(m, desc), w = m.w || style.width || 2;
              ctx.lineCap = 'round';
              ctx.beginPath();
              ctx.moveTo(ax, ay);
              ctx.lineTo(bx, by);
              ctx.lineWidth = w + 2;
              ctx.strokeStyle = 'rgba(0, 0, 0, 0.28)';
              ctx.stroke();
              ctx.lineWidth = w;
              ctx.strokeStyle = isSel(m) || isHov(m) ? lighten(c, 0.45) : rgba(c, 0.9);
              ctx.stroke();
              if (m.id != null && cards) targets.push({ kind: 'seg', layer: lid, id: String(m.id), label: m.l, ax, ay, bx, by, w, order });
              drawn++;
              break;
            }
            case 'area': {
              const c = colorOf(m, desc);
              const hi = isSel(m) || isHov(m);
              if (m.box) {
                let x0 = X(m.box[0]) + o, y0 = Y(m.box[1]), x1 = X(m.box[2]) + o, y1 = Y(m.box[3]);
                if (!visible(x0, y0, x1, y1)) break;
                if (x1 - x0 < 3) { const mx = (x0 + x1) / 2; x0 = mx - 1.5; x1 = mx + 1.5; }
                if (y1 - y0 < 3) { const my = (y0 + y1) / 2; y0 = my - 1.5; y1 = my + 1.5; }
                ctx.fillStyle = rgba(c, hi ? 0.42 : 0.24);
                ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
                ctx.lineWidth = hi ? 2 : 1.25;
                ctx.strokeStyle = hi ? lighten(c, 0.5) : rgba(c, 0.9);
                ctx.strokeRect(x0, y0, x1 - x0, y1 - y0);
                if (m.id != null && cards) targets.push({ kind: 'box', layer: lid, id: String(m.id), label: m.l, x0, y0, x1, y1, order });
                if (m.l && x1 - x0 > 80) labels.push({ x: (x0 + x1) / 2, y: (y0 + y1) / 2, text: m.l, area: c, pri: desc.priority || 0 });
              } else {
                const x = X(m.x) + o, y = Y(m.z), r = Math.max(1.5, (m.r || 0) / bpp);
                if (!visible(x - r, y - r, x + r, y + r)) break;
                ctx.beginPath();
                ctx.arc(x, y, r, 0, Math.PI * 2);
                ctx.fillStyle = rgba(c, hi ? 0.32 : 0.16);
                ctx.fill();
                ctx.lineWidth = hi ? 2 : 1.25;
                ctx.strokeStyle = hi ? lighten(c, 0.5) : rgba(c, 0.75);
                ctx.stroke();
                if (m.id != null && cards) targets.push({ kind: 'disc', layer: lid, id: String(m.id), label: m.l, x, y, r, order });
                if (m.l && r > 40) labels.push({ x, y, text: m.l, area: c, pri: desc.priority || 0 });
              }
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
              drawn++;
              break;
            }
            case 'cluster': {
              const x = X(m.x) + o, y = Y(m.z);
              if (!visible(x, y, x, y)) break;
              const r = clamp(9 + 3.4 * Math.log10(Math.max(1, m.n)), 10, 22);
              ctx.beginPath();
              ctx.arc(x, y, r, 0, Math.PI * 2);
              ctx.fillStyle = 'rgba(14, 18, 26, 0.86)';
              ctx.fill();
              const by = m.by && Object.keys(m.by).length ? m.by : null;
              ctx.lineWidth = 3.5;
              if (by) {
                const sum = Object.values(by).reduce((a, b) => a + b, 0) || 1;
                let a0 = -Math.PI / 2;
                for (const [k, n] of Object.entries(by)) {
                  const a1 = a0 + (n / sum) * Math.PI * 2;
                  const e = desc.legend && desc.legend[Number(k)];
                  ctx.beginPath();
                  ctx.arc(x, y, r - 1.75, a0, a1);
                  ctx.strokeStyle = (e && e.color) || style.color;
                  ctx.stroke();
                  a0 = a1;
                }
              } else {
                ctx.beginPath();
                ctx.arc(x, y, r - 1.75, 0, Math.PI * 2);
                ctx.strokeStyle = style.color;
                ctx.stroke();
              }
              ctx.font = `600 ${r > 14 ? 12 : 11}px ${FONT}`;
              ctx.textAlign = 'center';
              ctx.textBaseline = 'middle';
              ctx.fillStyle = '#f4f6fa';
              ctx.fillText(compact(m.n), x, y + 0.5);
              targets.push({ kind: 'pt', layer: lid, id: null, cluster: true, n: m.n, wx: m.x + off, wz: m.z, label: null, x, y, r, order });
              drawn++;
              break;
            }
            case 'label': {
              const x = X(m.x) + o, y = Y(m.z);
              if (!visible(x, y, x, y)) break;
              labels.push({ x, y: y - 10, text: m.text, pri: (desc.priority || 0) + 1000 });
              drawn++;
              break;
            }
            default:
              break;
          }
        }
      }
    }

    // Halos of the selected and hovered things, over every mark.
    for (const [x, y, r, s] of halos) {
      ctx.beginPath();
      ctx.arc(x, y, r + 5, 0, Math.PI * 2);
      ctx.lineWidth = 4;
      ctx.strokeStyle = 'rgba(0, 0, 0, 0.55)';
      ctx.stroke();
      ctx.lineWidth = 2;
      ctx.strokeStyle = s ? '#ffffff' : 'rgba(255, 255, 255, 0.7)';
      ctx.stroke();
    }

    // Labels last, on top, without overlaps (higher priority first).
    labels.sort((a, b) => b.pri - a.pri);
    const boxes = [];
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.lineJoin = 'round';
    for (const L of labels) {
      ctx.font = L.area ? `700 11px ${FONT}` : `500 12px ${FONT}`;
      const text = L.area ? String(L.text).toUpperCase() : String(L.text);
      if ('letterSpacing' in ctx) ctx.letterSpacing = L.area ? '1.5px' : '0px';
      const w = ctx.measureText(text).width, h = 14;
      const b = [L.x - w / 2 - 3, L.y - h / 2 - 1, L.x + w / 2 + 3, L.y + h / 2 + 1];
      if (b[2] < 0 || b[0] > W || b[3] < 0 || b[1] > H) continue;
      if (boxes.some(o => b[0] < o[2] && b[2] > o[0] && b[1] < o[3] && b[3] > o[1])) continue;
      boxes.push(b);
      ctx.lineWidth = 3.5;
      ctx.strokeStyle = 'rgba(8, 10, 14, 0.82)';
      ctx.strokeText(text, L.x, L.y);
      ctx.fillStyle = L.area ? lighten(L.area, 0.35) : '#f4f0e4';
      ctx.fillText(text, L.x, L.y);
    }
    if ('letterSpacing' in ctx) ctx.letterSpacing = '0px';
    return { targets, drawn, tracks };
  }

  Object.assign(MV, {
    legAt, parseColor, rgba, lighten, colorOf, glyphOf, legendOf, RAMPS, rampColor, compact,
    distToSegment, pick, glyphPath, drawGlyph, drawMarks, xExtent,
  });
  if (typeof module === 'object' && module.exports) module.exports = MV;
})();
