# Notices

mic-viewer is licensed MPL-2.0 (LICENSE). This file lists what came from elsewhere.

## Adapted from Deep Time

The code below was adapted from Deep Time (LGPL-3.0-or-later) and relicensed MPL-2.0 by its
copyright holder. Ben owns both repositories; he chose a shared MPL-2.0 viewer repo that Deep Time's
map core moves into (mic-rtsim DECISIONS.md VW3, BR-VWD, VIEWER.md §3.1). It is Deep Time's own
code: Deep Time's NOTICE.md lists no third-party source for any of it (checked at the commit below).

Deep Time commit adapted from: `30c4d6649b6f8fe1d2fc2bd4cc93d28fe61f3a39`
(MinecraftIndustrialComplex/deep-time, master).

| Deep Time file | mic-viewer file | Changes |
|---|---|---|
| `crates/deeptime-cli/src/watch/http.rs` (the server: accept loop, thread per connection, request reader, `respond`, keep-alive, the tailnet ranges and names, the POST guard) | `crates/mic-viewer/src/http.rs` | routes are a `Handler` trait; the `Host` check runs on every request, not only POSTs; loopback only unless `allow_tailnet`; a Unix socket listener; a CSP per answer; a connection cap |
| the same file's `stream_events` (`retry: 2000`, a comment every 15 s) | `crates/mic-viewer/src/sse.rs` and the stream route in `viewer.rs` | a writer the route drives; named events |
| `crates/deeptime-cli/src/watch/tiles.rs` (`TileCache`: versioned folder, least-recently-asked eviction to nine tenths, written aside and renamed in; the tile path parser and its tests) | `crates/mic-viewer/src/tiles.rs` | a `TileSource` trait instead of `BakeMap`; signed indices; a memory cache beside the disk one; only `v<N>` folders are swept |
| `crates/deeptime-viewer/src/lib.rs` (`encode_png`, `encode_png_rgb`) | `crates/mic-viewer/src/tiles.rs` | one function under both |
| `crates/deeptime-viewer/assets/worldmap.js` (the slippy map: view state, tile LRU and flight cap, ancestor fallback, zoom pick, pan, wheel, pinch, keys, the scale bar) | `crates/mic-viewer/web/map.js` | projection, pyramid, tiles and overlays are the caller's; no workbench globals |
| `tools/viewer/shot.py` (the DevTools-protocol client) | `tests/web/shot.py` | the viewer's pages and checks |
| `flake.lock`, `rust-toolchain.toml` (via mic-rtsim), `deny.toml` (via mic-native) | same paths | descriptions |

Everything not listed here is new code, MPL-2.0.
