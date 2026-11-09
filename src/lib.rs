//! # Bevy-Cesium
//!
//! Stream real terrain, aerial imagery and 3D Tiles into Bevy.
//! Native targets use Cesium Native; browser targets use the Rust tile loader.
//! Both backends use the same scene boundary, renderer and readiness tracking.
//!
//! Add [`CesiumPlugin`], mark your cameras with [`CesiumCamera`], and set
//! [`CesiumGeoreference`] to place Earth coordinates in your scene. Your app
//! supplies lighting and displays the attribution from [`CesiumCredits`].
//!
//! For stable startup, enable `CesiumPlugin::with_initial_load_wait` and show a
//! loading message until [`CesiumStatus::ready_for_capture`] reports readiness.
//! Set a timeout in your app. Enable `CesiumPlugin::with_lod_retention_margin`
//! to keep acquired terrain detail until it leaves all camera views plus a margin.
//!
//! For controlled captures, supply [`CesiumDetailViews`] and select
//! [`CesiumSelectionMode::Fixed`]. Repeatable pixels also need fixed terrain data
//! and a controlled rendering environment.
//!
//! See the [README and examples](https://github.com/elodin-sys/bevy-cesium)
//! for setup and the remaining work. One crate contains the plugin, renderer
//! and platform adapters. The browser backend uses the internal `tiles-core` crate.
#![deny(unsafe_code)]

#[cfg(not(target_family = "wasm"))]
mod backend;
#[cfg(any(target_family = "wasm", test))]
mod browser;
mod render;
#[cfg(not(target_family = "wasm"))]
mod terrain;
#[cfg(not(target_family = "wasm"))]
pub use backend::{CesiumPlugin, NativeBackend};
#[cfg(target_family = "wasm")]
pub use browser::{BrowserBackend, CesiumPlugin};
pub use render::*;
#[cfg(not(target_family = "wasm"))]
pub use terrain::{CartographicPosition, TerrainQueryError, TerrainSampler};
// CXX generates the FFI boundary here; application-facing code is safe Rust.
#[allow(unsafe_code)]
#[cfg(not(target_family = "wasm"))]
pub mod native;
