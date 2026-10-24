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
//! for setup and the remaining work. The lower-level `bevy_tiles` crate provides
//! the shared rendering contract; applications normally use this crate.
#![forbid(unsafe_code)]
use bevy_tiles::{Backend, BackendError, Frame, Options, Primitive, Tile};
#[doc(inline)]
pub use bevy_tiles::{
    CesiumCamera, CesiumConfigError, CesiumCredits, CesiumDetailViews, CesiumGeoreference,
    CesiumPrimitive, CesiumSelectionMode, CesiumSet, CesiumSource, CesiumStatus, CesiumView,
    CesiumViews, GeographicImagery,
};
/// Bevy plugin backed by Cesium Native. Use its builder methods to configure
/// detail, retention and the initial loading wait before adding it to your app.
pub type CesiumPlugin = bevy_tiles::TilesPlugin<NativeBackend>;
pub struct NativeBackend(cesium_native_bridge::Session);
impl Backend for NativeBackend {
    fn open(source: &bevy_tiles::CesiumSource, options: Options) -> Result<Self, BackendError> {
        let mut session = match source {
            bevy_tiles::CesiumSource::Url(url) => cesium_native_bridge::Session::open_url(url),
            bevy_tiles::CesiumSource::Terrain { url, imagery: None } => {
                cesium_native_bridge::Session::open_url(url)
            }
            bevy_tiles::CesiumSource::Terrain {
                imagery: Some(_), ..
            } => {
                return Err(BackendError(
                    "Raster overlays are not exposed by the Native bridge",
                ));
            }
            bevy_tiles::CesiumSource::Ion {
                asset_id,
                access_token,
            } => cesium_native_bridge::Session::open_ion(*asset_id, access_token),
        }
        .map_err(|_| BackendError("Could not open tileset"))?;
        session
            .configure(
                options.maximum_screen_error,
                options.unused_cache_bytes as u64,
            )
            .map_err(|_| BackendError("Invalid streaming options"))?;
        session
            .configure_retention(options.lod_retention_margin)
            .map_err(|_| BackendError("Invalid LOD retention margin"))?;
        Ok(Self(session))
    }
    fn update(
        &mut self,
        views: &[bevy_tiles::CesiumView],
        seconds: f32,
    ) -> Result<Frame, BackendError> {
        let views: Vec<_> = views
            .iter()
            .map(|v| cesium_native_bridge::View {
                position: v.position,
                direction: v.direction,
                up: v.up,
                width: v.width,
                height: v.height,
                horizontal_fov: v.horizontal_fov,
                vertical_fov: v.vertical_fov,
            })
            .collect();
        let f = self
            .0
            .update(&views, seconds)
            .map_err(|_| BackendError("Could not update tileset"))?;
        Ok(Frame {
            loaded: f
                .loaded
                .into_iter()
                .map(|t| Tile {
                    id: t.id,
                    primitives: t
                        .primitives
                        .into_iter()
                        .map(|p| Primitive {
                            transform: p.transform,
                            positions: p.positions,
                            normals: p.normals,
                            uvs: p.uvs,
                            indices: p.indices,
                            base_color: p.base_color,
                            pixels: p.pixels,
                            width: p.width,
                            height: p.height,
                            double_sided: p.double_sided,
                            unlit: p.unlit,
                            metallic: p.metallic,
                            roughness: p.roughness,
                            emissive: p.emissive,
                            alpha_mode: p.alpha_mode,
                            alpha_cutoff: p.alpha_cutoff,
                        })
                        .collect(),
                })
                .collect(),
            visible: f.visible,
            removed: f.removed,
            credits: f.credits,
            pending: f.pending,
            retained_tiles: f.retained_tiles as usize,
            selection_complete: f.selection_complete,
            load_failed: f.load_failed,
            error: None,
        })
    }
}
