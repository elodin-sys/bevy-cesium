//! # Bevy-Cesium
//!
//! Stream real terrain, aerial imagery and 3D Tiles into Bevy using Cesium Native.
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
//! and the native bridge.
#![deny(unsafe_code)]

mod backend;
mod render;
pub use backend::{
    CartographicPosition, CesiumPlugin, NativeBackend, TerrainQueryError, TerrainSampler,
};
pub use render::*;
// CXX generates the FFI boundary here; application-facing code is safe Rust.
#[allow(unsafe_code)]
pub mod native;
