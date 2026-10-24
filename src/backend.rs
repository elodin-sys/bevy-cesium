use crate::{native, render};
pub use native::{CartographicPosition, Error as TerrainQueryError};

/// Terrain heights for simulation, sampled before the simulation starts.
/// Uses the most detailed source data, independently of the renderer's LOD.
/// Heights are meters above the WGS84 ellipsoid, not mean sea level.
pub struct TerrainSampler(native::Session);
impl TerrainSampler {
    /// Open ion terrain without downloading aerial imagery.
    pub fn from_ion(asset_id: i64, token: &str) -> Result<Self, TerrainQueryError> {
        native::Session::open_terrain_ion(asset_id, token).map(Self)
    }
    /// Blocking query with a timeout. Fails if any position has no height.
    pub fn sample_heights(
        &mut self,
        positions: &[CartographicPosition],
        timeout: std::time::Duration,
    ) -> Result<Vec<f64>, TerrainQueryError> {
        self.0.sample_heights(positions, timeout)
    }
}

use render::{Backend, BackendError, Frame, Options, Primitive, Tile};
/// Bevy plugin backed by Cesium Native. Use its builder methods to configure
/// detail, retention and the initial loading wait before adding it to your app.
pub type CesiumPlugin = render::TilesPlugin<NativeBackend>;
pub struct NativeBackend(native::Session);
impl Backend for NativeBackend {
    fn open(source: &render::CesiumSource, options: Options) -> Result<Self, BackendError> {
        let mut session = match source {
            render::CesiumSource::Url(url) => native::Session::open_url(url),
            render::CesiumSource::Terrain { url, imagery: None } => native::Session::open_url(url),
            render::CesiumSource::Terrain {
                imagery: Some(_), ..
            } => {
                return Err(BackendError(
                    "Raster overlays are not exposed by the Native bridge",
                ));
            }
            render::CesiumSource::Ion {
                asset_id,
                access_token,
            } => native::Session::open_ion(*asset_id, access_token),
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
        views: &[render::CesiumView],
        seconds: f32,
    ) -> Result<Frame, BackendError> {
        let views: Vec<_> = views
            .iter()
            .map(|v| native::View {
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
