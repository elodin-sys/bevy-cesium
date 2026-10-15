//! Narrow, owned-data boundary to Cesium Native. No native pointers escape.
//! The session stays on its creating thread; Cesium owns its worker pool.

pub use ffi::{Frame, Primitive, Tile, View};

/// A native session confined to its creating thread. No native pointers escape.
pub struct Session(cxx::UniquePtr<ffi::Session>);

/// Redacted error: native diagnostics may contain credentials or signed URLs.
#[derive(Debug)]
pub struct Error;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cesium Native operation failed")
    }
}
impl std::error::Error for Error {}
impl Session {
    pub fn open_url(url: &str) -> Result<Self, Error> {
        ffi::open_url(url).map(Self).map_err(|_| Error)
    }
    pub fn open_ion(asset_id: i64, token: &str) -> Result<Self, Error> {
        if asset_id <= 0 || token.is_empty() {
            return Err(Error);
        }
        ffi::open_ion(asset_id, token).map(Self).map_err(|_| Error)
    }
    pub fn update(&mut self, views: &[View], seconds: f32) -> Result<Frame, Error> {
        if !seconds.is_finite() || seconds < 0.0 || views.iter().any(|v| !valid_view(v)) {
            return Err(Error);
        }
        self.0.pin_mut().update(views, seconds).map_err(|_| Error)
    }
}
fn valid_view(v: &View) -> bool {
    let length = |a: [f64; 3]| a.iter().map(|x| x * x).sum::<f64>();
    let cross = [
        v.direction[1] * v.up[2] - v.direction[2] * v.up[1],
        v.direction[2] * v.up[0] - v.direction[0] * v.up[2],
        v.direction[0] * v.up[1] - v.direction[1] * v.up[0],
    ];
    v.position
        .iter()
        .chain(&v.direction)
        .chain(&v.up)
        .all(|x| x.is_finite())
        && length(v.direction).is_finite()
        && length(v.up).is_finite()
        && length(cross).is_finite()
        && length(cross) > 1e-12
        && [v.width, v.height]
            .iter()
            .all(|x| x.is_finite() && *x > 0.0)
        && [v.horizontal_fov, v.vertical_fov]
            .iter()
            .all(|x| x.is_finite() && *x > 0.0 && *x < std::f64::consts::PI)
}

#[cxx::bridge(namespace = "cesium_bridge")]
mod ffi {
    #[derive(Clone, Copy)]
    struct View {
        position: [f64; 3],
        direction: [f64; 3],
        up: [f64; 3],
        width: f64,
        height: f64,
        horizontal_fov: f64,
        vertical_fov: f64,
    }

    struct Primitive {
        transform: [f64; 16],
        positions: Vec<f32>,
        normals: Vec<f32>,
        uvs: Vec<f32>,
        indices: Vec<u32>,
        base_color: [f32; 4],
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        double_sided: bool,
        unlit: bool,
        metallic: f32,
        roughness: f32,
        emissive: [f32; 3],
        // glTF OPAQUE=0, MASK=1, BLEND=2.
        alpha_mode: u8,
        alpha_cutoff: f32,
    }

    struct Tile {
        id: u64,
        primitives: Vec<Primitive>,
    }

    struct Frame {
        loaded: Vec<Tile>,
        visible: Vec<u64>,
        removed: Vec<u64>,
        credits: Vec<String>,
        pending: u32,
        load_failed: bool,
    }

    // SAFETY: CXX checks shared layouts and ownership at build time. Session
    // owns all native state; methods return copies, never borrowed native data.
    // No Send/Sync implementation: creation, update and destruction stay on
    // the main thread. C++ exceptions become Rust errors through Result.
    unsafe extern "C++" {
        include!("cesium_bridge/bridge.h");
        type Session;
        fn open_url(url: &str) -> Result<UniquePtr<Session>>;
        fn open_ion(asset_id: i64, token: &str) -> Result<UniquePtr<Session>>;
        fn update(self: Pin<&mut Session>, views: &[View], seconds: f32) -> Result<Frame>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_camera_geometry() {
        let mut view = View {
            position: [6_378_137.0, 0.0, 0.0],
            direction: [-1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            width: 1280.0,
            height: 720.0,
            horizontal_fov: 1.4,
            vertical_fov: 0.9,
        };
        assert!(valid_view(&view));
        view.up = view.direction;
        assert!(!valid_view(&view));
        view.up = [0.0, 0.0, 1.0];
        view.position[0] = f64::NAN;
        assert!(!valid_view(&view));
        view.position[0] = 0.0;
        view.vertical_fov = 0.0;
        assert!(!valid_view(&view));
    }
    #[test]
    fn errors_do_not_expose_native_diagnostics() {
        assert_eq!(Error.to_string(), "Cesium Native operation failed");
        assert!(Session::open_ion(-1, "not-a-real-token").is_err());
    }
}
