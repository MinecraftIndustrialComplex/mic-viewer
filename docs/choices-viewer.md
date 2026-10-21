# Choices made while building the viewer

Reversible choices taken while building mic-viewer and its consumers (mic-rtsim's `rtsim-cli view`
and `run`, Deep Time's workbench), with why. The design they implement is mic-rtsim's VIEWER.md, its
proposals VWD-1 to VWD-18 approved as recommended (DECISIONS.md BR-VWD). Anything that is Ben's call
is under "For Ben" at the end, the recommended option first.

| Id | Choice | Why |
|---|---|---|
| VC1 | The page's scripts are classic scripts that add to `globalThis.MicViewer` (and to `module.exports` under node), not ES modules as VIEWER.md §3.1 sketched. | Deep Time's workbench loads its scripts as classic `<script>` tags sharing globals (`window.DTWB`); modules would force its page to change shape. Classic scripts still need no build step, and node's test runner loads them. |
| VC2 | The web files live inside the crate (`crates/mic-viewer/web/`), not at the repo root. | `include_str!` then works when a consumer vendors the crate folder alone (VC4). |
| VC3 | The crate's Cargo.toml is self-contained: no workspace inheritance. | Same reason: the folder builds on its own inside another workspace. |
| VC4 | Deep Time vendors `crates/mic-viewer` at a pinned commit (`vendor/mic-viewer/`, the commit in `vendor/mic-viewer/PINNED`, refreshed by `tools/refresh-mic-viewer.sh`) instead of a git dependency. mic-rtsim pins it by git rev, as it pins mic-native. | Deep Time's builds and tests run on its Azure VM, which cannot fetch private GitHub repos (publickey denied). See "For Ben". |
| VC5 | The square tile pyramid's zoom 0 is one block a pixel and zooms go negative outward (zoom -6 is 64 blocks a pixel); tile indices are signed and unbounded. | Vanilla worlds have no frame to fit a zoom 0 into; block-aligned tiles make every tile's blocks exact. Deep Time's 2:1 pyramid stays its plug-in's. |
| VC6 | A published view is a `Scene`: immutable, one per publication, each layer's marks indexed by 512-block region with counts. A mark whose box covers more than 64 regions is kept on a list every query checks. The parts' binary form for the live tap's pages is left to V4. | The save and host sources build scenes in process; only the live tap needs bytes on disk, and its format is best fixed with the tap. |
| VC7 | The 2,000-mark budget is shared by water filling (each layer is offered an equal part; a layer that needs less takes only that; the remainder goes to higher priority first). A shape smaller than a 32 px bin bins like a point; larger edges, paths and areas keep the largest when over their share. | No visible layer starves another, a quiet layer always shows everything, and 100,000 district boxes at a distance become honest clusters rather than the 379 largest. |
| VC8 | Layer versions are a 53-bit hash of the layer's marks, not a publication counter. | The page refetches only layers whose marks changed; an unchanged layer costs nothing per publication. 53 bits so JavaScript holds them exactly. |
| VC9 | The stream's `frame` event also carries each frame's layer versions (`frames`). | The page compares against what `/api/marks` returned for its own frame. |
| VC10 | A viewer over a save caches tiles in memory, never in the world's folder (`TileCache::in_memory`); Deep Time keeps its disk cache under the run's folder. | V1's exit test: the save folder's bytes are unchanged. |
| VC11 | The repo has a synthetic demo world (`crates/mic-viewer-demo`) served through the same routes. | The page needs a fixture with every mark kind, a wrapped frame, history and a moving clock for development, screenshots and the browser smoke test; the budget test needs 100,000 districts. |

## For Ben

1. **How Deep Time gets mic-viewer (VC4).** Recommended: keep the vendored copy (works today, the VM
   needs nothing, a script refreshes it). Alternative: add a read-only deploy key for mic-viewer to
   the Deep Time VM so Deep Time can pin it by git rev like mic-rtsim does.
