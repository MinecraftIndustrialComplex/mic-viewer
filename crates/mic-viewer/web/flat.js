// SPDX-License-Identifier: MPL-2.0
// Projections and tile pyramids from the world JSON (docs/api.md §2.1, §2.2): `flat` and `xwrap`
// frames, the `square` pyramid, and the `plugin` kind, whose script supplies its own projection
// (and pyramids) through a factory. A classic script: adds to `globalThis.MicViewer`.
'use strict';

(function () {
  const MV = (globalThis.MicViewer = globalThis.MicViewer || {});

  /** Pyramid kinds a plug-in registered: `kind -> (pyramidDesc, tileLayerDesc) => pyramid`. */
  const pyramidKinds = (MV.pyramidKinds = MV.pyramidKinds || {});

  /**
   * The map core's projection for a frame of `/api/world` (`flat` or `xwrap`). A `plugin` frame
   * gets a stand-in with the frame's period and z range until `loadProjection` has run its script.
   */
  function projectionFor(frame) {
    const p = (frame && frame.projection) || { kind: 'flat' };
    const period = p.kind === 'xwrap' || p.kind === 'plugin' ? Math.max(0, Number(p.period_x) || 0) : 0;
    const zr = Array.isArray(p.z_range) && p.z_range.length === 2 ? [Number(p.z_range[0]), Number(p.z_range[1])] : null;
    return { kind: p.kind || 'flat', period, zRange: zr, latLon: null, grid: null };
  }

  /** The `square` pyramid: zoom 0 is a block a pixel, `pixel(z) = 2^-z`, unbounded, signed indices. */
  function squarePyramid(desc) {
    const minZoom = desc && desc.min_zoom != null ? desc.min_zoom : -8;
    const maxZoom = desc && desc.max_zoom != null ? desc.max_zoom : 0;
    return {
      kind: 'square',
      T: 256,
      minZoom,
      maxZoom,
      pixel: z => Math.pow(2, -z),
      originX: 0,
      originZ: 0,
      cols: () => 0,
      rows: () => null,
    };
  }

  /** The pyramid of a tile layer descriptor (`square`, or a kind a plug-in registered); null if unknown. */
  function pyramidFor(tileLayer) {
    const d = (tileLayer && tileLayer.pyramid) || {};
    if (d.kind === 'square') return squarePyramid(d);
    const make = pyramidKinds[d.kind];
    return make ? make(d, tileLayer) : null;
  }

  /** A tile URL function from a descriptor's template (`tiles/x/{z}/{x}/{y}.png`). */
  function tileUrl(template) {
    return (z, x, y) => String(template).replace('{z}', z).replace('{x}', x).replace('{y}', y);
  }

  /** The value at a dotted path on `globalThis` (`"DeepTime.viewerProjection"`). */
  function lookup(path) {
    let o = globalThis;
    for (const k of String(path || '').split('.')) {
      if (!k) return undefined;
      o = o == null ? undefined : o[k];
    }
    return o;
  }

  const loaded = new Map();   // script src -> Promise

  /** Loads a same-origin script once (the page's CSP allows only 'self'). */
  function loadScript(src) {
    if (loaded.has(src)) return loaded.get(src);
    const p = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.async = true;
      s.onload = () => resolve();
      s.onerror = () => reject(new Error('could not load ' + src));
      document.head.appendChild(s);
    });
    loaded.set(src, p);
    return p;
  }

  /**
   * The projection for any frame: `flat` and `xwrap` at once; for `plugin`, loads
   * `frame.projection.script` and calls the factory it names (a dotted global path, or a name the
   * script registered in `MicViewer.plugins`) with `(args, frame, MicViewer)`. The factory returns a
   * projection (`{ period, zRange, latLon?, grid? }`) and may register pyramid kinds in
   * `MicViewer.pyramidKinds`.
   */
  async function loadProjection(frame) {
    const base = projectionFor(frame);
    const p = frame.projection || {};
    if (p.kind !== 'plugin') return base;
    if (!p.script) throw new Error('a plugin frame without a script');
    await loadScript(p.script);
    const f = (MV.plugins && MV.plugins[p.factory]) || lookup(p.factory);
    if (typeof f !== 'function') throw new Error('the plugin script has no factory ' + p.factory);
    const made = await f(p.args || {}, frame, MV);
    return Object.assign(base, made || {});
  }

  MV.plugins = MV.plugins || {};
  Object.assign(MV, { projectionFor, squarePyramid, pyramidFor, tileUrl, loadProjection });
  if (typeof module === 'object' && module.exports) module.exports = MV;
})();
