use crate::CesiumSource;
/// Backend errors contain static descriptions, never credentials or source URLs.
#[derive(Debug, Clone, Copy)]
pub struct BackendError(pub &'static str);
impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for BackendError {}
#[derive(Clone, Copy)]
pub struct Options {
    /// Evict unused tiles when over budget; active tiles and GPU allocations are not capped.
    pub unused_cache_bytes: usize,
    /// Refinement threshold in physical screen pixels.
    pub maximum_screen_error: f64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            unused_cache_bytes: 256 * 1024 * 1024,
            maximum_screen_error: 16.0,
        }
    }
}
/// Owned data only. Backends never modify application cameras, lights or exposure.
pub trait Backend: Sized + 'static {
    fn open(source: &CesiumSource, options: Options) -> Result<Self, BackendError>;
    fn update(&mut self, views: &[CesiumView], seconds: f32) -> Result<Frame, BackendError>;
}
#[derive(Clone, Copy, Debug)]
pub struct CesiumView {
    pub position: [f64; 3],
    pub direction: [f64; 3],
    pub up: [f64; 3],
    pub width: f64,
    pub height: f64,
    pub horizontal_fov: f64,
    pub vertical_fov: f64,
}
#[derive(Debug)]
pub struct Primitive {
    pub transform: [f64; 16],
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
    pub base_color: [f32; 4],
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub double_sided: bool,
    pub unlit: bool,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub alpha_mode: u8,
    pub alpha_cutoff: f32,
}
#[derive(Debug)]
pub struct Tile {
    pub id: u64,
    pub primitives: Vec<Primitive>,
}
#[derive(Default)]
pub struct Frame {
    pub loaded: Vec<Tile>,
    pub visible: Vec<u64>,
    pub removed: Vec<u64>,
    pub credits: Vec<String>,
    pub pending: u32,
    pub load_failed: bool,
    pub error: Option<&'static str>,
}
