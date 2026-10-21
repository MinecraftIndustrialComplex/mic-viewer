// SPDX-License-Identifier: MPL-2.0
//! Create in a Minecraft save, for the viewer: the rail network (`create_tracks.dat`), the block
//! entities of factories and logistics (region files, read where asked), and both as map layers.
//!
//! Read only: files are read whole into memory and closed; nothing is written.

pub mod blocks;
pub mod draw;
pub mod rail;
pub mod tags;
