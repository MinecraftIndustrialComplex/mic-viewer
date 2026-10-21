# mic-viewer API and page contract

The contract between the page (`crates/mic-viewer/web/`), the Rust core (`crates/mic-viewer`) and the
projects that serve it (mic-rtsim's `rtsim-cli view` and `run`, Deep Time's `deeptime watch`). The
design is mic-rtsim's VIEWER.md; this file is what the code implements. Every route is a GET (other
methods answer 405) and nothing a page sends changes a simulation.

Units: positions are **blocks** (x east, z south, as Minecraft). Time is a **game tick** (`t`, an
integer; 24,000 ticks a game day). Colours are CSS hex strings.

## 1. Routes

| Route | Answer |
|---|---|
| `/` | the viewer page (`viewer.html`) |
| `/<file>.js`, `/viewer.css` | the page's scripts and style (`assets::file`) |
| `/api/world` | §2 |
| `/api/layers` | §3 |
| `/api/marks?frame&x0&z0&x1&z1&bpp&layers&t` | §4 |
| `/api/thing/<layer>/<id>?t` | §5 |
| `/api/events?frame&from&to&x0&z0&x1&z1&kinds&limit` | §6 |
| `/api/events/histogram?frame&from&to&buckets&kinds` | §6 |
| `/api/stream` | §7, server-sent events |
| `/tiles/<layer>/<z>/<x>/<y>.png` | base map tiles (§2.2); `<layer>` is a tile layer's `id`, indices signed |

Query numbers are decimal; `layers` and `kinds` are comma-separated ids. A missing `t` means now.

## 2. `/api/world`

```json
{
  "name": "MICTest",
  "source": "save",
  "source_note": "pages of generation 41, saved at game day 12 06:00",
  "build": "mic-rtsim 0123abc",
  "frames": [
    {
      "id": "minecraft:overworld", "label": "Overworld",
      "projection": { "kind": "flat", "period_x": 0, "z_range": null },
      "bounds": [-2048, -2048, 2048, 2048],
      "tiles": [
        { "id": "habitat", "label": "Habitat", "url": "tiles/habitat/{z}/{x}/{y}.png",
          "pyramid": { "kind": "square", "min_zoom": -8, "max_zoom": 0 },
          "bounds": [-2048, -2048, 2048, 2048], "overlay": false, "attribution": "rtsim HABITAT, 64-block cells" }
      ]
    }
  ],
  "time": { "tick": 288000, "ticks_per_day": 24000, "wall_ms": 1792560000000, "tps": 20.0, "running": false },
  "freshness": { "age_ms": 0, "cycle_ms": null, "note": "a save: nothing moves" },
  "budget": 2000,
  "event_kinds": [ { "id": "conflict.raid", "label": "Raid", "group": "conflict", "color": "#e5484d", "severity": 2 } ],
  "has_events": true,
  "stream": true
}
```

- `source`: `save`, `live`, `host` or `demo`. The page shows it as a badge.
- `frames`: one per dimension. `projection.kind` is `flat` (no wrap), `xwrap` (east and west wrap
  with `period_x` blocks; x runs over `[-period_x/2, period_x/2)`), or `plugin` (a projection
  supplied by a plug-in script: `{"kind": "plugin", "script": "plugins/deeptime/projection.js",
  "factory": "...", "args": {...}}`, for Deep Time worlds). `z_range` is `[zmin, zmax]` or null.
- `bounds` `[x0, z0, x1, z1]`: where the frame has content; "home" fits it.
- `time.running`: whether ticks advance (live, host); the page then extrapolates `tick` at `tps`.

### 2.1 Projections in the page

The map core takes a projection object:

```js
{ period: 0 | <blocks>,          // east-west wrap period; 0 for none
  zRange: null | [zmin, zmax],    // the frame's north and south limits, if any
  latLon: null | (x, z) => [lat, lon],   // for the readout (Deep Time)
  grid: null | (ctx, view, map) => {} }  // an optional graticule painter
```

