// SPDX-License-Identifier: MPL-2.0
// The viewer page (VIEWER.md §10): loads `/api/world` and `/api/layers`, builds the layer panel, asks
// `/api/marks` for the view, follows `/api/stream`, opens cards, drives the timeline, and keeps the
// view in the URL hash. Only GETs leave the page. A classic script: adds to `globalThis.MicViewer`
// (the pure helpers also to `module.exports` under node, where nothing else runs).
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});
  const TPD = 24000;
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

  // ------------------------------------------------------------------------------------------------
  // Pure helpers
  // ------------------------------------------------------------------------------------------------

  const numOrNull = v => {
    if (v == null || String(v).trim() === '') return null;
    const n = Number(v);
    return isFinite(n) ? n : null;
  };

  /**
   * The view a URL hash asks for: `#frame=..&x=..&z=..&bpp=..&layers=a,b&t=..&card=layer/id`.
   * Missing or malformed parts are null.
   */
  function parseHash(hash) {
    const out = { frame: null, x: null, z: null, bpp: null, layers: null, t: null, card: null };
    const s = String(hash || '').replace(/^#/, '');
    for (const part of s.split('&')) {
      const i = part.indexOf('=');
      if (i <= 0) continue;
      let k, v;
      try {
        k = decodeURIComponent(part.slice(0, i));
        v = decodeURIComponent(part.slice(i + 1).replace(/\+/g, ' '));
      } catch (e) {
        continue;
      }
      if (k === 'frame') out.frame = v || null;
      else if (k === 'x' || k === 'z') out[k] = numOrNull(v);
      else if (k === 'bpp') {
        const b = numOrNull(v);
        out.bpp = b != null && b > 0 ? b : null;
      } else if (k === 'layers') out.layers = v.split(',').map(x => x.trim()).filter(Boolean);
      else if (k === 't') {
        const t = numOrNull(v);
        out.t = t != null ? Math.round(t) : null;
      } else if (k === 'card') {
        const j = v.indexOf('/');
        if (j > 0 && j < v.length - 1) out.card = { layer: v.slice(0, j), id: v.slice(j + 1) };
      }
    }
    return out;
  }

  /** The hash for a view (`parseHash`'s inverse; numbers rounded to what a link needs). */
  function buildHash(s) {
    const enc = v => encodeURIComponent(v).replace(/%3A/gi, ':');
    const parts = [];
    if (s.frame) parts.push('frame=' + enc(s.frame));
    if (s.x != null && isFinite(s.x)) parts.push('x=' + Math.round(s.x));
    if (s.z != null && isFinite(s.z)) parts.push('z=' + Math.round(s.z));
    if (s.bpp != null && s.bpp > 0) parts.push('bpp=' + Number(s.bpp.toPrecision(4)));
    if (s.layers) parts.push('layers=' + s.layers.map(enc).join(','));
    if (s.t != null) parts.push('t=' + Math.round(s.t));
    if (s.card) parts.push('card=' + enc(s.card.layer) + '/' + enc(s.card.id));
    return '#' + parts.join('&');
  }

  /** Whether a layer is asked for at `bpp` blocks per CSS pixel. */
  function inZoom(d, bpp) {
    const lo = d.min_bpp != null ? d.min_bpp : 0, hi = d.max_bpp != null ? d.max_bpp : Infinity;
    return bpp >= lo && bpp <= hi;
  }

  /** Which way a layer outside its zoom range lies: 'in' (zoom in to see it), 'out', or null. */
  function zoomHint(d, bpp) {
    if (d.max_bpp != null && bpp > d.max_bpp) return 'in';
    if (d.min_bpp != null && bpp < d.min_bpp) return 'out';
    return null;
  }

  /** The block box `[x0, z0, x1, z1]` of a view, grown by `pad` of its size on every side. */
  function viewBox(v, pad) {
    const g = 1 + 2 * (pad || 0);
    const hw = (v.w / 2) * v.bpp * g, hh = (v.h / 2) * v.bpp * g;
    return [v.cx - hw, v.cz - hh, v.cx + hw, v.cz + hh];
  }

  const boxInside = (a, b) => !!b && a[0] >= b[0] && a[1] >= b[1] && a[2] <= b[2] && a[3] <= b[3];

  /** The `/api/marks` URL for a frame, a box, a zoom, layers and a time (null: now). */
  function marksURL(frame, box, bpp, layers, t) {
    const enc = encodeURIComponent;
    let q = `api/marks?frame=${enc(frame)}&x0=${Math.floor(box[0])}&z0=${Math.floor(box[1])}` +
      `&x1=${Math.ceil(box[2])}&z1=${Math.ceil(box[3])}&bpp=${Number(bpp.toPrecision(6))}` +
      `&layers=${layers.map(enc).join(',')}`;
    if (t != null) q += `&t=${Math.round(t)}`;
    return q;
  }

  /** The layers held whose version in a stream `frame` message (for `frame`) has moved on. */
  function changedLayers(msg, frame, held) {
    const fv = msg && msg.frames && msg.frames[frame];
    if (!fv) return [];
    return Object.keys(held || {}).filter(id => fv[id] != null && held[id] && fv[id] !== held[id].version);
  }

  /** The stream `events` rows for the frame shown: a row without a frame is the first frame's. */
  function rowsForFrame(rows, frame, firstFrame) {
    return (rows || []).filter(r => (r.frame || firstFrame) === frame);
  }

  /** Which base maps start shown, by id: the viewer's saved choice, else the layer's `default_on`,
   *  else the first base map and every overlay. */
  function tilesShown(tiles, saved) {
    const on = {};
    let bases = 0;
    for (const d of tiles || []) {
      const s = saved && saved[d.id];
      on[d.id] = s != null ? !!s : d.default_on != null ? !!d.default_on : !!d.overlay || bases === 0;
      if (!d.overlay) bases++;
    }
    return on;
  }

  /** Whether a source is stale: nothing published for two cycles. */
  function isStale(fresh, sinceMs) {
    if (!fresh || !fresh.cycle_ms) return false;
    return (fresh.age_ms || 0) + Math.max(0, sinceMs) > 2 * fresh.cycle_ms;
  }

  /** The tick now: the last one known, moved on at `tps` while the world runs and is not stale. */
  function tickAt(time, sinceMs, stale) {
    if (!time) return 0;
    if (!time.running || stale || !(time.tps > 0)) return time.tick;
    return time.tick + Math.floor((Math.max(0, sinceMs) / 1000) * time.tps);
  }

  /** "just now", "12 s ago", "4 min ago", "3 h ago", "2 days ago". */
  function fmtAge(ms) {
    if (ms == null || !isFinite(ms)) return '';
    const s = Math.round(ms / 1000);
    if (s < 5) return 'just now';
    if (s < 60) return `${s} s ago`;
    const m = Math.round(s / 60);
    if (m < 60) return `${m} min ago`;
    const h = Math.round(m / 60);
    if (h < 48) return `${h} h ago`;
    return `${Math.round(h / 24)} days ago`;
  }

  // ------------------------------------------------------------------------------------------------
  // The page
  // ------------------------------------------------------------------------------------------------

  function boot() {
    const $ = id => document.getElementById(id);
    const html = document.documentElement;
    const mq = q => (typeof matchMedia === 'function' ? matchMedia(q) : { matches: false, addEventListener() {} });
    const phoneMQ = mq('(max-width: 759px)');
    const reduceMQ = mq('(prefers-reduced-motion: reduce)');
    const lightMQ = mq('(prefers-color-scheme: light)');
    const SVGNS = 'http://www.w3.org/2000/svg';
    const fmtInt = MV.fmtInt;
    const now = () => performance.now();
    let map = null, timeline = null;

    function el(tag, cls, text) {
      const e = document.createElement(tag);
      if (cls) e.className = cls;
      if (text != null) e.textContent = text;
      return e;
    }
    function svgEl(tag, attrs) {
      const e = document.createElementNS(SVGNS, tag);
      for (const k of Object.keys(attrs || {})) e.setAttribute(k, attrs[k]);
      return e;
    }
    function icon(name, cls) {
      const s = svgEl('svg', { class: 'icon' + (cls ? ' ' + cls : ''), 'aria-hidden': 'true' });
      const u = svgEl('use', { href: '#i-' + name });
      s.appendChild(u);
      return s;
    }

    /** Per-viewer settings; every access may throw (private windows, blocked storage). */
    const store = {
      get(k) {
        try {
          const v = globalThis.localStorage.getItem('micviewer.' + k);
          return v == null ? null : JSON.parse(v);
        } catch (e) {
          return null;
        }
      },
      set(k, v) {
        try {
          globalThis.localStorage.setItem('micviewer.' + k, JSON.stringify(v));
        } catch (e) { /* the page works without it */ }
      },
    };

    const A = {
      ready: false, world: null, groups: [], layers: [], byId: new Map(), tpd: TPD,
      frame: null, frameDesc: null, on: new Set(), tilesOn: new Set(), tileList: [],
      marks: { key: null, box: null, bpp: 0, t: null, layers: {} }, fullSeq: 0, marksTimer: 0, pending: 0,
      t: null, time: null, timeAt: 0, fresh: null, freshAt: 0,
      card: null, cardData: null, stack: [], cardSeq: 0, selected: null, hovered: null,
      events: [], eventLegs: [], eventHover: null, eventSel: null, flashes: [],
      // Stream messages taken: `frame` messages, and live event rows for the frame shown.
      heard: { frames: 0, rows: 0 },
      lastDraw: null, es: null, connected: false, helloSeen: false, hideTimer: 0, hashTimer: 0, ownHash: null,
      flight: 0, cursorW: null, sheet: null,
    };

    // ----------------------------------------------------------------------------------------------
    // Fetching (GET only)
    // ----------------------------------------------------------------------------------------------
    function getJSON(url) {
      A.pending++;
      return fetch(url, { credentials: 'same-origin', headers: { Accept: 'application/json' } })
        .then(r => {
          if (!r.ok) {
            const e = new Error(`${r.status} ${r.statusText || ''}`.trim());
            e.status = r.status;
            throw e;
          }
          return r.json();
        })
        .finally(() => { A.pending--; });
    }

    let toastTimer = 0;
    function toast(msg, ms) {
      const t = $('toast');
      t.textContent = msg;
      t.hidden = false;
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => { t.hidden = true; }, ms || 5000);
    }

    // ----------------------------------------------------------------------------------------------
    // Theme
    // ----------------------------------------------------------------------------------------------
    const cssVar = name => getComputedStyle(html).getPropertyValue(name).trim();
    const effectiveTheme = () => html.dataset.theme || (lightMQ.matches ? 'light' : 'dark');

    function applyTheme() {
      const s = store.get('theme');
      if (s === 'dark' || s === 'light') html.dataset.theme = s;
      else delete html.dataset.theme;
      const dark = effectiveTheme() === 'dark';
      const b = $('btn-theme');
      b.setAttribute('aria-label', dark ? 'Switch to the light theme' : 'Switch to the dark theme');
      b.title = dark ? 'Light theme' : 'Dark theme';
      b.textContent = '';
      b.appendChild(icon(dark ? 'sun' : 'moon'));
      if (map) { map.setBackground(cssVar('--map-bg') || '#05070b'); placeOverlays(); }
      if (timeline) timeline.redraw();
    }
    $('btn-theme').addEventListener('click', () => {
      store.set('theme', effectiveTheme() === 'dark' ? 'light' : 'dark');
      applyTheme();
    });
    if (lightMQ.addEventListener) lightMQ.addEventListener('change', () => applyTheme());
    applyTheme();

    // ----------------------------------------------------------------------------------------------
    // The map
    // ----------------------------------------------------------------------------------------------
    const canvas = $('map');

    const stale = () => isStale(A.fresh, now() - A.freshAt);
    const nowTick = () => tickAt(A.time, now() - A.timeAt, stale());
    const drawT = () => (A.t != null ? A.t : nowTick());
    const sameRef = (a, b) => (!a && !b) || (a && b && a.layer === b.layer && String(a.id) === String(b.id));

    function drawList() {
      const bpp = map.view.bpp;
      return A.layers
        .filter(d => A.on.has(d.id) && A.marks.layers[d.id] && inZoom(d, bpp))
        .sort((a, b) => (a.priority || 0) - (b.priority || 0))
        .map(d => ({ desc: d, data: A.marks.layers[d.id] }));
    }

    function kindColor(kind) {
      const k = (A.world.event_kinds || []).find(e => e.id === kind);
      return k ? k.color : '#d19dff';
    }

    function drawEvents(ctx) {
      const v = map.view;
      const left = v.cx - (v.w / 2) * v.bpp, top = v.cz - (v.h / 2) * v.bpp;
      const X = x => (x - left) / v.bpp, Y = z => (z - top) / v.bpp;
      // Legs of the window (parties on the move), dashed.
      ctx.setLineDash([5, 4]);
      ctx.lineWidth = 1.6;
      for (const g of A.eventLegs) {
        ctx.strokeStyle = MV.rgba(kindColor(g.kind), 0.85);
        for (const q of g.p || []) {
          const mid = (q[0] + q[2]) / 2;
          for (const off of map.copies(mid, Math.abs(q[2] - q[0]) / 2 + 20 * v.bpp)) {
            ctx.beginPath();
            ctx.moveTo(X(q[0] + off), Y(q[1]));
            ctx.lineTo(X(q[2] + off), Y(q[3]));
            ctx.stroke();
          }
        }
      }
      ctx.setLineDash([]);
      for (const r of A.events) {
        const c = kindColor(r.kind);
        const rad = r.sev >= 2 ? 5.5 : 4.2;
        for (const off of map.copies(r.x, 16 * v.bpp)) {
          const x = X(r.x + off), y = Y(r.z);
          if (x < -20 || y < -20 || x > v.w + 20 || y > v.h + 20) continue;
          ctx.beginPath();
          ctx.arc(x, y, rad, 0, Math.PI * 2);
          ctx.fillStyle = MV.rgba(c, 0.35);
          ctx.fill();
          ctx.lineWidth = 2.5;
          ctx.strokeStyle = 'rgba(8, 10, 14, 0.75)';
          ctx.stroke();
          ctx.lineWidth = 1.5;
          ctx.strokeStyle = c;
          ctx.stroke();
        }
      }
      for (const r of [A.eventHover, A.eventSel]) {
        if (!r) continue;
        for (const off of map.copies(r.x, 30 * v.bpp)) {
          const x = X(r.x + off), y = Y(r.z);
          ctx.beginPath();
          ctx.arc(x, y, 12, 0, Math.PI * 2);
          ctx.lineWidth = 4;
          ctx.strokeStyle = 'rgba(8, 10, 14, 0.7)';
          ctx.stroke();
          ctx.lineWidth = 2;
          ctx.strokeStyle = '#ffffff';
          ctx.stroke();
          ctx.beginPath();
          for (const [a, b] of [[-20, -15], [15, 20]]) {
            ctx.moveTo(x + a, y); ctx.lineTo(x + b, y);
            ctx.moveTo(x, y + a); ctx.lineTo(x, y + b);
          }
          ctx.stroke();
        }
      }
      // Live events: a ring that grows and fades for two seconds.
      const t = now();
      A.flashes = A.flashes.filter(f => t - f.at < 2000);
      for (const f of A.flashes) {
        const u = (t - f.at) / 2000;
        for (const off of map.copies(f.x, 60 * v.bpp)) {
          ctx.beginPath();
          ctx.arc(X(f.x + off), Y(f.z), 6 + 34 * u, 0, Math.PI * 2);
          ctx.lineWidth = 2.5 * (1 - u) + 0.5;
          ctx.strokeStyle = MV.rgba(f.c, 1 - u);
          ctx.stroke();
        }
      }
    }

    function overlay(ctx) {
      if (!A.ready) return;
      A.lastDraw = MV.drawMarks(ctx, map, drawList(), { t: drawT(), selected: A.selected, hovered: A.hovered });
      drawEvents(ctx);
    }

    function onView() {
      if (!A.ready) return;
      updateReadout();
      updateZoomRows();
      scheduleMarks();
      scheduleHash();
      if (timeline) timeline.viewChanged();
    }

    function updateReadout() {
      const ro = $('readout');
      const v = map.view;
      const [x, z] = A.cursorW || [v.cx, v.cz];
      const bx = Math.floor(x), bz = Math.floor(z);
      ro.textContent = '';
      const part = (k, val) => {
        const s = el('span', 'ro-part');
        s.append(el('i', null, k), document.createTextNode(' ' + val));
        ro.appendChild(s);
      };
      part('x', fmtInt(bx));
      part('z', fmtInt(bz));
      const proj = map.projection;
      if (proj && typeof proj.latLon === 'function') {
        const ll = proj.latLon(x, z);
        if (ll) part('lat, lon', `${ll[0].toFixed(2)}, ${ll[1].toFixed(2)}`);
      } else part('chunk', `${fmtInt(Math.floor(bx / 16))}, ${fmtInt(Math.floor(bz / 16))}`);
      ro.classList.toggle('centre', !A.cursorW);
      ro.title = A.cursorW ? 'The block under the pointer' : 'The block at the centre of the map';
    }

    const tip = $('tip');
    function showTip(hit, px, py) {
      if (!hit || px == null) {
        tip.hidden = true;
        return;
      }
      const d = A.byId.get(hit.layer);
      tip.textContent = '';
      if (hit.cluster) {
        tip.append(el('b', null, `${fmtInt(hit.n)} ${d ? d.label.toLowerCase() : 'things'}`), el('span', 'muted', 'Click to zoom in'));
      } else if (hit.kind === 'cell') {
        tip.append(el('b', null, MV.fmtNum(hit.value)), el('span', 'muted', d ? d.label : hit.layer));
      } else {
        tip.append(el('b', null, hit.label || `${d ? d.label : hit.layer} ${hit.id}`), el('span', 'muted', d ? d.label : hit.layer));
      }
      tip.hidden = false;
      const v = map.view;
      const w = tip.offsetWidth, h = tip.offsetHeight;
      const x = px + 16 + w > v.w ? px - 12 - w : px + 16;
      const y = py + 18 + h > v.h ? py - 12 - h : py + 18;
      tip.style.transform = `translate(${Math.round(Math.max(4, x))}px, ${Math.round(Math.max(4, y))}px)`;
    }

    function onHover(x, z, px, py) {
      if (!A.ready) return;
      A.cursorW = x == null ? null : [x, z];
      A.cursorPx = px == null ? null : [px, py];
      updateReadout();
      const hit = px != null && A.lastDraw ? MV.pick(A.lastDraw.targets, px, py) : null;
      const hov = hit && hit.id != null ? { layer: hit.layer, id: hit.id } : null;
      if (!sameRef(hov, A.hovered)) {
        A.hovered = hov;
        map.invalidate();
      }
      canvas.classList.toggle('hit', !!hit && (hit.id != null || !!hit.cluster));
      showTip(hit, px, py);
    }

    function activate(hit) {
      if (!hit) return false;
      if (hit.cluster) {
        flyTo(hit.wx, hit.wz, map.view.bpp / 3);
        return true;
      }
      if (hit.id != null) {
        openCard(hit.layer, hit.id, {});
        return true;
      }
      return false;
    }

    function onClick(x, z, px, py) {
      if (!A.ready) return;
      const hit = A.lastDraw ? MV.pick(A.lastDraw.targets, px, py) : null;
      if (!activate(hit) && A.card) closeCard();
    }

    function onKey(ev) {
      if (ev.key === 'Enter' || ev.key === ' ') {
        // The mark under the pointer, else the one at the centre.
        const v = map.view;
        const p = A.cursorPx || [v.w / 2, v.h / 2];
        return activate(A.lastDraw ? MV.pick(A.lastDraw.targets, p[0], p[1]) : null);
      }
      return false;
    }

    function cancelFlight() {
      if (A.flight) cancelAnimationFrame(A.flight);
      A.flight = 0;
    }
    canvas.addEventListener('pointerdown', cancelFlight);
    canvas.addEventListener('wheel', cancelFlight, { passive: true });

    /** Moves the view to (x, z), at `bpp` (default: no further out than 4 blocks a pixel). */
    function flyTo(x, z, bpp) {
      cancelFlight();
      const v = map.view;
      const P = (map.projection && map.projection.period) || 0;
      const target = clamp(bpp || Math.min(v.bpp, 4), map.minBpp(), map.maxBpp());
      const dx = P > 0 ? MV.wrap(x - v.cx, P) : x - v.cx, dz = z - v.cz;
      if (reduceMQ.matches) {
        map.setView(v.cx + dx, z, target);
        return;
      }
      const screens = Math.hypot(dx, dz) / (Math.max(v.w, v.h) * Math.max(v.bpp, target));
      const far = Math.log(Math.max(1, screens));
      const dur = clamp(380 + 220 * Math.log2(1 + screens), 380, 1100);
      const b0 = Math.log(v.bpp), b1 = Math.log(target), cx0 = v.cx, cz0 = v.cz, t0 = now();
      const step = () => {
        const k = clamp((now() - t0) / dur, 0, 1);
        const u = k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
        map.setView(cx0 + dx * u, cz0 + dz * u, Math.exp(b0 + (b1 - b0) * u + far * 4 * u * (1 - u)));
        A.flight = k < 1 ? requestAnimationFrame(step) : 0;
      };
      A.flight = requestAnimationFrame(step);
    }

    // ----------------------------------------------------------------------------------------------
    // Marks
    // ----------------------------------------------------------------------------------------------
    const wantedLayers = bpp => A.layers.filter(d => A.on.has(d.id) && inZoom(d, bpp)).map(d => d.id);
    const marksKey = want => `${A.frame}|${A.t == null ? 'now' : A.t}|${want.join(',')}`;

    function scheduleMarks(ms) {
      clearTimeout(A.marksTimer);
      A.marksTimer = setTimeout(() => {
        A.marksTimer = 0;
        fetchMarks(null, false);
      }, ms == null ? 140 : ms);
    }

    /**
     * Asks `/api/marks` for the view: every wanted layer (`only` null), or just `only` (layers whose
     * version moved, merged into what is held). Skipped while the view stays inside the box held at
     * about the same zoom, unless `force`.
     */
    function fetchMarks(only, force) {
      const v = map.view;
      if (!A.frame || !v.w) return;
      const want = wantedLayers(v.bpp);
      const key = marksKey(want);
      const M = A.marks;
      if (!only && !force && M.key === key && boxInside(viewBox(v, 0), M.box) && v.bpp <= M.bpp * 1.1 && v.bpp >= M.bpp / 1.1) return;
      if (!want.length) {
        A.fullSeq++;
        A.marks = { key, box: viewBox(v, 0.25), bpp: v.bpp, t: A.t, layers: {} };
        updateCounts();
        map.invalidate();
        return;
      }
      const ask = only ? only.filter(id => want.includes(id)) : want;
      if (!ask.length) return;
      const box = only ? M.box : viewBox(v, 0.25);
      const bpp = only ? M.bpp : v.bpp;
      const seq = only ? A.fullSeq : ++A.fullSeq;
      const t = A.t;
      getJSON(marksURL(A.frame, box, bpp, ask, t)).then(ans => {
        if (seq !== A.fullSeq) return;            // a newer full answer is coming
        const got = (ans && ans.layers) || {};
        if (only) {
          if (A.marks.key !== key) return;
          Object.assign(A.marks.layers, got);
        } else {
          A.marks = { key, box, bpp, t, layers: got };
        }
        updateCounts();
        map.invalidate();
      }, e => {
        if (seq !== A.fullSeq) return;
        if (e.status === 404 && t != null) toast('There is no record of this frame at that time.');
        else if (e.status !== 404) toast(`The marks did not load (${e.message}).`);
        A.marks = { key, box, bpp, t, layers: only ? A.marks.layers : {} };
        updateCounts();
        map.invalidate();
      });
    }

    // ----------------------------------------------------------------------------------------------
    // The layer panel
    // ----------------------------------------------------------------------------------------------
    function starPath(c, R, r) {
      let d = '';
      for (let i = 0; i < 10; i++) {
        const a = -Math.PI / 2 + (i * Math.PI) / 5, rr = i % 2 ? r : R;
        d += (i ? 'L' : 'M') + (c + rr * Math.cos(a)).toFixed(2) + ' ' + (c + rr * Math.sin(a)).toFixed(2);
      }
      return d + 'Z';
    }

    /** A 16 x 16 swatch: the glyph of a point or leg layer, a line, an area, a grid or text. */
    function swatch(kind, glyph, color) {
      const s = svgEl('svg', { viewBox: '0 0 16 16', class: 'glyph', 'aria-hidden': 'true' });
      const add = (tag, a) => s.appendChild(svgEl(tag, a));
      const casing = { stroke: 'rgba(0,0,0,.45)', 'stroke-width': 1 };
      if (kind === 'edge') add('path', { d: 'M2 13 14 3', stroke: color, 'stroke-width': 2.6, 'stroke-linecap': 'round', fill: 'none' });
      else if (kind === 'path') add('path', { d: 'M1.5 12 5.5 6.5 9.5 10 14.5 3.5', stroke: color, 'stroke-width': 2, 'stroke-linejoin': 'round', 'stroke-linecap': 'round', fill: 'none' });
      else if (kind === 'area') add('rect', { x: 2, y: 2, width: 12, height: 12, rx: 3, fill: color, 'fill-opacity': 0.35, stroke: color, 'stroke-width': 1.5 });
      else if (kind === 'cell') {
        const fs = [0.15, 0.45, 0.7, 0.95];
        fs.forEach((f, i) => add('rect', { x: 2 + (i % 2) * 6.5, y: 2 + Math.floor(i / 2) * 6.5, width: 5.5, height: 5.5, rx: 1, fill: MV.rampColor('heat', f) }));
      } else if (kind === 'label') {
        const t = svgEl('text', { x: 8, y: 12.5, 'text-anchor': 'middle', 'font-size': 11, 'font-weight': 700, fill: color });
        t.textContent = 'Aa';
        s.appendChild(t);
      } else {
        switch (glyph) {
          case 'square': add('rect', Object.assign({ x: 3, y: 3, width: 10, height: 10, rx: 1.5, fill: color }, casing)); break;
          case 'diamond': add('path', Object.assign({ d: 'M8 1.6 14.4 8 8 14.4 1.6 8Z', fill: color }, casing)); break;
          case 'triangle': add('path', Object.assign({ d: 'M8 2 14.6 13.6H1.4Z', fill: color }, casing)); break;
          case 'ring': add('circle', { cx: 8, cy: 8, r: 4.8, fill: 'none', stroke: color, 'stroke-width': 2.6 }); break;
          case 'cross': add('path', { d: 'M3.5 3.5 12.5 12.5M12.5 3.5 3.5 12.5', stroke: color, 'stroke-width': 2.8, 'stroke-linecap': 'round' }); break;
          case 'star': add('path', Object.assign({ d: starPath(8, 7.2, 3.1), fill: color }, casing)); break;
          default: add('circle', Object.assign({ cx: 8, cy: 8, r: 5.4, fill: color }, casing));
        }
      }
      return s;
    }

    function layerKind(d) {
      return (d.kinds && d.kinds[0]) || 'point';
    }

    function saveLayers() {
      const o = store.get('layers') || {};
      for (const d of A.layers) o[d.id] = A.on.has(d.id);
      store.set('layers', o);
    }

    function setLayer(id, on) {
      if (on === A.on.has(id)) return;
      if (on) A.on.add(id);
      else A.on.delete(id);
      saveLayers();
      syncRows();
      if (on) fetchMarks(null, true);
      else {
        // Dropping a layer needs no answer: forget it and keep the rest.
        delete A.marks.layers[id];
        A.marks.key = marksKey(wantedLayers(A.marks.bpp || map.view.bpp));
        if (A.card && A.card.layer === id) closeCard();
        map.invalidate();
      }
      scheduleHash();
    }

    function setTileLayer(id, on) {
      if (on) A.tilesOn.add(id);
      else A.tilesOn.delete(id);
      const o = store.get('tiles') || {};
      o[id] = on;
      store.set('tiles', o);
      map.setTiles(A.tileList.map(l => Object.assign({}, l, { visible: A.tilesOn.has(l.id) })));
      syncRows();
    }

    function buildPanel() {
      const body = $('side-body');
      body.textContent = '';
      const tiles = (A.frameDesc && A.frameDesc.tiles) || [];
      if (tiles.length) {
        const sec = el('section', 'grp');
        sec.appendChild(el('h3', 'grp-title', 'Base map'));
        for (const d of tiles) {
          const row = el('div', 'lrow trow');
          row.dataset.tiles = d.id;
          const lab = el('label', 'lrow-main');
          const cb = el('input');
          cb.type = 'checkbox';
          cb.checked = A.tilesOn.has(d.id);
          cb.addEventListener('change', () => setTileLayer(d.id, cb.checked));
          const sw = el('span', 'tswatch');
          sw.appendChild(icon('tiles'));
          lab.append(cb, sw, el('span', 'lname', d.label || d.id), el('span', 'lcount', d.overlay ? 'overlay' : ''));
          row.appendChild(lab);
          if (d.attribution) row.appendChild(el('p', 'ldesc', d.attribution));
          sec.appendChild(row);
        }
        body.appendChild(sec);
      }
      const groups = A.groups.slice();
      for (const d of A.layers) if (!groups.some(g => g.id === d.group)) groups.push({ id: d.group, label: d.group || 'Other' });
      for (const g of groups) {
        const ls = A.layers.filter(d => d.group === g.id);
        if (!ls.length) continue;
        const sec = el('section', 'grp');
        const h = el('h3', 'grp-title', g.label);
        sec.appendChild(h);
        for (const d of ls) {
          const row = el('div', 'lrow');
          row.dataset.layer = d.id;
          const lab = el('label', 'lrow-main');
          const cb = el('input');
          cb.type = 'checkbox';
          cb.checked = A.on.has(d.id);
          cb.addEventListener('change', () => setLayer(d.id, cb.checked));
          const descId = 'ld-' + d.id.replace(/[^A-Za-z0-9_-]/g, '_');
          const color = (d.style && d.style.color) || '#cccccc';
          lab.append(cb, swatch(layerKind(d), d.style && d.style.glyph, color), el('span', 'lname', d.label), el('span', 'lcount num'));
          row.appendChild(lab);
          if (d.description) {
            const p = el('p', 'ldesc', d.description);
            p.id = descId;
            p.title = d.description;
            cb.setAttribute('aria-describedby', descId);
            row.appendChild(p);
          }
          if (d.legend && d.legend.length) {
            const ul = el('ul', 'legend');
            ul.setAttribute('aria-label', `${d.label} legend`);
            for (const e of d.legend) {
              const li = el('li', 'chip');
              li.append(swatch(layerKind(d), e.glyph || (d.style && d.style.glyph), e.color), el('span', null, e.label));
              ul.appendChild(li);
            }
            row.appendChild(ul);
          }
          sec.appendChild(row);
        }
        body.appendChild(sec);
      }
      syncRows();
    }

    /** Checkbox state, zoom greying, and the counts of the last answer. */
    function syncRows() {
      const bpp = map.view.bpp;
      for (const row of document.querySelectorAll('.lrow[data-layer]')) {
        const d = A.byId.get(row.dataset.layer);
        if (!d) continue;
        const on = A.on.has(d.id);
        row.classList.toggle('on', on);
        row.querySelector('input').checked = on;
        const hint = zoomHint(d, bpp);
        row.classList.toggle('outzoom', !!hint);
        const count = row.querySelector('.lcount');
        const data = A.marks.layers[d.id];
        count.classList.remove('clustered');
        if (hint) {
          count.textContent = hint === 'in' ? 'zoom in' : 'zoom out';
          row.title = hint === 'in' ? `${d.label}: shown when zoomed in further` : `${d.label}: shown when zoomed out further`;
        } else {
          row.title = '';
          if (!on || !data) count.textContent = '';
          else if (data.clustered) {
            count.textContent = MV.compact(data.total);
            count.classList.add('clustered');
            count.title = `${fmtInt(data.total)} in view, grouped`;
          } else {
            count.textContent = data.shown < data.total ? `${MV.compact(data.shown)} of ${MV.compact(data.total)}` : MV.compact(data.total);
            count.title = `${fmtInt(data.total)} in view`;
          }
        }
        row.classList.toggle('nopast', A.t != null && on && !d.has_past);
      }
      for (const row of document.querySelectorAll('.lrow[data-tiles]')) {
        const on = A.tilesOn.has(row.dataset.tiles);
        row.classList.toggle('on', on);
        row.querySelector('input').checked = on;
      }
      for (const sec of document.querySelectorAll('#side-body .grp')) {
        const rows = sec.querySelectorAll('.lrow');
        let n = 0;
        for (const r of rows) if (r.classList.contains('on')) n++;
        sec.querySelector('.grp-title').dataset.count = `${n}/${rows.length}`;
      }
    }
    const updateCounts = () => syncRows();
    let zoomRowsTimer = 0;
    function updateZoomRows() {
      if (zoomRowsTimer) return;
      zoomRowsTimer = setTimeout(() => {
        zoomRowsTimer = 0;
        syncRows();
      }, 100);
    }

    // ----------------------------------------------------------------------------------------------
    // Cards
    // ----------------------------------------------------------------------------------------------
    const cardBox = $('card');
    const cardBody = $('card-body');

    const cardCtx = {
      layerOf: id => A.byId.get(id),
      onLink: (layer, id) => openCard(layer, id, { push: true, fly: true }),
      onFly: at => flyTo(at[0], at[1]),
      fmtTick: t => MV.fmtGameTime(t, A.tpd),
      fmtAt: at => `x ${fmtInt(Math.floor(at[0]))}, z ${fmtInt(Math.floor(at[1]))}`,
    };

    function openCard(layer, id, o) {
      o = o || {};
      if (o.push && A.card && !sameRef(A.card, { layer, id })) A.stack.push(A.card);
      else if (!o.push) A.stack = [];
      A.card = { layer, id: String(id) };
      A.selected = A.card;
      showSheet('card');
      loadCard({ fly: !!o.fly });
      map.invalidate();
      scheduleHash();
    }

    function loadCard(o) {
      o = o || {};
      const ref = A.card;
      if (!ref) return;
      const seq = ++A.cardSeq;
      $('card-back').hidden = A.stack.length === 0;
      if (!o.quiet) MV.renderCardState(cardBody, { loading: true });
      const q = `api/thing/${encodeURIComponent(ref.layer)}/${encodeURIComponent(ref.id)}${A.t != null ? `?t=${A.t}` : ''}`;
      getJSON(q).then(card => {
        if (seq !== A.cardSeq) return;
        A.cardData = card;
        const keep = cardBody.scrollTop;
        MV.renderCard(cardBody, card, cardCtx);
        if (o.quiet) cardBody.scrollTop = keep;
        if (o.fly && card.at) flyTo(card.at[0], card.at[1]);
      }, e => {
        if (seq !== A.cardSeq) return;
        A.cardData = null;
        const d = A.byId.get(ref.layer);
        MV.renderCardState(cardBody, e.status === 404
          ? { title: 'Not here', error: A.t != null ? `${d ? d.label : ref.layer} ${ref.id} did not exist at this time.` : `${d ? d.label : ref.layer} ${ref.id} is gone.` }
          : { title: 'Could not open the card', error: e.message });
      });
    }

    let cardTimer = 0;
    function refreshCard() {
      clearTimeout(cardTimer);
      cardTimer = setTimeout(() => loadCard({ quiet: true }), 250);
    }

    function closeCard() {
      A.card = null;
      A.cardData = null;
      A.stack = [];
      A.selected = null;
      A.cardSeq++;
      cardBox.hidden = true;
      if (A.sheet === 'card') A.sheet = null;
      placeOverlays();
      map.invalidate();
      scheduleHash();
    }

    $('card-close').addEventListener('click', () => {
      closeCard();
      canvas.focus({ preventScroll: true });
    });
    $('card-back').addEventListener('click', () => {
      if (!A.stack.length) return;
      A.card = A.stack.pop();
      A.selected = A.card;
      loadCard({ fly: true });
      map.invalidate();
      scheduleHash();
    });

    // ----------------------------------------------------------------------------------------------
    // Panels: a side column and a bottom row on a desk, sheets on a phone (one at a time)
    // ----------------------------------------------------------------------------------------------
    const side = $('side'), tlPanel = $('tl');

    function showSheet(name) {
      if (phoneMQ.matches) {
        if (name !== 'side') setSide(false, true);
        if (name !== 'timeline') setTimeline(false, true);
        if (name !== 'card' && A.card) closeCard();
      }
      A.sheet = name;
      if (name === 'card') cardBox.hidden = false;
      placeOverlays();
    }

    // On a desk the card covers the stage's right edge: the zoom buttons and the scale bar step
    // left of it. On a phone the card is a sheet and they stay put.
    function placeOverlays() {
      if (!map) return;
      const covered = !cardBox.hidden && !phoneMQ.matches;
      $('stage').classList.toggle('card-open', covered);
      map.setScaleBar({
        right: covered ? cardBox.offsetWidth + 26 : 14, bottom: 14,
        ink: cssVar('--scale-ink') || '#ffffff', halo: cssVar('--scale-halo') || 'rgba(0,0,0,.55)',
      });
    }

    function setSide(on, quiet) {
      if (on) showSheet('side');
      side.hidden = !on;
      $('btn-layers').setAttribute('aria-expanded', String(on));
      if (!quiet && !phoneMQ.matches) store.set('side', on);
      if (on && phoneMQ.matches) $('side-close').focus({ preventScroll: true });
    }

    function setTimeline(on, quiet) {
      if (!timeline) return;
      if (on) showSheet('timeline');
      tlPanel.hidden = !on;
      if (!on && A.eventSel) { A.eventSel = null; A.eventHover = null; if (map) map.invalidate(); }
      $('btn-timeline').setAttribute('aria-expanded', String(on));
      timeline.setOpen(on);
      if (!quiet && !phoneMQ.matches) store.set('timeline', on);
    }

    $('btn-layers').addEventListener('click', () => setSide(side.hidden));
    $('side-close').addEventListener('click', () => {
      setSide(false);
      $('btn-layers').focus({ preventScroll: true });
    });
    $('btn-timeline').addEventListener('click', () => setTimeline(tlPanel.hidden));
    $('zoom-in').addEventListener('click', () => map.zoomAt(map.view.w / 2, map.view.h / 2, 2));
    $('zoom-out').addEventListener('click', () => map.zoomAt(map.view.w / 2, map.view.h / 2, 0.5));
    $('zoom-home').addEventListener('click', () => {
      cancelFlight();
      map.home();
    });

    function layoutChanged() {
      if (!map) return;
      const phone = phoneMQ.matches;
      html.classList.toggle('phone', phone);
      placeOverlays();
      if (phone) {
        // One sheet at a time: keep the card if one is open, else nothing.
        if (A.card) {
          setSide(false, true);
          setTimeline(false, true);
        } else if (!side.hidden && !tlPanel.hidden) setTimeline(false, true);
      }
    }
    if (phoneMQ.addEventListener) phoneMQ.addEventListener('change', layoutChanged);

    document.addEventListener('keydown', ev => {
      if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
      const tag = ev.target && ev.target.tagName;
      if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') {
        if (ev.key !== 'Escape') return;
      }
      if (ev.key === 'Escape') {
        if (A.card) closeCard();
        else if (phoneMQ.matches && !side.hidden) setSide(false);
        else if (phoneMQ.matches && !tlPanel.hidden) setTimeline(false);
        else return;
        ev.preventDefault();
      } else if ((ev.key === 'l' || ev.key === 'L') && ev.target === canvas) {
        setSide(side.hidden);
      } else if ((ev.key === 't' || ev.key === 'T') && ev.target === canvas && timeline) {
        setTimeline(tlPanel.hidden);
      }
    });

    // ----------------------------------------------------------------------------------------------
    // Time: the source badge, the past
    // ----------------------------------------------------------------------------------------------
    const SOURCES = { save: 'Save', live: 'Live', host: 'Host', demo: 'Demo' };

    function setTime(time, fresh) {
      if (time) {
        A.time = time;
        A.timeAt = now();
        if (time.ticks_per_day) A.tpd = time.ticks_per_day;
      }
      if (fresh) {
        A.fresh = fresh;
        A.freshAt = now();
      }
      updateBadge();
    }

    function updateBadge() {
      if (!A.world) return;
      const src = A.world.source || 'save';
      const badge = $('source');
      const st = stale();
      const down = !!(A.world.stream && !A.connected && A.helloSeen);
      badge.className = `source src-${src}${st ? ' stale' : ''}${down ? ' down' : ''}${A.time && A.time.running && !st && !down ? ' running' : ''}`;
      $('src-kind').textContent = SOURCES[src] || src;
      $('src-time').textContent = A.time ? MV.fmtGameTime(nowTick(), A.tpd) : '';
      let fresh = '';
      const f = A.fresh;
      const age = f && f.age_ms != null ? f.age_ms + (now() - A.freshAt) : null;
      if (down) fresh = 'reconnecting';
      else if (st) fresh = `stale, updated ${fmtAge(age)}`;
      else if (f && f.note) fresh = f.note;
      else if (f && f.cycle_ms) fresh = `updated ${fmtAge(age)}`;
      $('src-fresh').textContent = fresh;
      $('src-fresh').hidden = !fresh;
      const bits = [A.world.source_note, A.world.build].filter(Boolean);
      badge.title = bits.join('. ');
    }

    function updatePast() {
      const past = A.t != null;
      $('past').hidden = !past;
      if (past) $('past-time').textContent = MV.fmtGameTime(A.t, A.tpd);
      $('stage').classList.toggle('in-past', past);
      syncRows();
    }

    /** Shows the map at tick `t` (null: now). */
    function setT(t, fromTimeline) {
      if (t != null && t >= nowTick()) t = null;
      if (t === A.t) return;
      A.t = t;
      if (!fromTimeline && timeline) timeline.setT(t);
      updatePast();
      fetchMarks(null, true);
      if (A.card) refreshCard();
      scheduleHash();
      map.invalidate();
    }
    $('past-now').addEventListener('click', () => setT(null));

    // ----------------------------------------------------------------------------------------------
    // The stream
    // ----------------------------------------------------------------------------------------------
    const parse = s => {
      try {
        return JSON.parse(s);
      } catch (e) {
        return null;
      }
    };

    function onFrameMsg(msg) {
      if (!msg) return;
      A.heard.frames++;
      setTime(msg.time, msg.freshness);
      if (A.t != null) return;                       // the past does not move
      const changed = changedLayers(msg, A.frame, A.marks.layers);
      if (changed.length) {
        // A partial answer keeps the shares only when nothing was grouped (the budget did not bind).
        const grouped = Object.values(A.marks.layers).some(l => l && l.clustered);
        fetchMarks(grouped ? null : changed, true);
        if (A.card && changed.includes(A.card.layer)) refreshCard();
      }
    }

    function onLiveEvents(msg) {
      const rows = rowsForFrame(msg && msg.rows, A.frame, A.world.frames[0] && A.world.frames[0].id);
      if (!rows.length) return;
      A.heard.rows += rows.length;
      if (timeline) timeline.addLive(rows);
      if (A.t != null) return;
      const at = now();
      for (const r of rows.slice(-40)) A.flashes.push({ x: r.x, z: r.z, c: kindColor(r.kind), at });
      map.invalidate();
    }

    function openStream() {
      if (A.es || !A.world || !A.world.stream || typeof EventSource !== 'function') return;
      const es = new EventSource('api/stream');
      A.es = es;
      es.addEventListener('open', () => {
        A.connected = true;
        updateBadge();
      });
      es.addEventListener('error', () => {
        A.connected = false;
        updateBadge();
        if (es.readyState === 2 && A.es === es) {
          A.es = null;
          setTimeout(() => { if (!document.hidden) openStream(); }, 5000);
        }
      });
      es.addEventListener('hello', ev => {
        const w = parse(ev.data);
        if (!w) return;
        A.connected = true;
        setTime(w.time, w.freshness);
        // Back after a gap: what was missed is unknown, so ask again.
        if (A.helloSeen) {
          fetchMarks(null, true);
          if (A.card) refreshCard();
          if (timeline) timeline.refresh();
        }
        A.helloSeen = true;
      });
      es.addEventListener('frame', ev => onFrameMsg(parse(ev.data)));
      es.addEventListener('events', ev => onLiveEvents(parse(ev.data)));
    }

    function closeStream() {
      if (!A.es) return;
      A.es.close();
      A.es = null;
      A.connected = false;
      updateBadge();
    }

    document.addEventListener('visibilitychange', () => {
      clearTimeout(A.hideTimer);
      if (document.hidden) A.hideTimer = setTimeout(closeStream, 60000);
      else if (!A.es) openStream();
    });

    // ----------------------------------------------------------------------------------------------
    // The URL hash
    // ----------------------------------------------------------------------------------------------
    function hashState() {
      const v = map.view;
      return {
        frame: A.frame, x: v.cx, z: v.cz, bpp: v.bpp,
        layers: A.layers.filter(d => A.on.has(d.id)).map(d => d.id), t: A.t, card: A.card,
      };
    }
    function scheduleHash() {
      if (!A.ready) return;
      clearTimeout(A.hashTimer);
      A.hashTimer = setTimeout(() => {
        const h = buildHash(hashState());
        if (h === location.hash) return;
        A.ownHash = h;
        try {
          history.replaceState(null, '', h);
        } catch (e) { /* a sandboxed frame */ }
      }, 300);
    }

    function applyLayersFromHash(h) {
      if (!h.layers) return false;
      A.on = new Set(h.layers.filter(id => A.byId.has(id)));
      return true;
    }

    async function applyHash(h) {
      if (applyLayersFromHash(h)) syncRows();
      if (h.frame && h.frame !== A.frame && A.world.frames.some(f => f.id === h.frame)) await setFrame(h.frame, h);
      else if (h.x != null && h.z != null) map.setView(h.x, h.z, h.bpp || map.view.bpp);
      setT(h.t, false);
      if (h.card && !sameRef(h.card, A.card)) openCard(h.card.layer, h.card.id, {});
      fetchMarks(null, true);
    }
    globalThis.addEventListener('hashchange', () => {
      if (location.hash === A.ownHash) return;
      applyHash(parseHash(location.hash));
    });

    // ----------------------------------------------------------------------------------------------
    // Frames
    // ----------------------------------------------------------------------------------------------
    async function setFrame(id, view) {
      const f = A.world.frames.find(x => x.id === id) || A.world.frames[0];
      const changing = A.frame !== f.id;
      A.frame = f.id;
      A.frameDesc = f;
      A.marks = { key: null, box: null, bpp: 0, t: null, layers: {} };
      A.lastDraw = null;
      A.events = [];
      A.eventLegs = [];
      A.eventSel = null;
      if (changing && A.card) closeCard();
      let proj;
      try {
        proj = await MV.loadProjection(f);
      } catch (e) {
        toast(`The projection of ${f.label} did not load: ${e.message}`);
        proj = MV.projectionFor(f);
      }
      const usable = (f.tiles || []).filter(d => MV.pyramidFor(d));
      const shown = tilesShown(usable, store.get('tiles') || {});
      const list = [];
      let base = null;
      for (const d of usable) {
        const pyr = MV.pyramidFor(d);
        if (!base) base = pyr;
        if (shown[d.id]) A.tilesOn.add(d.id);
        else A.tilesOn.delete(d.id);
        list.push({ id: d.id, url: MV.tileUrl(d.url), alpha: d.alpha != null ? d.alpha : 1, pyramid: pyr, bounds: d.bounds || null, visible: A.tilesOn.has(d.id) });
      }
      A.tileList = list;
      map.setProjection(proj, base || MV.squarePyramid({}), f.bounds || null);
      map.setTiles(list);
      if (view && view.x != null && view.z != null) map.setView(view.x, view.z, view.bpp || map.view.bpp);
      else map.home();
      $('frame').value = f.id;
      buildPanel();
      updateReadout();
      fetchMarks(null, true);
      if (timeline) timeline.refresh();
      scheduleHash();
    }

    $('frame').addEventListener('change', ev => setFrame(ev.target.value, null));

    // ----------------------------------------------------------------------------------------------
    // Start
    // ----------------------------------------------------------------------------------------------
    function animate() {
      requestAnimationFrame(animate);
      if (!A.ready || document.hidden) return;
      const live = A.t == null && A.time && A.time.running && A.time.tps > 0 && !stale();
      if ((live && A.lastDraw && A.lastDraw.tracks.length) || A.flashes.length) map.invalidate();
    }

    async function start() {
      const bootBox = $('boot');
      let world, layers;
      try {
        [world, layers] = await Promise.all([getJSON('api/world'), getJSON('api/layers')]);
      } catch (e) {
        bootBox.textContent = '';
        const retry = el('button', 'btn', 'Try again');
        retry.type = 'button';
        retry.addEventListener('click', () => location.reload());
        bootBox.append(el('p', null, `The viewer server did not answer (${e.message}).`), retry);
        bootBox.classList.add('failed');
        return;
      }
      A.world = world;
      A.groups = layers.groups || [];
      A.layers = layers.layers || [];
      for (const d of A.layers) A.byId.set(d.id, d);
      document.title = world.name ? `${world.name}: mic-viewer` : 'mic-viewer';
      $('world-name').textContent = world.name || '';
      setTime(world.time, world.freshness);

      // Frames.
      const sel = $('frame');
      for (const f of world.frames || []) {
        const o = el('option', null, f.label || f.id);
        o.value = f.id;
        sel.appendChild(o);
      }
      $('frame-pick').hidden = (world.frames || []).length < 2;

      // Layers: the link's, else this viewer's last, else the defaults.
      const h = parseHash(location.hash);
      if (!applyLayersFromHash(h)) {
        const saved = store.get('layers') || {};
        for (const d of A.layers) if (saved[d.id] != null ? saved[d.id] : d.default_on) A.on.add(d.id);
      }

      map = MV.createMap({
        canvas,
        projection: { period: 0, zRange: null },
        pyramid: MV.squarePyramid({}),
        background: cssVar('--map-bg') || '#05070b',
        minBpp: 1 / 4,
        onView, onHover, onClick, onKey,
        overlays: [overlay],
        scaleBar: { right: 14, bottom: 14 },
        start: false,
      });
      applyTheme();

      if (world.has_events !== false) {
        timeline = MV.createTimeline({
          root: $('tl-root'),
          getJSON,
          kinds: world.event_kinds || [],
          tpd: A.tpd,
          frame: () => A.frame,
          box: () => viewBox(map.view, 0),
          now: nowTick,
          onScrub: t => setT(t, true),
          onPick: row => {
            A.eventSel = row;
            flyTo(row.x, row.z);
            map.invalidate();
          },
          onRows: (rows, legs) => {
            A.events = rows || [];
            A.eventLegs = legs || [];
            // A picked event stays marked when the list is asked again after the fly (new objects,
            // same event); it goes with the timeline, a frame change or the next pick.
            map.invalidate();
          },
          onHoverRow: row => {
            A.eventHover = row;
            map.invalidate();
          },
          store: { get: k => store.get('tl.' + k), set: (k, v) => store.set('tl.' + k, v) },
        });
        const close = el('button', 'btn icon-btn tl-close');
        close.type = 'button';
        close.setAttribute('aria-label', 'Close the timeline');
        close.title = 'Close the timeline (T)';
        close.appendChild(icon('close'));
        close.addEventListener('click', () => {
          setTimeline(false);
          $('btn-timeline').focus({ preventScroll: true });
        });
        $('tl-root').querySelector('.tl-head').appendChild(close);
      } else {
        $('btn-timeline').hidden = true;
      }

      A.t = h.t != null && h.t < (world.time ? world.time.tick : Infinity) ? h.t : null;
      if (timeline && A.t != null) timeline.setT(A.t);
      await setFrame(h.frame && world.frames.some(f => f.id === h.frame) ? h.frame : world.frames[0].id, h);
      A.ready = true;
      updatePast();
      layoutChanged();
      const phone = phoneMQ.matches;
      const sideOn = store.get('side');
      setSide(phone ? false : sideOn != null ? !!sideOn : true, true);
      if (timeline) setTimeline(!phone && !!store.get('timeline'), true);
      if (h.card) openCard(h.card.layer, h.card.id, {});
      bootBox.hidden = true;
      map.start();
      onView();
      openStream();
      setInterval(updateBadge, 1000);
      requestAnimationFrame(animate);
      html.classList.add('ready');
    }

    MV.app = {
      get ready() { return A.ready; },
      get map() { return map; },
      get timeline() { return timeline; },
      get state() { return A; },
      get lastDraw() { return A.lastDraw; },
      /** Nothing loading, nothing waiting to draw. */
      idle() { return A.ready && A.pending === 0 && !A.marksTimer && !A.flight && map.idle(); },
      openCard: (layer, id) => openCard(layer, id, {}),
      closeCard,
      setFrame: id => setFrame(id, null),
      setT: t => setT(t, false),
      setTimeline: on => setTimeline(on),
      setSide: on => setSide(on),
      flyTo,
    };

    start();
  }

  Object.assign(MV, { parseHash, buildHash, inZoom, zoomHint, viewBox, marksURL, changedLayers, rowsForFrame, tilesShown, isStale, tickAt, fmtAge });
  if (typeof module === 'object' && module.exports) module.exports = MV;

  if (typeof document !== 'undefined' && document.getElementById) {
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
    else boot();
  }
})();
