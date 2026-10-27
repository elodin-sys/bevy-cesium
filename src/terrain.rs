use crate::native;
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

    /// Blocking query of up to one million positions. Fails if any height is missing.
    /// Reuse this sampler across batches to retain the tileset and downloaded tiles.
    /// The timeout applies to this call; use a fresh sampler to retry after an error.
    pub fn sample_heights(
        &mut self,
        positions: &[CartographicPosition],
        timeout: std::time::Duration,
    ) -> Result<Vec<f64>, TerrainQueryError> {
        self.0.sample_heights(positions, timeout)
    }
}
