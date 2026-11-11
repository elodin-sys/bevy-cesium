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
//! Set a timeout in your app. On native targets, enable
//! `CesiumPlugin::with_lod_retention_margin` to keep acquired terrain detail until
//! it leaves all camera views plus a margin. Browser builds require a zero margin.
//!
//! # Platforms
//!
//! The backend is selected by target, without Cargo features. Browser builds use
//! HTTP(S) sources and do not compile the C++ bridge or link a Cesium Native SDK.
//! They support explicit 3D Tiles and EPSG:4326/TMS quantized-mesh terrain with
//! static availability and matching imagery. [`CesiumSource::IonTerrain`] and
//! terrain height sampling are native-only. See the README for format limits.
//!
//! [Native API](https://elodin-sys.github.io/bevy-cesium/bevy_cesium/)
//! · [Browser API](https://elodin-sys.github.io/bevy-cesium/wasm/bevy_cesium/)
//!
//! For controlled captures, supply [`CesiumDetailViews`] and select
//! [`CesiumSelectionMode::Fixed`]. Repeatable pixels also need fixed terrain data
//! and a controlled rendering environment.
//!
//! See the [README and examples](https://github.com/elodin-sys/bevy-cesium)
//! for setup and supported sources. One crate contains the plugin, renderer
//! and platform adapters. The browser backend uses the internal `tiles-core` crate.
#![deny(unsafe_code)]

#[cfg(not(target_family = "wasm"))]
mod backend;
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
