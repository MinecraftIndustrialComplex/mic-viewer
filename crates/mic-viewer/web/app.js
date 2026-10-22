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

  /** Why a shown layer has nothing (the marks answer's `empty`), or null: only for a layer that is
   *  on, in its zoom range, and holds no marks at all. */
  function emptyReason(on, hint, data) {
    return on && !hint && data && data.empty && !data.total ? String(data.empty) : null;
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

  const SOURCES = { save: 'Save', live: 'Live', host: 'Host', demo: 'Demo' };
  const groupInt = n => {
    const s = String(Math.abs(Math.round(n))).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return n < 0 && s !== '0' ? '-' + s : s;
  };

  /** "day 3,467 at 11:46": a tick as a game day and its clock. */
  function fmtDayAt(tick, tpd) {
    return `day ${groupInt(MV.gameDay(tick, tpd))} at ${MV.gameClock(tick, tpd)}`;
  }

  /** The header's line: "Save, day 3,467 at 11:46" (the source alone without a clock). */
  function fmtSourceTime(source, tick, tpd) {
    const name = SOURCES[source] || (source ? source.charAt(0).toUpperCase() + source.slice(1) : 'Save');
    return tick == null ? name : `${name}, ${fmtDayAt(tick, tpd)}`;
  }

  /** `s` as a sentence: a capital first, a full stop last ("" stays ""). */
  function sentence(s) {
    s = String(s || '').trim();
    if (!s) return '';
    s = s.charAt(0).toUpperCase() + s.slice(1);
    return /[.!?…]$/.test(s) ? s : s + '.';
  }

  /**
   * The header's status as two sentences, `[clock, note]`: "Save, day 3,467 at 11:46." and
   * "Nothing moves." A note that starts by naming the source again ("a save: nothing moves") drops
   * that; with no note the clock has no full stop.
   */
  function fmtStatus(source, tick, tpd, note) {
    const head = fmtSourceTime(source, tick, tpd);
    const name = (SOURCES[source] || source || 'save').toLowerCase();
    const rest = String(note || '').trim().replace(new RegExp(`^(?:(?:a|an|the)\\s+)?${name.replace(/[^a-z0-9]/g, '.')}\\s*:\\s*`, 'i'), '');
    return rest ? [sentence(head), sentence(rest)] : [head, ''];
  }

  /**
   * A layer's count in the panel, `{ text, title }`: what is on screen now (`drawn`, from the last
   * draw), so it does not move with how far round the view the last answer reached. A layer of
   * cells (never counted on screen) shows the answer's total; nothing yet shows nothing.
   */
  function layerCount(d, data, drawn) {
    const cells = (d.kinds || []).length > 0 && d.kinds.every(k => k === 'cell');
    if (data && cells) return { text: fmtCount(data.total), title: `${groupInt(data.total)} in view` };
    if (!data || drawn == null) return { text: '', title: '' };
    const title = data.clustered ? `${groupInt(drawn)} on screen, grouped where they crowd`
      : data.shown < data.total ? `${groupInt(drawn)} on screen; not every one is drawn at this zoom` : `${groupInt(drawn)} on screen`;
    return { text: fmtCount(drawn), title };
  }

  /** The key's row for count bubbles: a layer of its own, after the layers, when any is drawn. */
  const CLUSTER_KEY = {
    id: '_clusters', label: 'Many in one place: zoom in', kinds: ['cluster'], style: { color: 'ink' }, legend: [],
    description: 'A bubble stands for that many things too close together to draw one by one.',
  };

  /**
   * What the map key lists: for each layer drawn (in `layers` order) the symbols on screen, from
   * `seen` (a Map, or an object, of layer id to the legend indices drawn, -1 for the layer's own
   * style). Each group: `{ layer, entries: [{ index, label }] }`, legend order, the layer's own
   * style last (named after the layer, or "Other" beside legend entries). With `opts.clusters`
   * (count bubbles drawn) a last group says what a bubble is.
   */
  function keyGroups(seen, layers, opts) {
    const out = [];
    if (!seen) return out;
    const get = id => (typeof seen.get === 'function' ? seen.get(id) : seen[id]);
    for (const d of layers || []) {
      const s = get(d.id);
      if (!s) continue;
      const legend = d.legend || [];
      const idx = [...new Set([...s].map(Number))]
        .filter(i => i === -1 || (Number.isInteger(i) && i >= 0 && i < legend.length))
        .sort((a, b) => (a < 0) - (b < 0) || a - b);
      if (!idx.length) continue;
      const entries = idx.map(i => ({ index: i, label: i >= 0 ? legend[i].label : idx.length > 1 ? 'Other' : d.label }));
      out.push({ layer: d, entries });
    }
    if (opts && opts.clusters > 0) out.push({ layer: CLUSTER_KEY, entries: [{ index: -1, label: CLUSTER_KEY.label }] });
    return out;
  }

  /** `seen` worked out from the marks held, for drawing code that does not report it: each layer's
   *  legend indices, a cluster's from its `by` counts, -1 for a mark without one. */
  function seenFromAnswers(list) {
    const seen = new Map();
    for (const { desc, data } of list || []) {
      const s = new Set();
      for (const m of (data && data.marks) || []) {
        if (m.k === 'cluster' && m.by && Object.keys(m.by).length) for (const k of Object.keys(m.by)) s.add(Number(k));
        else s.add(m.ci != null ? m.ci : -1);
      }
      if (s.size) seen.set(desc.id, s);
    }
    return seen;
  }

  /** The zoom to fly to for a thing found by name: town level or closer, inside its layer's range. */
  function findBpp(desc, bpp) {
    let b = Math.min(bpp, 2);
    if (desc && desc.max_bpp != null && b > desc.max_bpp) b = desc.max_bpp;
    if (desc && desc.min_bpp != null && b < desc.min_bpp) b = desc.min_bpp;
    return b;
  }

  /** The base map shown: the first base (not overlay) tile layer that is on, or null for none. */
  function baseShown(tiles, on) {
    const d = (tiles || []).find(t => !t.overlay && on.has(t.id));
    return d ? d.id : null;
  }

  /** A layer's count in the panel: whole numbers to 9,999, then compact ("12k"). */
  function fmtCount(n) {
    if (n == null || !isFinite(n)) return '';
    if (Math.abs(n) < 10000) return groupInt(n);
    return MV.compact ? MV.compact(n) : groupInt(n);
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
      // The map's colours (`--map-*`) and the muted base map's filter, read on a theme change.
      theme: null, muteFilter: null,
      // Layers with nothing in this frame at all (id -> why), and layers just turned on.
      empty: new Map(), justOn: new Set(),
      rows: null, groupBoxes: null, notHere: null,
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

    /** The map's colours (viewer.css's `--map-*`), read once per theme change, not per frame. */
    function readTheme() {
      const dark = effectiveTheme() === 'dark';
      A.theme = {
        dark,
        ink: cssVar('--map-ink') || (dark ? '#ebe6dc' : '#1f2328'),
        ink2: cssVar('--map-ink-2') || (dark ? '#a59f94' : '#5b6470'),
        paper: cssVar('--map-paper') || (dark ? '#14171b' : '#f1efe9'),
        halo: cssVar('--map-halo') || (dark ? 'rgba(14, 16, 19, 0.9)' : 'rgba(255, 255, 255, 0.92)'),
        bad: cssVar('--map-bad') || (dark ? '#ff6b6b' : '#d1242f'),
        warn: cssVar('--map-warn') || (dark ? '#e3b341' : '#b7791f'),
        off: cssVar('--map-off') || '#8b949e',
      };
      A.muteFilter = cssVar('--map-mute') || null;
    }

    const muted = () => store.get('muted') !== false;
    function applyMute() {
      if (map) map.setTileFilter(muted() ? A.muteFilter : null);
    }

    function applyTheme() {
      const s = store.get('theme');
      if (s === 'dark' || s === 'light') html.dataset.theme = s;
      else delete html.dataset.theme;
      readTheme();
      const dark = A.theme.dark;
      const b = $('btn-theme');
      b.setAttribute('aria-label', dark ? 'Switch to the light theme' : 'Switch to the dark theme');
      b.title = dark ? 'Light theme' : 'Dark theme';
      b.textContent = '';
      b.append(icon(dark ? 'sun' : 'moon'), el('span', 'tbtn-label', dark ? 'Light' : 'Dark'));
      paintSwatches();
      if (map) {
        map.setBackground(A.theme.paper);
        applyMute();
        placeOverlays();
      }
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
      const th = A.theme;
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
          ctx.lineWidth = 3;
          ctx.strokeStyle = th.halo;
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
          ctx.strokeStyle = th.halo;
          ctx.stroke();
          ctx.lineWidth = 2;
          ctx.strokeStyle = th.ink;
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
      A.lastDraw = MV.drawMarks(ctx, map, drawList(), { t: drawT(), selected: A.selected, hovered: A.hovered, theme: A.theme });
      drawEvents(ctx);
      scheduleKey();
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

    /** Opens what `hit` is (`at`: the world point picked, kept in view of the card). */
    function activate(hit, at) {
      if (!hit) return false;
      if (hit.cluster) {
        flyTo(hit.wx, hit.wz, map.view.bpp / 3);
        return true;
      }
      if (hit.id != null) {
        openCard(hit.layer, hit.id, { at });
        return true;
      }
      return false;
    }

    function onClick(x, z, px, py) {
      if (!A.ready) return;
      const hit = A.lastDraw ? MV.pick(A.lastDraw.targets, px, py) : null;
      if (!activate(hit, [x, z]) && A.card) closeCard();
    }

    function onKey(ev) {
      if (ev.key === 'Enter' || ev.key === ' ') {
        // The mark under the pointer, else the one at the centre.
        const v = map.view;
        const p = A.cursorPx || [v.w / 2, v.h / 2];
        return activate(A.lastDraw ? MV.pick(A.lastDraw.targets, p[0], p[1]) : null, map.toWorld(p[0], p[1]));
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

    /** `flyTo`, landing (x, z) in the middle of the part of the map no panel covers. */
    function flyShown(x, z, bpp) {
      const v = map.view;
      const target = clamp(bpp || Math.min(v.bpp, 4), map.minBpp(), map.maxBpp());
      const [x0, y0, x1, y1] = openPart();
      flyTo(x - ((x0 + x1) / 2 - v.w / 2) * target, z - ((y0 + y1) / 2 - v.h / 2) * target, target);
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
        noteEmpty(got);
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
    // Swatches: the map's own drawing code (marks.js's drawSwatch) in a small canvas, painted again
    // on a theme change; an SVG stand-in where the drawing code has none.
    // ----------------------------------------------------------------------------------------------
    function starPath(c, R, r) {
      let d = '';
      for (let i = 0; i < 10; i++) {
        const a = -Math.PI / 2 + (i * Math.PI) / 5, rr = i % 2 ? r : R;
        d += (i ? 'L' : 'M') + (c + rr * Math.cos(a)).toFixed(2) + ' ' + (c + rr * Math.sin(a)).toFixed(2);
      }
      return d + 'Z';
    }

    /** A 16 x 16 SVG swatch: the glyph of a point or leg layer, a line, an area, a grid or text. */
    function svgSwatch(kind, glyph, color) {
      const s = svgEl('svg', { viewBox: '0 0 16 16', class: 'glyph', 'aria-hidden': 'true' });
      const add = (tag, a) => s.appendChild(svgEl(tag, a));
      const casing = { stroke: 'rgba(0,0,0,.45)', 'stroke-width': 1 };
      if (kind === 'edge') add('path', { d: 'M2 13 14 3', stroke: color, 'stroke-width': 2.6, 'stroke-linecap': 'round', fill: 'none' });
      else if (kind === 'path') add('path', { d: 'M1.5 12 5.5 6.5 9.5 10 14.5 3.5', stroke: color, 'stroke-width': 2, 'stroke-linejoin': 'round', 'stroke-linecap': 'round', fill: 'none' });
      else if (kind === 'area') add('rect', { x: 2, y: 2, width: 12, height: 12, rx: 2, fill: color, 'fill-opacity': 0.25, stroke: color, 'stroke-width': 1.5 });
      else if (kind === 'cell') {
        const fs = [0.15, 0.45, 0.7, 0.95];
        fs.forEach((f, i) => add('rect', { x: 2 + (i % 2) * 6.5, y: 2 + Math.floor(i / 2) * 6.5, width: 5.5, height: 5.5, rx: 1, fill: MV.rampColor('heat', f) }));
      } else if (kind === 'label') {
        const t = svgEl('text', { x: 8, y: 12.5, 'text-anchor': 'middle', 'font-size': 11, 'font-weight': 600, fill: 'currentColor' });
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

    function paintSwatch(c) {
      const s = c._sw;
      const dpr = globalThis.devicePixelRatio || 1;
      c.width = Math.round(s.w * dpr);
      c.height = Math.round(s.h * dpr);
      const g = c.getContext('2d');
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.clearRect(0, 0, s.w, s.h);
      try {
        MV.drawSwatch(g, s.desc, s.index, s.w, s.h, A.theme);
      } catch (e) { /* a swatch is not worth an error */ }
    }
    function paintSwatches() {
      if (!MV.drawSwatch) return;
      for (const c of document.querySelectorAll('canvas.sw')) if (c._sw) paintSwatch(c);
    }

    /** The swatch of a layer (`index` null) or of one of its legend entries, `w` x `h` CSS px. */
    function swatchEl(desc, index, w, h) {
      w = w || 22;
      h = h || 14;
      let s;
      if (MV.drawSwatch) {
        s = el('canvas', 'sw');
        s._sw = { desc, index, w, h };
        paintSwatch(s);
      } else {
        const e = index != null && desc.legend ? desc.legend[index] : null;
        const color = (e && e.color) || (desc.style && desc.style.color) || '#888888';
        s = svgSwatch(layerKind(desc), (e && e.glyph) || (desc.style && desc.style.glyph), color);
      }
      s.setAttribute('aria-hidden', 'true');
      s.style.width = w + 'px';
      s.style.height = h + 'px';
      return s;
    }

    // ----------------------------------------------------------------------------------------------
    // The layer panel: the base map, then the layers by group, then (collapsed) the layers this world
    // has nothing for
    // ----------------------------------------------------------------------------------------------
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

    /** Notes which layers answered that this frame has nothing for them; one just turned on says so. */
    function noteEmpty(got) {
      for (const [id, data] of Object.entries(got)) {
        const why = emptyReason(true, null, data);
        if (why) {
          if (!A.empty.has(id) && A.justOn.has(id)) {
            const d = A.byId.get(id);
            toast(`${d ? d.label : id}: not in this world (${why}).`);
          }
          A.empty.set(id, why);
        } else A.empty.delete(id);
        A.justOn.delete(id);
      }
    }

    const usableTiles = () => ((A.frameDesc && A.frameDesc.tiles) || []).filter(d => MV.pyramidFor(d));

    function pushTiles() {
      map.setTiles(A.tileList.map(l => Object.assign({}, l, { visible: A.tilesOn.has(l.id) })));
    }
    function saveTiles() {
      const o = store.get('tiles') || {};
      for (const l of A.tileList) o[l.id] = A.tilesOn.has(l.id);
      store.set('tiles', o);
    }
    function setTileLayer(id, on) {
      if (on) A.tilesOn.add(id);
      else A.tilesOn.delete(id);
      saveTiles();
      pushTiles();
      syncRows();
    }
    /** Shows one base map (`id`), or none (null); overlays stay as they are. */
    function setBase(id) {
      for (const l of A.tileList) {
        if (l.overlay) continue;
        if (l.id === id) A.tilesOn.add(l.id);
        else A.tilesOn.delete(l.id);
      }
      saveTiles();
      pushTiles();
      syncRows();
    }

    function layerRow(d) {
      const row = el('div', 'lrow');
      row.dataset.layer = d.id;
      if (d.description) row.title = d.description;
      const lab = el('label', 'lrow-main');
      const cb = el('input');
      cb.type = 'checkbox';
      cb.checked = A.on.has(d.id);
      cb.addEventListener('change', () => {
        if (cb.checked) A.justOn.add(d.id);
        setLayer(d.id, cb.checked);
      });
      lab.append(cb, swatchEl(d, null, 22, 14), el('span', 'lname', d.label), el('span', 'lcount num'));
      row.appendChild(lab);
      // The description and the key of the layer, behind a toggle.
      const detail = el('div', 'ldetail');
      detail.id = 'ld-' + d.id.replace(/[^A-Za-z0-9_-]/g, '_');
      detail.hidden = true;
      if (d.description) detail.appendChild(el('p', 'ldesc', d.description));
      if (d.legend && d.legend.length) {
        const ul = el('ul', 'legend');
        ul.setAttribute('aria-label', `${d.label} key`);
        d.legend.forEach((e, i) => {
          const li = el('li');
          li.append(swatchEl(d, i, 22, 14), el('span', null, e.label));
          ul.appendChild(li);
        });
        detail.appendChild(ul);
      }
      if (detail.childNodes.length) {
        const more = el('button', 'lmore');
        more.type = 'button';
        more.setAttribute('aria-expanded', 'false');
        more.setAttribute('aria-controls', detail.id);
        more.setAttribute('aria-label', `About ${d.label}`);
        more.title = 'What this layer shows';
        more.appendChild(icon('chevron'));
        more.addEventListener('click', () => {
          const open = detail.hidden;
          detail.hidden = !open;
          more.setAttribute('aria-expanded', String(open));
        });
        row.appendChild(more);
        if (d.description) cb.setAttribute('aria-describedby', detail.id);
      }
      row.appendChild(detail);
      return row;
    }

    function buildPanel() {
      const body = $('side-body');
      body.textContent = '';
      A.rows = new Map();
      A.groupBoxes = new Map();
      const tiles = usableTiles();
      if (tiles.length) {
        const sec = el('section', 'grp');
        sec.appendChild(el('h3', 'grp-title', 'Base map'));
        const line = el('div', 'base');
        const bases = tiles.filter(d => !d.overlay);
        if (bases.length) {
          const seg = el('div', 'seg');
          seg.id = 'base-seg';
          seg.setAttribute('role', 'group');
          seg.setAttribute('aria-label', 'Base map');
          for (const d of [...bases, { id: '', label: 'None' }]) {
            const b = el('button', null, d.label || d.id);
            b.type = 'button';
            b.dataset.base = d.id;
            // A label cut short in a narrow column is still said in full on hover.
            b.title = d.attribution ? `${d.label || d.id}. ${d.attribution}` : d.label || d.id;
            b.addEventListener('click', () => setBase(d.id || null));
            seg.appendChild(b);
          }
          line.appendChild(seg);
        }
        const mute = el('label', 'mute');
        mute.title = 'Wash out the base map so what is drawn over it reads first';
        const cb = el('input');
        cb.type = 'checkbox';
        cb.id = 'mute';
        cb.checked = muted();
        cb.addEventListener('change', () => {
          store.set('muted', cb.checked);
          applyMute();
        });
        mute.append(cb, document.createTextNode('Muted'));
        line.appendChild(mute);
        sec.appendChild(line);
        for (const d of tiles.filter(t => t.overlay)) {
          const row = el('div', 'lrow trow');
          row.dataset.tiles = d.id;
          if (d.attribution) row.title = d.attribution;
          const lab = el('label', 'lrow-main');
          const tcb = el('input');
          tcb.type = 'checkbox';
          tcb.checked = A.tilesOn.has(d.id);
          tcb.addEventListener('change', () => setTileLayer(d.id, tcb.checked));
          const sw = el('span', 'tswatch');
          sw.appendChild(icon('tiles'));
          lab.append(tcb, sw, el('span', 'lname', d.label || d.id), el('span', 'lcount'));
          row.appendChild(lab);
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
        sec.dataset.group = g.id;
        sec.appendChild(el('h3', 'grp-title', g.label));
        const list = el('div', 'grp-rows');
        for (const d of ls) {
          const row = layerRow(d);
          A.rows.set(d.id, row);
          list.appendChild(row);
        }
        sec.appendChild(list);
        A.groupBoxes.set(g.id, { sec, list });
        body.appendChild(sec);
      }
      const box = el('details', 'nothere');
      const sum = el('summary');
      const title = el('span');
      sum.append(icon('chevron'), title);
      const list = el('div', 'nothere-rows');
      box.append(sum, list);
      box.hidden = true;
      body.appendChild(box);
      A.notHere = { box, list, title };
      syncRows();
    }

    /** Puts a layer's row in its group, or in "Not in this world" when the frame has nothing for it,
     *  keeping the layers' order. Moves it only when it changes place (a move drops the focus). */
    function placeRow(d, row, away) {
      const target = away ? A.notHere.list : (A.groupBoxes.get(d.group) || {}).list;
      if (!target || row.parentNode === target) return;
      let before = null;
      for (let j = A.layers.indexOf(d) + 1; j < A.layers.length && !before; j++) {
        const r = A.rows.get(A.layers[j].id);
        if (r && r.parentNode === target) before = r;
      }
      target.insertBefore(row, before);
    }

    /** Checkbox state, places, zoom greying, and the counts of the last answer. */
    function syncRows() {
      if (!A.rows || !map) return;
      const bpp = map.view.bpp;
      for (const d of A.layers) {
        const row = A.rows.get(d.id);
        if (!row) continue;
        const on = A.on.has(d.id);
        const why = A.empty.get(d.id) || null;
        placeRow(d, row, why);
        row.classList.toggle('on', on);
        row.querySelector('input').checked = on;
        const hint = zoomHint(d, bpp);
        row.classList.toggle('outzoom', !!hint);
        const count = row.querySelector('.lcount');
        if (hint) {
          count.textContent = hint === 'in' ? 'zoom in' : 'zoom out';
          count.title = hint === 'in' ? `${d.label}: drawn when zoomed in further` : `${d.label}: drawn when zoomed out further`;
        } else setCount(d, count, on);
        row.classList.toggle('nopast', A.t != null && on && !d.has_past);
        const detail = row.querySelector('.ldetail');
        let note = detail.querySelector('.lempty');
        if (why) {
          if (!note) {
            note = el('p', 'lempty');
            detail.prepend(note);
          }
          note.textContent = why.charAt(0).toUpperCase() + why.slice(1);
        } else if (note) note.remove();
      }
      for (const row of document.querySelectorAll('.lrow[data-tiles]')) {
        const on = A.tilesOn.has(row.dataset.tiles);
        row.classList.toggle('on', on);
        row.querySelector('input').checked = on;
      }
      const base = baseShown(A.tileList, A.tilesOn) || '';
      for (const b of document.querySelectorAll('#base-seg button')) b.setAttribute('aria-pressed', String(b.dataset.base === base));
      const mute = $('mute');
      if (mute) mute.checked = muted();
      for (const { sec, list } of A.groupBoxes.values()) sec.hidden = !list.children.length;
      const n = A.notHere.list.children.length;
      A.notHere.box.hidden = !n;
      A.notHere.title.textContent = `Not in this world (${n})`;
    }
    const updateCounts = () => syncRows();

    /** A layer's count cell from the last draw (blank while the layer is off or not yet drawn). */
    function setCount(d, cell, on) {
      const drawn = A.lastDraw && A.lastDraw.counts ? A.lastDraw.counts.get(d.id) : undefined;
      const c = on ? layerCount(d, A.marks.layers[d.id], drawn) : { text: '', title: '' };
      if (cell.textContent !== c.text) cell.textContent = c.text;
      cell.title = c.title;
    }

    /** The counts alone, after a draw (on the key's clock, not every frame). */
    function syncCounts() {
      if (!A.rows || !map) return;
      for (const d of A.layers) {
        const row = A.rows.get(d.id);
        if (!row || row.classList.contains('outzoom')) continue;
        setCount(d, row.querySelector('.lcount'), A.on.has(d.id));
      }
    }
    let zoomRowsTimer = 0;
    function updateZoomRows() {
      if (zoomRowsTimer) return;
      zoomRowsTimer = setTimeout(() => {
        zoomRowsTimer = 0;
        syncRows();
      }, 100);
    }

    // ----------------------------------------------------------------------------------------------
    // The key: every symbol drawn on screen now, by layer, with the map's own swatches
    // ----------------------------------------------------------------------------------------------
    const keyBox = $('key'), keyHead = $('key-head'), keyBody = $('key-body');
    let keyTimer = 0, keySig = null;

    function scheduleKey() {
      if (keyTimer) return;
      keyTimer = setTimeout(() => {
        keyTimer = 0;
        updateKey();
      }, 250);
    }

    function updateKey() {
      if (!A.ready) return;
      syncCounts();
      const seen = A.lastDraw && A.lastDraw.seen && typeof A.lastDraw.seen.get === 'function' ? A.lastDraw.seen : seenFromAnswers(drawList());
      const groups = keyGroups(seen, A.layers, { clusters: A.lastDraw ? A.lastDraw.clusters : 0 });
      const sig = groups.map(g => g.layer.id + ':' + g.entries.map(e => e.index).join(',')).join('|');
      if (sig === keySig) return;
      keySig = sig;
      keyBox.hidden = !groups.length;
      keyBody.textContent = '';
      for (const g of groups) {
        const box = el('div', 'key-grp');
        // A layer drawn in one style is one line named after it; one with several gets a heading.
        const single = g.entries.length === 1 && g.entries[0].label === g.layer.label;
        if (!single) box.appendChild(el('h3', null, g.layer.label));
        const ul = el('ul');
        for (const e of g.entries) {
          const li = el('li');
          li.append(swatchEl(g.layer, e.index < 0 ? null : e.index, 22, 14), el('span', null, e.label));
          if (single && g.layer.description) li.title = g.layer.description;
          ul.appendChild(li);
        }
        box.appendChild(ul);
        keyBody.appendChild(box);
      }
    }

    // The key is a fixed width and scrolls inside past a height (viewer.css); its open or shut
    // choice is remembered.
    function setKeyOpen(open, quiet) {
      keyBody.hidden = !open;
      keyHead.setAttribute('aria-expanded', String(open));
      keyHead.title = open ? 'Hide the key' : 'Show the key';
      if (!quiet) store.set('key', open);
    }
    keyHead.addEventListener('click', () => setKeyOpen(keyBody.hidden));

    // ----------------------------------------------------------------------------------------------
    // Cards
    // ----------------------------------------------------------------------------------------------
    const cardBox = $('card');
    const cardBody = $('card-body');

    const cardCtx = {
      layerOf: id => A.byId.get(id),
      onLink: (layer, id) => openCard(layer, id, { push: true, fly: true }),
      onFly: at => flyShown(at[0], at[1]),
      fmtTick: t => MV.fmtGameTime(t, A.tpd),
      fmtAt: at => `x ${fmtInt(Math.floor(at[0]))}, z ${fmtInt(Math.floor(at[1]))}`,
      swatch: d => swatchEl(d, null, 18, 12),
    };

    function openCard(layer, id, o) {
      o = o || {};
      if (o.push && A.card && !sameRef(A.card, { layer, id })) A.stack.push(A.card);
      else if (!o.push) A.stack = [];
      A.card = { layer, id: String(id) };
      A.selected = A.card;
      showSheet('card');
      loadCard({ fly: !!o.fly, at: o.at || null, go: o.go || null });
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
        // Flown to (a link, back): to the middle of the map the card leaves open. Picked on the
        // map: kept out from under the card once it has its height.
        if (o.go) flyShown(o.go[0], o.go[1], o.go[2]);
        else if (o.fly && card.at) flyShown(card.at[0], card.at[1]);
        else if (o.at) reveal(o.at);
      }, e => {
        if (seq !== A.cardSeq) return;
        A.cardData = null;
        if (o.go) flyShown(o.go[0], o.go[1], o.go[2]);
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
    // The scale bar sits between the readout (bottom) and the zoom buttons (viewer.css).
    const SCALE_FONT = '500 11px system-ui, -apple-system, "Segoe UI", sans-serif';

    // What covers the map on a desk, for fitting the opening view into the rest: the key (bottom
    // left, the strip above it too, to keep the fit simple) and an open card (right). On a phone
    // the key is shut and sheets come and go, so nothing.
    const KEY_W = 224;
    function homePad() {
      if (phoneMQ.matches) return null;
      const pad = {};
      if (!keyBody.hidden) pad.left = (keyBox.hidden ? KEY_W : keyBox.offsetWidth) + 24;
      if (!cardBox.hidden) pad.right = cardBox.offsetWidth + 24;
      return pad;
    }

    /**
     * The part of the map not under a panel, in canvas px `[x0, y0, x1, y1]`: above a phone's sheet
     * and below its floating source line, or left of a desk's card.
     */
    function openPart() {
      const v = map.view, c = canvas.getBoundingClientRect();
      let x0 = 0, y0 = 0, x1 = v.w, y1 = v.h;
      if (!cardBox.hidden) {
        const r = cardBox.getBoundingClientRect();
        if (phoneMQ.matches) y1 = Math.min(y1, r.top - c.top);
        else x1 = Math.min(x1, r.left - c.left);
      }
      const src = $('source');
      if (phoneMQ.matches && src && src.offsetParent) y0 = Math.max(y0, src.getBoundingClientRect().bottom - c.top);
      if (x1 - x0 < 80) { x0 = 0; x1 = v.w; }
      if (y1 - y0 < 80) { y0 = 0; y1 = v.h; }
      return [x0, y0, x1, y1];
    }

    /**
     * Brings world point `at` (`[x, z]`) into the open part of the map when a panel covers it (a
     * phone's card sheet over what was tapped, a desk's card over what was found): moves the view,
     * not the zoom, so it sits in the middle of what is left. `always`: even when already in view.
     */
    function reveal(at, always) {
      if (!map || !at) return;
      const v = map.view;
      const [x0, y0, x1, y1] = openPart();
      const [sx, sy] = map.toScreen(at[0], at[1]);
      const m = 24;
      if (!always && sx >= x0 + m && sx <= x1 - m && sy >= y0 + m && sy <= y1 - m) return;
      const mx = (x0 + x1) / 2, my = (y0 + y1) / 2;
      flyTo(at[0] - (mx - v.w / 2) * v.bpp, at[1] - (my - v.h / 2) * v.bpp, v.bpp);
    }

    function placeOverlays() {
      if (!map) return;
      const phone = phoneMQ.matches;
      const covered = !cardBox.hidden && !phone;
      $('stage').classList.toggle('card-open', covered);
      map.setScaleBar({
        right: covered ? cardBox.offsetWidth + 24 : phone ? 16 : 12, bottom: phone ? 44 : 40,
        ink: A.theme.ink, halo: A.theme.halo, font: SCALE_FONT,
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
      map.home(A.frameDesc ? A.frameDesc.home : null);
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
      } else if (ev.key === '/' && !$('find').hidden) {
        ev.preventDefault();
        findQ.focus();
        findQ.select();
      }
    });

    // ----------------------------------------------------------------------------------------------
    // Finding things by name (`/api/find`, when the world says `find`): arrows and Enter pick, a pick
    // flies there and opens the card
    // ----------------------------------------------------------------------------------------------
    const findQ = $('find-q'), findList = $('find-list');
    const F = { seq: 0, timer: 0, items: [], active: -1, q: '' };

    function findOpen(on) {
      findList.hidden = !on;
      findQ.setAttribute('aria-expanded', String(on));
      if (!on) findQ.removeAttribute('aria-activedescendant');
    }

    function findMark() {
      for (const li of findList.querySelectorAll('.find-opt')) {
        const on = Number(li.dataset.i) === F.active;
        li.setAttribute('aria-selected', String(on));
        if (on) {
          findQ.setAttribute('aria-activedescendant', li.id);
          li.scrollIntoView({ block: 'nearest' });
        }
      }
    }

    function findRender() {
      findList.textContent = '';
      if (!F.q) {
        findOpen(false);
        return;
      }
      if (!F.items.length) {
        const li = el('li', 'find-none', `Nothing found for "${F.q}"`);
        li.setAttribute('role', 'option');
        li.setAttribute('aria-disabled', 'true');
        findList.appendChild(li);
      }
      F.items.forEach((r, i) => {
        const d = A.byId.get(r.layer);
        const li = el('li', 'find-opt');
        li.id = 'find-opt-' + i;
        li.dataset.i = String(i);
        li.setAttribute('role', 'option');
        const frame = r.frame && r.frame !== A.frame ? A.world.frames.find(f => f.id === r.frame) : null;
        const kind = [r.kind || (d ? d.label : r.layer), frame ? frame.label || frame.id : null].filter(Boolean).join(', ');
        li.append(d ? swatchEl(d, null, 22, 14) : el('span'), el('span', 'find-label', r.label || String(r.id)), el('span', 'find-kind', kind));
        // Before the input's blur closes the list.
        li.addEventListener('mousedown', ev => {
          ev.preventDefault();
          findPick(i);
        });
        findList.appendChild(li);
      });
      findOpen(true);
      findMark();
    }

    function findAsk() {
      const q = findQ.value.trim();
      F.q = q;
      const seq = ++F.seq;
      if (!q) {
        F.items = [];
        findRender();
        return;
      }
      getJSON(`api/find?q=${encodeURIComponent(q)}&limit=20`).then(ans => {
        if (seq !== F.seq) return;
        F.items = (ans && ans.found) || [];
        F.active = F.items.length ? 0 : -1;
        findRender();
      }, () => {
        if (seq !== F.seq) return;
        F.items = [];
        F.active = -1;
        findRender();
      });
    }

    async function findPick(i) {
      const r = F.items[i];
      if (!r) return;
      findOpen(false);
      canvas.focus({ preventScroll: true });
      const d = A.byId.get(r.layer);
      if (d && !A.on.has(r.layer)) setLayer(r.layer, true);
      const bpp = findBpp(d, map.view.bpp);
      if (r.frame && r.frame !== A.frame && A.world.frames.some(f => f.id === r.frame)) {
        await setFrame(r.frame, { x: r.x, z: r.z, bpp });
        openCard(r.layer, r.id, {});
      } else {
        // The flight waits for the card, to land what was found where the card leaves room.
        openCard(r.layer, r.id, { go: r.x != null && r.z != null ? [r.x, r.z, bpp] : null });
      }
    }

    findQ.addEventListener('input', () => {
      clearTimeout(F.timer);
      F.timer = setTimeout(findAsk, 120);
    });
    findQ.addEventListener('keydown', ev => {
      const n = F.items.length;
      if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
        if (!n) return;
        ev.preventDefault();
        if (findList.hidden) findOpen(true);
        F.active = ev.key === 'ArrowDown' ? (F.active + 1) % n : (F.active - 1 + n) % n;
        findMark();
      } else if (ev.key === 'Enter') {
        ev.preventDefault();
        if (n) findPick(F.active >= 0 ? F.active : 0);
      } else if (ev.key === 'Escape' && !findList.hidden) {
        ev.preventDefault();
        ev.stopPropagation();
        findOpen(false);
      }
    });
    findQ.addEventListener('focus', () => { if (F.q) findRender(); });
    findQ.addEventListener('blur', () => findOpen(false));

    // ----------------------------------------------------------------------------------------------
    // Time: where the data comes from and when, the past
    // ----------------------------------------------------------------------------------------------

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
      let fresh = '';
      const f = A.fresh;
      const age = f && f.age_ms != null ? f.age_ms + (now() - A.freshAt) : null;
      if (down) fresh = 'reconnecting';
      else if (st) fresh = `stale, updated ${fmtAge(age)}`;
      else if (f && f.note) fresh = f.note;
      else if (f && f.cycle_ms) fresh = `updated ${fmtAge(age)}`;
      // One line read as sentences: "Save, day 3,467 at 11:46. Nothing moves."
      const [head, note] = fmtStatus(src, A.time ? nowTick() : null, A.tpd, fresh);
      $('src-text').textContent = head;
      $('src-fresh').textContent = note;
      $('src-fresh').hidden = !note;
      const bits = [A.world.source_note, A.world.build].filter(Boolean);
      badge.title = bits.join('. ');
    }

    function updatePast() {
      const past = A.t != null;
      $('past').hidden = !past;
      if (past) $('past-time').textContent = fmtDayAt(A.t, A.tpd);
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
      A.empty = new Map();
      keySig = null;
      if (changing && A.card) closeCard();
      let proj;
      try {
        proj = await MV.loadProjection(f);
      } catch (e) {
        toast(`The projection of ${f.label} did not load: ${e.message}`);
        proj = MV.projectionFor(f);
      }
      const usable = usableTiles();
      const shown = tilesShown(usable, store.get('tiles') || {});
      const list = [];
      let base = null;
      for (const d of usable) {
        const pyr = MV.pyramidFor(d);
        if (!base) base = pyr;
        if (shown[d.id]) A.tilesOn.add(d.id);
        else A.tilesOn.delete(d.id);
        // Overlays carry data and are never muted; base maps take the map's filter.
        list.push({
          id: d.id, url: MV.tileUrl(d.url), alpha: d.alpha != null ? d.alpha : 1, pyramid: pyr, bounds: d.bounds || null,
          overlay: !!d.overlay, filter: d.overlay ? false : null, visible: A.tilesOn.has(d.id),
        });
      }
      A.tileList = list;
      map.setProjection(proj, base || MV.squarePyramid({}), f.bounds || null);
      map.setTiles(list);
      if (view && view.x != null && view.z != null) map.setView(view.x, view.z, view.bpp || map.view.bpp);
      else map.home(f.home || null);
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
      $('find').hidden = !world.find;

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
        background: A.theme.paper,
        tileFilter: muted() ? A.muteFilter : null,
        minBpp: 1 / 16,
        homePad,
        onView, onHover, onClick, onKey,
        overlays: [overlay],
        scaleBar: { right: 12, bottom: 40, ink: A.theme.ink, halo: A.theme.halo, font: SCALE_FONT },
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
        const close = el('button', 'btn btn-quiet icon-btn tl-close');
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
      // The panels open before the first frame: its fit, and the first marks asked for, then see the
      // map at the size it will be (the counts do not change once the side column comes).
      const phone = phoneMQ.matches;
      const sideOn = store.get('side');
      setSide(phone ? false : sideOn != null ? !!sideOn : true, true);
      if (timeline) setTimeline(!phone && !!store.get('timeline'), true);
      const keyOpen = store.get('key');
      setKeyOpen(phone ? false : keyOpen != null ? !!keyOpen : true, true);
      await setFrame(h.frame && world.frames.some(f => f.id === h.frame) ? h.frame : world.frames[0].id, h);
      A.ready = true;
      updatePast();
      layoutChanged();
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
      openCard: (layer, id, at) => openCard(layer, id, { at: at || null }),
      closeCard,
      setFrame: id => setFrame(id, null),
      setT: t => setT(t, false),
      setTimeline: on => setTimeline(on),
      setSide: on => setSide(on),
      flyTo,
    };

    start();
  }

  Object.assign(MV, {
    parseHash, buildHash, inZoom, zoomHint, viewBox, marksURL, changedLayers, rowsForFrame, emptyReason, tilesShown,
    isStale, tickAt, fmtAge, fmtDayAt, fmtSourceTime, sentence, fmtStatus, layerCount, keyGroups, seenFromAnswers, findBpp,
    baseShown, fmtCount,
  });
  if (typeof module === 'object' && module.exports) module.exports = MV;

  if (typeof document !== 'undefined' && document.getElementById) {
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
    else boot();
  }
})();
