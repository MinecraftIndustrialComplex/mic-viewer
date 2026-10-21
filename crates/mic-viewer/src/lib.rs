// SPDX-License-Identifier: MPL-2.0
//! The shared world viewer of mic-rtsim and Deep Time (mic-rtsim's VIEWER.md §3.1).
//!
//! - [`http`]: a small loopback HTTP/1.1 server with a `Host` check on every request;
//! - [`sse`]: server-sent events;
//! - [`tiles`]: the [`tiles::TileSource`] trait, the tile cache and its route;
//! - [`api`]: the API's types and JSON (docs/api.md);
//! - [`scene`]: published marks grouped by region, the one reader every layer goes through;
//! - [`aggregate`]: the mark budget and binning;
//! - [`viewer`]: the routes over a project's [`viewer::Backend`];
//! - [`assets`]: the page's files.
//!
//! The core knows neither project: Deep Time and rtsim each build a binary from it and their own
//! sources.

pub mod aggregate;
pub mod api;
pub mod assets;
pub mod http;
pub mod scene;
pub mod sse;
pub mod tiles;
pub mod viewer;

/// The default port (VWD-7).
pub const DEFAULT_PORT: u16 = 8740;
