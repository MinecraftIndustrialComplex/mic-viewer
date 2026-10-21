// SPDX-License-Identifier: MPL-2.0
// Renders any card of `/api/thing/<layer>/<id>` (docs/api.md §5, VIEWER.md §4.4) with one
// renderer: title, subtitle, fields with units and formatted numbers, links that open the linked
// thing's card, sections, small series as sparklines, and when the card is older than now (`as_of`,
// `note`). Every server string goes in as text, never as markup. A classic script: adds to
// `globalThis.MicViewer`.
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});
  const SVG = 'http://www.w3.org/2000/svg';

  // ------------------------------------------------------------------------------------------------
  // Pure helpers
  // ------------------------------------------------------------------------------------------------

  function group(intPart) {
    return intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  }

  /**
   * A number for people: thousands separated, as many decimals as its size deserves (1,234,567;
   * 1,235 for 1,234.5; 123.4; 12.35; 1.04; 0.0123), never "-0", tiny and huge values in exponent
   * form.
   */
  function fmtNum(v) {
    if (typeof v !== 'number') v = Number(v);
    if (!isFinite(v)) return isNaN(v) ? '-' : v > 0 ? 'inf' : '-inf';
    const a = Math.abs(v);
    if (a !== 0 && (a >= 1e15 || a < 1e-4)) return v.toExponential(2).replace('e+', 'e');
    let s;
    if (a >= 1000 || Number.isInteger(v)) s = String(Math.round(a));
    else if (a >= 100) s = a.toFixed(1);
    else if (a >= 1) s = a.toFixed(2);
    else s = a.toPrecision(3);
    if (s.includes('.')) s = s.replace(/0+$/, '').replace(/\.$/, '');
    const [i, f] = s.split('.');
    const out = group(i) + (f ? '.' + f : '');
    return v < 0 && out !== '0' ? '-' + out : out;
  }

  /** A field's value with its unit: numbers formatted, "%" and the like stuck to the number. */
  function fmtValue(value, unit) {
    const text = typeof value === 'number' ? fmtNum(value) : value == null ? '' : String(value);
    if (!unit) return text;
    return /^(%|°|°C|°F|‰|x)$/.test(unit) ? text + unit : text + ' ' + unit;
  }

  /**
   * Whether a section's fields read better as a compact table than as a key and value list: many
   * rows (a schedule, a cargo list), every label and value short.
   */
  function isCompact(fields) {
    if (!Array.isArray(fields) || fields.length < 6) return false;
    return fields.every(f => f && String(f.label || '').length <= 28 && fmtValue(f.value, f.unit).length <= 20);
  }

  /**
   * A sparkline of `points` (`[[x, y], ...]`) in a `w` x `h` box: the line's and the filled area's
   * SVG path data, and the extremes. Points are sorted by x; y grows downward on screen.
   */
  function sparkPath(points, w, h, pad) {
    pad = pad == null ? 2 : pad;
    const pts = (points || []).filter(p => p && isFinite(p[0]) && isFinite(p[1])).slice().sort((a, b) => a[0] - b[0]);
    if (!pts.length) return { line: '', area: '', min: null, max: null, first: null, last: null };
    let x0 = pts[0][0], x1 = pts[pts.length - 1][0];
    let y0 = Infinity, y1 = -Infinity;
    for (const p of pts) { y0 = Math.min(y0, p[1]); y1 = Math.max(y1, p[1]); }
    if (x1 === x0) { x0 -= 1; x1 += 1; }
    const flat = y1 === y0;
    const sx = x => pad + ((x - x0) / (x1 - x0)) * (w - 2 * pad);
    const sy = y => (flat ? h / 2 : pad + (1 - (y - y0) / (y1 - y0)) * (h - 2 * pad));
    const r = v => Math.round(v * 100) / 100;
    const line = pts.map((p, i) => (i ? 'L' : 'M') + r(sx(p[0])) + ' ' + r(sy(p[1]))).join(' ');
    const area = line + ` L${r(sx(pts[pts.length - 1][0]))} ${h} L${r(sx(pts[0][0]))} ${h} Z`;
    return { line, area, min: y0, max: y1, first: pts[0], last: pts[pts.length - 1] };
  }

  // ------------------------------------------------------------------------------------------------
  // DOM
  // ------------------------------------------------------------------------------------------------

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  /** A field's value into `cell`: a link that opens the linked card, or text. */
  function fieldValue(cell, f, ctx) {
    const text = fmtValue(f.value, f.unit);
    if (f.link && ctx.onLink) {
      const b = el('button', 'linkbtn', text);
      b.type = 'button';
      const target = ctx.layerOf ? ctx.layerOf(f.link.layer) : null;
      b.title = `Open ${target ? target.label.toLowerCase() : f.link.layer} ${text}`;
      b.addEventListener('click', () => ctx.onLink(f.link.layer, String(f.link.id)));
      cell.appendChild(b);
    } else {
      cell.textContent = text;
      if (typeof f.value === 'number') cell.classList.add('num');
    }
  }

  function fieldList(fields, ctx) {
    const dl = el('dl', 'kv');
    for (const f of fields || []) {
      const dd = el('dd');
      fieldValue(dd, f, ctx);
      dl.append(el('dt', null, f.label), dd);
    }
    return dl;
  }

  function fieldTable(fields, ctx) {
    const table = el('table', 'ctab');
    const body = el('tbody');
    for (const f of fields) {
      const tr = el('tr');
      const v = el('td', 'v');
      fieldValue(v, f, ctx);
      tr.append(el('td', null, f.label), v);
      body.appendChild(tr);
    }
    table.appendChild(body);
    return table;
  }

  function sparkline(s, ctx) {
    const fig = el('figure', 'spark');
    const head = el('figcaption', 'spark-head');
    const name = el('span', 'spark-name', s.label);
    const p = sparkPath(s.points, 300, 48, 3);
    const lastV = p.last ? fmtValue(p.last[1], s.unit) : '';
    head.append(name, el('span', 'spark-last num', lastV));
    fig.appendChild(head);
    if (p.line) {
      const svg = document.createElementNS(SVG, 'svg');
      svg.setAttribute('viewBox', '0 0 300 48');
      svg.setAttribute('preserveAspectRatio', 'none');
      svg.setAttribute('class', 'spark-svg');
      svg.setAttribute('role', 'img');
      svg.setAttribute('aria-label', `${s.label}: ${fmtNum(p.min)} to ${fmtNum(p.max)}${s.unit ? ' ' + s.unit : ''}`);
      const area = document.createElementNS(SVG, 'path');
      area.setAttribute('d', p.area);
      area.setAttribute('class', 'spark-area');
      const line = document.createElementNS(SVG, 'path');
      line.setAttribute('d', p.line);
      line.setAttribute('class', 'spark-line');
      line.setAttribute('vector-effect', 'non-scaling-stroke');
      svg.append(area, line);
      fig.appendChild(svg);
      const foot = el('div', 'spark-foot');
      const t = ctx.fmtTick || (x => String(x));
      foot.append(
        el('span', null, t(p.first[0])),
        el('span', 'num', `${fmtNum(p.min)} to ${fmtNum(p.max)}`),
        el('span', null, t(p.last[0])),
      );
      fig.appendChild(foot);
    } else {
      fig.appendChild(el('p', 'muted', 'No points yet.'));
    }
    return fig;
  }

  /**
   * Renders `card` into `root` (emptied first). `ctx`: `{ layerOf(id) -> layer descriptor,
   * onLink(layer, id), onFly([x, z]), fmtTick(t) -> text, fmtAt([x, z]) -> text }`.
   */
  function renderCard(root, card, ctx) {
    ctx = ctx || {};
    root.textContent = '';
    const desc = ctx.layerOf ? ctx.layerOf(card.layer) : null;
    const head = el('header', 'card-head');
    const kicker = el('div', 'card-kicker');
    let sw = desc && ctx.swatch ? ctx.swatch(desc) : null;
    if (!sw) {
      sw = el('span', 'swatch');
      if (desc && desc.style) sw.style.background = desc.style.color;
    }
    kicker.append(sw, el('span', null, desc ? desc.label : card.layer));
    const title = el('h2', 'card-title', card.title || `${card.layer} ${card.id}`);
    title.id = 'card-title';
    head.append(kicker, title);
    if (card.subtitle) head.appendChild(el('p', 'card-sub', card.subtitle));
    root.appendChild(head);

    if (card.at && ctx.onFly) {
      const tools = el('div', 'card-tools');
      const fly = el('button', 'btn btn-sm', 'Fly to');
      fly.type = 'button';
      fly.addEventListener('click', () => ctx.onFly(card.at));
      tools.append(fly, el('span', 'card-at num', ctx.fmtAt ? ctx.fmtAt(card.at) : `${Math.round(card.at[0])}, ${Math.round(card.at[1])}`));
      root.appendChild(tools);
    }
    if (card.as_of != null || card.note) {
      const parts = [];
      if (card.as_of != null) parts.push('As of ' + (ctx.fmtTick ? ctx.fmtTick(card.as_of) : card.as_of));
      if (card.note) parts.push(card.note);
      const text = parts.join('. ');
      root.appendChild(el('p', 'card-note', text.charAt(0).toUpperCase() + text.slice(1)));
    }
    if (card.fields && card.fields.length) root.appendChild(fieldList(card.fields, ctx));
    for (const s of card.sections || []) {
      const sec = el('section', 'card-sec');
      sec.append(el('h3', null, s.title), isCompact(s.fields) ? fieldTable(s.fields, ctx) : fieldList(s.fields, ctx));
      root.appendChild(sec);
    }
    if (card.series && card.series.length) {
      const sec = el('section', 'card-sec');
      sec.appendChild(el('h3', null, card.series.length > 1 ? 'Series' : 'Over time'));
      for (const s of card.series) sec.appendChild(sparkline(s, ctx));
      root.appendChild(sec);
    }
    return root;
  }

  /** A card panel's placeholder: `{ loading }` or `{ error, title }`. */
  function renderCardState(root, state) {
    root.textContent = '';
    const box = el('div', 'card-state');
    if (state.loading) {
      box.append(el('div', 'spinner'), el('p', 'muted', 'Opening the card'));
    } else {
      box.append(el('h2', 'card-title', state.title || 'Not found'), el('p', 'muted', state.error || ''));
    }
    root.appendChild(box);
    return root;
  }

  Object.assign(MV, { fmtNum, fmtValue, isCompact, sparkPath, renderCard, renderCardState });
  if (typeof module === 'object' && module.exports) module.exports = MV;
})();
