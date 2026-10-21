// SPDX-License-Identifier: MPL-2.0
// The event history (VIEWER.md §10.6): a histogram of `/api/events/histogram` stacked by kind, game
// days on top and wall dates under them; a scrubber that sets the time the map shows (null: live);
// the events of `/api/events` around that time in the map's view, newest first, each a button that
// flies there. Rows from the stream's `events` messages come in live. Also the game-time and wall
// date formatting the page shares. A classic script: adds to `globalThis.MicViewer`.
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  const FONT = 'system-ui, -apple-system, "Segoe UI", Roboto, "DejaVu Sans", sans-serif';
  const TPD = 24000;

  // ------------------------------------------------------------------------------------------------
  // Pure helpers
  // ------------------------------------------------------------------------------------------------

  const pad2 = n => (n < 10 ? '0' : '') + n;

  /** The game day of a tick (day 0 starts at tick 0). */
  function gameDay(tick, tpd) {
    return Math.floor(tick / (tpd || TPD));
  }

  /** The clock of a tick, "hh:mm": tick 0 of a day is 06:00, as in Minecraft. */
  function gameClock(tick, tpd) {
    tpd = tpd || TPD;
    const into = ((tick % tpd) + tpd) % tpd;
    const minutes = Math.floor((into / tpd) * 1440 + 360) % 1440;
    return pad2(Math.floor(minutes / 60)) + ':' + pad2(minutes % 60);
  }

  /** "Day 40, 06:00". */
  function fmtGameTime(tick, tpd) {
    return `Day ${gameDay(tick, tpd)}, ${gameClock(tick, tpd)}`;
  }

  /** A span of ticks for people: "3 days", "1 day 4 h", "45 min". */
  function fmtSpan(ticks, tpd) {
    tpd = tpd || TPD;
    const a = Math.abs(ticks);
    const days = Math.floor(a / tpd), hours = Math.floor(((a % tpd) / tpd) * 24), mins = Math.round(((a % (tpd / 24)) / tpd) * 1440);
    if (days >= 10) return `${days} days`;
    if (days >= 1) return `${days} day${days > 1 ? 's' : ''}${hours ? ` ${hours} h` : ''}`;
    if (hours >= 1) return `${hours} h${mins ? ` ${mins} min` : ''}`;
    return `${mins} min`;
  }

  /**
   * Wall-clock milliseconds at `tick` from `[tick, ms]` pairs (sorted by tick): linear within a
   * pair, the nearest pair's slope outside; one pair: 50 ms a tick; none: null.
   */
  function wallAt(pairs, tick) {
    if (!pairs || !pairs.length) return null;
    if (pairs.length === 1) return pairs[0][1] + (tick - pairs[0][0]) * 50;
    let i = 0;
    while (i < pairs.length - 2 && tick > pairs[i + 1][0]) i++;
    const [t0, m0] = pairs[i], [t1, m1] = pairs[i + 1];
    if (t1 === t0) return m0;
    return m0 + ((tick - t0) * (m1 - m0)) / (t1 - t0);
  }

  /** A wall date in local time: "Oct 21" or, with `time`, "Oct 21 14:05". */
  function fmtWall(ms, time) {
    if (ms == null || !isFinite(ms)) return '';
    const d = new Date(ms);
    const day = `${MONTHS[d.getMonth()]} ${d.getDate()}`;
    return time ? `${day} ${pad2(d.getHours())}:${pad2(d.getMinutes())}` : day;
  }

  /**
   * Wall labels for axis ticks (`ms` per tick, null where unknown): a date only where it changes;
   * when the ticks are less than about a day apart in wall time, the clock too ("Oct 21 14:05",
   * then "16:05" until the date changes).
   */
  function wallLabels(ms) {
    let step = Infinity, prev = null;
    for (const v of ms) {
      if (v == null || !isFinite(v)) continue;
      if (prev != null) step = Math.min(step, Math.abs(v - prev));
      prev = v;
    }
    const clockToo = !(step >= 20 * 3600e3);
    let lastDay = null;
    return ms.map(v => {
      if (v == null || !isFinite(v)) return '';
      const d = new Date(v);
      const day = `${MONTHS[d.getMonth()]} ${d.getDate()}`;
      const clock = `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
      const same = day === lastDay;
      lastDay = day;
      if (!clockToo) return same ? '' : day;
      return same ? clock : `${day} ${clock}`;
    });
  }

  /**
   * Axis ticks between `from` and `to`: the smallest step of whole game hours or days that leaves
   * `minPx` between labels at `pxPerTick`. Returns `{ step, ticks: [tick, ...] }`.
   */
  function axisTicks(from, to, pxPerTick, minPx, tpd) {
    tpd = tpd || TPD;
    const h = tpd / 24;
    const steps = [h, 2 * h, 3 * h, 6 * h, 12 * h, tpd, 2 * tpd, 5 * tpd, 10 * tpd, 20 * tpd, 50 * tpd, 100 * tpd, 200 * tpd, 500 * tpd, 1000 * tpd];
    let step = steps[steps.length - 1];
    for (const s of steps) if (s * pxPerTick >= minPx) { step = s; break; }
    const ticks = [];
    for (let t = Math.ceil(from / step) * step; t <= to && ticks.length < 500; t += step) ticks.push(t);
    return { step, ticks };
  }

  /**
   * Per bucket, the stacked counts of the shown kinds in `order`: `{ stacks: [[[kind, n], ...]],
   * totals, max }`.
   */
  function stackBuckets(series, order, hidden) {
    const kinds = order.filter(k => series[k] && !(hidden && hidden.has(k)));
    for (const k of Object.keys(series || {})) if (!order.includes(k) && !(hidden && hidden.has(k))) kinds.push(k);
    const n = Math.max(0, ...kinds.map(k => series[k].length));
    const stacks = [], totals = [];
    let max = 0;
    for (let i = 0; i < n; i++) {
      const s = [];
      let sum = 0;
      for (const k of kinds) {
        const v = series[k][i] || 0;
        if (v) { s.push([k, v]); sum += v; }
      }
      stacks.push(s);
      totals.push(sum);
      max = Math.max(max, sum);
    }
    return { stacks, totals, max };
  }

  // ------------------------------------------------------------------------------------------------
  // The component
  // ------------------------------------------------------------------------------------------------

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  const SPANS = [['1', '1 day'], ['7', '7 days'], ['30', '30 days'], ['all', 'All']];

  /**
   * `opts`: `{ root, getJSON(url) -> Promise, kinds: event_kinds, tpd, frame() -> id,
   * box() -> [x0, z0, x1, z1] | null, now() -> tick, onScrub(t | null), onPick(row),
   * onRows(rows), onHoverRow(row | null), store: { get(k), set(k, v) } }`.
   */
  function createTimeline(opts) {
    const root = opts.root;
    const tpd = opts.tpd || TPD;
    const kindInfo = new Map((opts.kinds || []).map(k => [k.id, k]));
    const order = (opts.kinds || []).map(k => k.id);
    const S = {
      span: 'all', hidden: new Set(), hist: null, t: null, rows: [], truncated: false,
      open: false, seq: 0, listSeq: 0, hover: -1, w: 0, h: 0, dpr: 1, listTimer: 0, histTimer: 0,
    };
    try {
      const sp = opts.store && opts.store.get('span');
      if (SPANS.some(([k]) => k === sp)) S.span = sp;
      const hid = opts.store && opts.store.get('hiddenKinds');
      if (Array.isArray(hid)) for (const k of hid) S.hidden.add(k);
    } catch (e) { /* storage is optional */ }

    // Skeleton.
    root.textContent = '';
    const head = el('div', 'tl-head');
    const spanSeg = el('div', 'seg');
    spanSeg.setAttribute('role', 'group');
    spanSeg.setAttribute('aria-label', 'Time span');
    for (const [k, label] of SPANS) {
      const b = el('button', null, label);
      b.type = 'button';
      b.dataset.span = k;
      spanSeg.appendChild(b);
    }
    const kindsBox = el('div', 'tl-kinds');
    kindsBox.setAttribute('role', 'group');
    kindsBox.setAttribute('aria-label', 'Event kinds shown');
    for (const id of order) {
      const k = kindInfo.get(id);
      const b = el('button', 'chip tl-kind');
      b.type = 'button';
      b.dataset.kind = id;
      const dot = el('span', 'dot');
      dot.style.background = k.color;
      b.append(dot, el('span', null, k.label), el('span', 'chip-n num', ''));
      b.title = `${k.label} (${k.group}): click to hide or show`;
      kindsBox.appendChild(b);
    }
    const nowBtn = el('button', 'btn btn-sm tl-now', 'Now');
    nowBtn.type = 'button';
    nowBtn.title = 'Back to the present (End)';
    head.append(spanSeg, kindsBox, nowBtn);
    const body = el('div', 'tl-body');
    const chart = el('div', 'tl-chart');
    const canvas = el('canvas', 'tl-canvas');
    canvas.tabIndex = 0;
    canvas.setAttribute('role', 'slider');
    canvas.setAttribute('aria-label', 'Time shown on the map: arrows step, Home goes to the start, End returns to now');
    const tip = el('div', 'tl-tip');
    tip.hidden = true;
    chart.append(canvas, tip);
    const list = el('div', 'tl-list');
    const listHead = el('div', 'tl-list-head');
    const rowsBox = el('ol', 'tl-rows');
    list.append(listHead, rowsBox);
    body.append(chart, list);
    root.append(head, body);
    const ctx = canvas.getContext('2d');

    function save() {
      try {
        if (opts.store) {
          opts.store.set('span', S.span);
          opts.store.set('hiddenKinds', [...S.hidden]);
        }
      } catch (e) { /* storage is optional */ }
    }

    function syncControls() {
      for (const b of spanSeg.children) b.setAttribute('aria-pressed', String(b.dataset.span === S.span));
      nowBtn.disabled = S.t == null;
      const sums = {};
      if (S.hist) for (const [k, v] of Object.entries(S.hist.series || {})) sums[k] = v.reduce((a, b) => a + b, 0);
      for (const b of kindsBox.children) {
        const id = b.dataset.kind;
        b.setAttribute('aria-pressed', String(!S.hidden.has(id)));
        b.lastChild.textContent = sums[id] != null ? (MV.compact ? MV.compact(sums[id]) : String(sums[id])) : '0';
      }
    }

    // ----------------------------------------------------------------------------------------------
    // Data
    // ----------------------------------------------------------------------------------------------
    const now = () => (opts.now ? opts.now() : 0);
    const enc = encodeURIComponent;

    function buckets() {
      return clamp(Math.floor((S.w || 600) / 5), 24, 360);
    }

    function fetchHist() {
      if (!S.open) return;
      const n = now();
      const to = n + 1;
      let q = `api/events/histogram?frame=${enc(opts.frame())}&to=${to}&buckets=${buckets()}`;
      if (S.span !== 'all') q += `&from=${Math.max(0, to - Number(S.span) * tpd)}`;
      const seq = ++S.seq;
      return opts.getJSON(q).then(h => {
        if (seq !== S.seq || !h) return;
        S.hist = h;
        syncControls();
        draw();
        fetchList();
      }, () => {});
    }

    function bucketWidth() {
      const h = S.hist;
      return h ? Math.max(1, (h.to - h.from) / Math.max(1, h.buckets)) : tpd / 6;
    }

    /** The window the list shows: around the scrubbed time, else the last game day. */
    function listWindow() {
      if (S.t != null) {
        const w = Math.max(bucketWidth() * 1.5, tpd / 24);
        return [Math.floor(S.t - w), Math.ceil(S.t + w)];
      }
      const n = now();
      return [n - tpd, n + 1];
    }

    function fetchList() {
      if (!S.open) return;
      const [from, to] = listWindow();
      let q = `api/events?frame=${enc(opts.frame())}&from=${from}&to=${to}&limit=300`;
      const b = opts.box && opts.box();
      if (b) q += `&x0=${Math.floor(b[0])}&z0=${Math.floor(b[1])}&x1=${Math.ceil(b[2])}&z1=${Math.ceil(b[3])}`;
      const shown = order.filter(k => !S.hidden.has(k));
      if (shown.length < order.length) q += `&kinds=${shown.map(enc).join(',')}`;
      const seq = ++S.listSeq;
      return opts.getJSON(q).then(a => {
        if (seq !== S.listSeq || !a) return;
        S.rows = (a.rows || []).slice().sort((x, y) => y.t - x.t);
        S.legs = a.legs || [];
        S.truncated = !!a.truncated;
        S.window = [from, to];
        renderList();
        if (opts.onRows) opts.onRows(S.rows, S.legs);
      }, () => {});
    }

    function scheduleList(ms) {
      clearTimeout(S.listTimer);
      S.listTimer = setTimeout(fetchList, ms == null ? 250 : ms);
    }

    // ----------------------------------------------------------------------------------------------
    // The list
    // ----------------------------------------------------------------------------------------------
    function renderList() {
      rowsBox.textContent = '';
      const [from, to] = S.window || listWindow();
      const where = opts.box && opts.box() ? 'in view' : '';
      const n = S.rows.length;
      listHead.textContent = '';
      listHead.append(
        el('b', null, `${n}${S.truncated ? '+' : ''} event${n === 1 ? '' : 's'} ${where}`),
        el('span', 'muted', S.t != null ? `${fmtGameTime(from, tpd)} to ${gameClock(to, tpd)}` : 'the last game day'),
      );
      if (!n) {
        rowsBox.appendChild(el('li', 'tl-empty', S.t != null ? 'Nothing recorded here around this time.' : 'Nothing recorded here in the last game day.'));
        return;
      }
      for (const r of S.rows.slice(0, 300)) {
        const li = el('li');
        const b = el('button', 'tl-row');
        b.type = 'button';
        const k = kindInfo.get(r.kind) || { label: r.kind, color: '#999' };
        const dot = el('span', 'dot');
        dot.style.background = k.color;
        const when = el('span', 'tl-when num', `D${gameDay(r.t, tpd)} ${gameClock(r.t, tpd)}`);
        const what = el('span', 'tl-what');
        const kind = el('span', 'tl-kindname', k.label);
        if (r.sev >= 2) kind.classList.add('sev2');
        what.append(kind, el('span', 'tl-text', r.text || ''));
        b.append(dot, when, what);
        b.title = `${fmtGameTime(r.t, tpd)}: ${r.text || k.label}. Click to see it on the map.`;
        b.addEventListener('click', () => opts.onPick && opts.onPick(r));
        b.addEventListener('mouseenter', () => opts.onHoverRow && opts.onHoverRow(r));
        b.addEventListener('mouseleave', () => opts.onHoverRow && opts.onHoverRow(null));
        b.addEventListener('focus', () => opts.onHoverRow && opts.onHoverRow(r));
        b.addEventListener('blur', () => opts.onHoverRow && opts.onHoverRow(null));
        li.appendChild(b);
        rowsBox.appendChild(li);
      }
    }

    // ----------------------------------------------------------------------------------------------
    // The chart
    // ----------------------------------------------------------------------------------------------
    const TOP = 18, BOTTOM = 18;

    function resize() {
      const r = canvas.getBoundingClientRect();
      const dpr = globalThis.devicePixelRatio || 1;
      const w = Math.max(1, Math.round(r.width)), h = Math.max(1, Math.round(r.height));
      if (w === S.w && h === S.h && dpr === S.dpr) return false;
      const grew = Math.abs(w - S.w) > 40;
      S.w = w; S.h = h; S.dpr = dpr;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      return grew;
    }

    const css = name => getComputedStyle(root).getPropertyValue(name).trim();
    const xOf = t => {
      const h = S.hist;
      return h ? ((t - h.from) / Math.max(1, h.to - h.from)) * S.w : 0;
    };
    const tOf = x => {
      const h = S.hist;
      return h ? h.from + (clamp(x, 0, S.w) / Math.max(1, S.w)) * (h.to - h.from) : now();
    };

    function draw() {
      if (!S.open) return;
      if (resize() && S.hist) {
        fetchHist();
        return;
      }
      const d = S.dpr, W = S.w, H = S.h;
      ctx.setTransform(d, 0, 0, d, 0, 0);
      ctx.clearRect(0, 0, W, H);
      const h = S.hist;
      const fg2 = css('--fg-2') || '#b0b8c7', fg3 = css('--fg-3') || '#7f8a9d', line = css('--line-2') || 'rgba(160,175,200,.16)';
      const accent = css('--accent') || '#7d8cff';
      if (!h) {
        ctx.fillStyle = fg3;
        ctx.font = `12px ${FONT}`;
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        ctx.fillText('Loading the history', W / 2, H / 2);
        return;
      }
      const chartH = H - TOP - BOTTOM;
      const st = stackBuckets(h.series || {}, order, S.hidden);
      const n = Math.max(1, h.buckets);
      const bw = W / n;
      const scale = st.max > 0 ? chartH / st.max : 0;
      // Grid: the axis, and day lines.
      const ax = axisTicks(h.from, h.to, W / Math.max(1, h.to - h.from), 84, tpd);
      ctx.lineWidth = 1;
      ctx.strokeStyle = line;
      ctx.beginPath();
      for (const t of ax.ticks) {
        const x = Math.round(xOf(t)) + 0.5;
        ctx.moveTo(x, TOP - 4);
        ctx.lineTo(x, TOP + chartH);
      }
      ctx.moveTo(0, TOP + chartH + 0.5);
      ctx.lineTo(W, TOP + chartH + 0.5);
      ctx.stroke();
      // Bars.
      for (let i = 0; i < st.stacks.length; i++) {
        let y = TOP + chartH;
        const x = i * bw;
        const hl = i === S.hover;
        for (const [k, v] of st.stacks[i]) {
          const hh = v * scale;
          const info = kindInfo.get(k);
          ctx.fillStyle = info ? info.color : '#999';
          ctx.globalAlpha = hl ? 1 : 0.82;
          ctx.fillRect(x + (bw > 3 ? 0.5 : 0), y - hh, Math.max(1, bw - (bw > 3 ? 1 : 0)), hh);
          y -= hh;
        }
      }
      ctx.globalAlpha = 1;
      // Labels: game time on top, wall dates under.
      ctx.font = `500 11px ${FONT}`;
      ctx.textBaseline = 'top';
      ctx.textAlign = 'left';
      const daily = ax.step >= tpd;
      const shown = ax.ticks.filter(t => Math.round(xOf(t)) + 3 <= W - 40);
      const walls = wallLabels(shown.map(t => wallAt(h.wall, t)));
      shown.forEach((t, i) => {
        const x = Math.round(xOf(t)) + 3;
        ctx.fillStyle = fg2;
        ctx.fillText(daily || t % tpd === 0 ? `Day ${gameDay(t, tpd)}` : gameClock(t, tpd), x, 2);
        ctx.fillStyle = fg3;
        if (walls[i]) ctx.fillText(walls[i], x, TOP + chartH + 4);
      });
      // The time shown: the scrubber, or "now" at the right end.
      const cur = S.t != null ? S.t : null;
      if (cur != null) {
        const x = Math.round(xOf(cur)) + 0.5;
        ctx.strokeStyle = accent;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(x, TOP - 2);
        ctx.lineTo(x, TOP + chartH);
        ctx.stroke();
        ctx.fillStyle = accent;
        ctx.beginPath();
        ctx.moveTo(x - 5, TOP - 8);
        ctx.lineTo(x + 5, TOP - 8);
        ctx.lineTo(x, TOP - 2);
        ctx.closePath();
        ctx.fill();
      } else {
        ctx.fillStyle = css('--live') || '#3fcf8e';
        ctx.fillRect(W - 3, TOP, 3, chartH);
      }
      canvas.setAttribute('aria-valuemin', String(h.from));
      canvas.setAttribute('aria-valuemax', String(h.to));
      canvas.setAttribute('aria-valuenow', String(cur != null ? cur : h.to));
      canvas.setAttribute('aria-valuetext', cur != null ? fmtGameTime(cur, tpd) : 'Now');
    }

    function showTip(i, x) {
      const h = S.hist;
      if (!h || i < 0 || i >= h.buckets) { tip.hidden = true; return; }
      const bw = bucketWidth();
      const t0 = h.from + i * bw, t1 = t0 + bw;
      const parts = [];
      let total = 0;
      for (const k of order) {
        const v = h.series[k] ? h.series[k][i] || 0 : 0;
        if (v && !S.hidden.has(k)) { parts.push([k, v]); total += v; }
      }
      tip.textContent = '';
      tip.appendChild(el('b', null, `${fmtGameTime(t0, tpd)} to ${gameClock(t1, tpd)}`));
      tip.appendChild(el('div', 'muted', `${total} event${total === 1 ? '' : 's'}`));
      for (const [k, v] of parts.sort((a, b) => b[1] - a[1]).slice(0, 5)) {
        const row = el('div', 'tl-tip-row');
        const dot = el('span', 'dot');
        dot.style.background = kindInfo.get(k).color;
        row.append(dot, el('span', null, kindInfo.get(k).label), el('span', 'num', String(v)));
        tip.appendChild(row);
      }
      tip.hidden = false;
      const tw = tip.offsetWidth || 180;
      tip.style.left = `${clamp(x - tw / 2, 0, Math.max(0, S.w - tw))}px`;
    }

    // ----------------------------------------------------------------------------------------------
    // Input
    // ----------------------------------------------------------------------------------------------
    function setT(t, fromUser) {
      const h = S.hist;
      if (t != null && h) t = clamp(Math.round(t), h.from, h.to - 1);
      if (t != null && t >= now()) t = null;   // the present is "live"
      if (t === S.t) return;
      S.t = t;
      syncControls();
      draw();
      scheduleList(fromUser ? 200 : 0);
      if (fromUser && opts.onScrub) opts.onScrub(t);
    }

    let drag = null;
    canvas.addEventListener('pointerdown', ev => {
      if (!S.hist) return;
      if (ev.pointerType !== 'touch') canvas.focus({ preventScroll: true });
      try { canvas.setPointerCapture(ev.pointerId); } catch (e) { /* synthetic events */ }
      drag = ev.pointerId;
      const r = canvas.getBoundingClientRect();
      setT(tOf(ev.clientX - r.left), true);
    });
    canvas.addEventListener('pointermove', ev => {
      const r = canvas.getBoundingClientRect();
      const x = ev.clientX - r.left;
      if (drag === ev.pointerId) setT(tOf(x), true);
      const h = S.hist;
      const i = h ? Math.floor((x / Math.max(1, S.w)) * h.buckets) : -1;
      if (i !== S.hover) {
        S.hover = i;
        draw();
      }
      showTip(i, x);
    });
    const up = ev => { if (drag === ev.pointerId) drag = null; };
    canvas.addEventListener('pointerup', up);
    canvas.addEventListener('pointercancel', up);
    canvas.addEventListener('pointerleave', () => {
      S.hover = -1;
      tip.hidden = true;
      draw();
    });
    canvas.addEventListener('keydown', ev => {
      const h = S.hist;
      if (!h) return;
      const bw = bucketWidth();
      const cur = S.t != null ? S.t : h.to;
      let used = true;
      if (ev.key === 'ArrowLeft') setT(cur - bw * (ev.shiftKey ? 10 : 1), true);
      else if (ev.key === 'ArrowRight') setT(cur + bw * (ev.shiftKey ? 10 : 1), true);
      else if (ev.key === 'PageUp') setT(cur - bw * 10, true);
      else if (ev.key === 'PageDown') setT(cur + bw * 10, true);
      else if (ev.key === 'Home') setT(h.from, true);
      else if (ev.key === 'End') setT(null, true);
      else used = false;
      if (used) ev.preventDefault();
    });
    nowBtn.addEventListener('click', () => setT(null, true));
    spanSeg.addEventListener('click', ev => {
      const b = ev.target.closest('[data-span]');
      if (!b || b.dataset.span === S.span) return;
      S.span = b.dataset.span;
      save();
      syncControls();
      fetchHist();
    });
    kindsBox.addEventListener('click', ev => {
      const b = ev.target.closest('[data-kind]');
      if (!b) return;
      const k = b.dataset.kind;
      if (S.hidden.has(k)) S.hidden.delete(k);
      else S.hidden.add(k);
      save();
      syncControls();
      draw();
      scheduleList(0);
    });
    if (typeof ResizeObserver === 'function') new ResizeObserver(() => draw()).observe(canvas);
    syncControls();

    return {
      get t() { return S.t; },
      get rows() { return S.rows; },
      get legs() { return S.legs || []; },
      get open() { return S.open; },
      hiddenKinds: () => new Set(S.hidden),
      setOpen(on) {
        S.open = on;
        if (on) {
          resize();
          fetchHist();
        } else if (opts.onRows) opts.onRows([], []);
      },
      /** The map shows `t` (null: live); from the page (a deep link, an event click, "Now"). */
      setT(t) { setT(t, false); },
      /** The view or the frame changed: the list follows. */
      viewChanged() { scheduleList(350); },
      refresh() { fetchHist(); },
      /** Rows from the stream: into the list when they belong there, the histogram later. */
      addLive(rows) {
        if (!S.open || !rows || !rows.length) return;
        if (S.t == null) {
          const b = opts.box && opts.box();
          const fresh = rows.filter(r => !S.hidden.has(r.kind) && (!b || (r.x >= b[0] && r.x <= b[2] && r.z >= b[1] && r.z <= b[3])));
          if (fresh.length) {
            S.rows = fresh.concat(S.rows).sort((x, y) => y.t - x.t).slice(0, 300);
            renderList();
            if (opts.onRows) opts.onRows(S.rows, S.legs || []);
          }
        }
        clearTimeout(S.histTimer);
        S.histTimer = setTimeout(fetchHist, 5000);
      },
      redraw: draw,
    };
  }

  Object.assign(MV, { gameDay, gameClock, fmtGameTime, fmtSpan, wallAt, fmtWall, wallLabels, axisTicks, stackBuckets, createTimeline });
  if (typeof module === 'object' && module.exports) module.exports = MV;
})();