`flat.js` builds the `flat` and `xwrap` ones from the world JSON (`MicViewer.projectionFor(frame)`).

### 2.2 Tile pyramids

A pyramid object says where tile `(z, x, y)` lies:

```js
{ T: 256,                         // tile edge, px
  minZoom, maxZoom,
  pixel: z => <blocks per tile pixel at zoom z>,
  originX, originZ,               // the block corner of tile (x = 0, y = 0), the same at every zoom
  cols: z => 0 | <tiles across, wrapping>,     // 0: unbounded, no wrap
  rows: z => null | [ymin, ymax] }             // null: unbounded
```

Tile `(z, x, y)` covers blocks `originX + x*S .. originX + (x+1)*S` east and `originZ + y*S ..` south,
with `S = T * pixel(z)`. The ancestor of `(z, x, y)` is `(z-1, x >> 1, y >> 1)`.

- `square` (`MicViewer.squarePyramid({min_zoom, max_zoom})`): `pixel(z) = 2^-z` (zoom 0 is one block a
  pixel, zoom -6 is 64 blocks a pixel), origin `(0, 0)`, unbounded, indices may be negative.
- Deep Time's 2:1 pyramid is its plug-in's (zoom 0 is two tiles wide, the frame starts at
  `(-C/2, -C/4)`, `cols(z) = 2 << z`, `rows(z) = [0, 2^z - 1]`).

A tile request outside `bounds` is never made. A tile with nothing in it answers 204.

## 3. `/api/layers`

```json
{
  "groups": [ { "id": "settlements", "label": "Settlements" } ],
  "layers": [
    {
      "id": "settle.villages", "group": "settlements", "label": "Villages",
      "description": "Villages and player bases, sized by population",
      "kinds": ["point"], "priority": 10, "default_on": true,
      "min_bpp": 0, "max_bpp": 1e9,
      "style": { "color": "#e0b04a", "glyph": "circle", "size": 6, "width": 2 },
      "legend": [ { "label": "Human", "color": "#e0b04a", "glyph": "circle" } ],
      "has_cards": true, "has_past": false, "animated": false
    }
  ]
}
```

- `min_bpp`, `max_bpp`: the zoom range (blocks per CSS pixel) where the layer is asked for. Outside
  it the layer is greyed in the panel and not requested.
- `glyph`: `circle`, `square`, `diamond`, `triangle`, `ring`, `cross`, `star`.
- `legend` entries are addressed by index from marks (`ci`).
- `priority`: higher draws on top and gets its share of the budget first.

## 4. `/api/marks`

```json
{
  "t": 288000, "frame": "minecraft:overworld",
  "layers": {
    "settle.villages": { "version": 7, "total": 4096, "shown": 812, "clustered": false, "marks": [] },
    "travel.caravans": { "version": 3, "total": 18000, "shown": 300, "clustered": true, "marks": [] }
  }
}
```

The server returns at most `budget` marks in all (2,000), shared among the asked layers by priority.
A layer with more marks in view than its share is binned on a 32 px screen grid and answered with
`cluster` marks (`clustered: true`). Marks (short keys keep the answers small):

| `k` | Fields | Drawn as |
|---|---|---|
| `point` | `id`, `x`, `z`, `ci?`, `c?`, `g?`, `s?`, `l?` (label), `ring?` | a glyph |
| `leg` | `id`, `p: [[x0,z0,x1,z1], ...]` pieces, `t0`, `t1`, `ci?`, `c?`, `g?`, `l?` | a mover at its place on the leg at time t, its track faint |
| `path` | `id?`, `p: [[x,z], ...]`, `ci?`, `c?`, `w?` | a polyline |
| `edge` | `id?`, `a: [x,z]`, `b: [x,z]`, `w?` (CSS px), `ci?`, `c?` | a line |
| `area` | `id?`, `x`, `z`, `r` or `box: [x0,z0,x1,z1]`, `ci?`, `c?`, `l?` | a translucent disc or box |
| `cell` | `x0`, `z0`, `size`, `w`, `h`, `v: [...]` (row-major, null for none), `ramp?`, `lo?`, `hi?` | a grid of coloured squares |
| `cluster` | `x`, `z`, `n`, `by?: {"<legend index>": n}` | a bubble with the count |
| `label` | `x`, `z`, `text` | text |

- `ci` is an index into the layer's `legend`; `c` an explicit colour; neither: the layer's style.
- `s` multiplies the style's size (population, head count).
- Leg pieces are already cut at seams; the page never joins pieces. Between `t0` and `t1` the mover
  is at the matching fraction of the pieces' total length; before `t0` at the start, after `t1` at
  the end.
- `id` is the layer-local id passed to `/api/thing/<layer>/<id>`.

## 5. `/api/thing/<layer>/<id>?t`

```json
{
  "layer": "travel.caravans", "id": "12345",
  "title": "Caravan 12345", "subtitle": "outbound to Ashford",
  "at": [1204, -330],
  "as_of": null, "note": null,
  "fields": [ { "label": "Home", "value": "Oakvale", "unit": null, "link": { "layer": "settle.villages", "id": "17" } } ],
  "sections": [ { "title": "Cargo", "fields": [ { "label": "grain", "value": 64, "unit": "items" } ] } ],
  "series": [ { "label": "Price index", "unit": "", "points": [[288000, 1.04]] } ]
}
```

404 when the thing is gone. `value` is a string or a number (the page formats numbers). `as_of`
(a tick) and `note` say when a card is older than now ("as of game day 4").

## 6. Events

`/api/events` returns archive rows in a time window (and, with `x0..z1`, a box):

```json
{
  "rows": [ { "t": 280000, "kind": "conflict.raid", "sev": 2, "x": 1100, "z": -400,
              "subject": "88", "object": "17", "mag": 12, "text": "Goblins raid North Smelter" } ],
  "legs": [ { "t0": 279000, "t1": 281000, "kind": "conflict.party", "subject": "88",
              "p": [[900, -600, 1100, -400]] } ],
  "truncated": false
}
```

`/api/events/histogram` returns counts per bucket per kind:

```json
{ "from": 0, "to": 288000, "buckets": 120,
  "series": { "conflict.raid": [0, 0, 3] },
  "wall": [[0, 1792000000000], [288000, 1792560000000]] }
```

`wall` pairs ticks with wall-clock milliseconds (the page writes dates under game days). Kinds are
described once in `/api/world` `event_kinds`.

## 7. `/api/stream`

Server-sent events, `retry: 2000`, a comment every 15 s:

- `hello`: the `/api/world` answer.
- `frame`: `{"t": <tick>, "time": {...}, "freshness": {...}, "versions": {"<layer>": <version>},
  "frames": {"<frame>": {"<layer>": <version>}}}` when a new publication arrives. `frames` holds the
  versions `/api/marks` answers with; the page refetches marks only for layers of its frame whose
  version changed. (`versions` is their xor over the frames.)
- `events`: `{"rows": [...]}` archive rows newer than the stream's start.

A page hidden for 60 s closes its stream and reopens it when shown (open streams are the live
source's lease, VIEWER.md §11).

## 8. Security (VIEWER.md §13)

- Binds loopback by default. Tailnet addresses (100.64.0.0/10, fd7a:115c:a1e0::/48) only with
  `allow_tailnet`; any other address is refused at start.
- Every request's `Host` must name `localhost`, `127.0.0.1` or `[::1]` (any port, so `ssh -L
  9000:127.0.0.1:8740` works); tailnet names (a bare machine name, `*.ts.net`, a tailnet IP) only with
  `allow_tailnet`. Anything else is 403 (no DNS rebinding).
- Peers must be loopback (or tailnet with the flag).
- The viewer page carries `Content-Security-Policy: default-src 'self'; img-src 'self' data: blob:;
  style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'`.
  The page therefore uses no inline scripts, no inline `<style>` and no `style=""` attributes; styles
  set from scripts go through the CSSOM (`el.style.width = ...`), which the policy allows.
- `X-Content-Type-Options: nosniff`, no CORS headers.
