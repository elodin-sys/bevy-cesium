//! Optional live smoke test; ordinary CI remains independent of ion credentials.
use bevy_cesium::native::{Session, View};
use std::collections::HashSet;
use std::time::{Duration, Instant};

#[test]
#[ignore = "Requires an ion token and CESIUM_ION_IMAGERY_ASSET_ID=2"]
fn ion_terrain_receives_textured_replacements() {
    let token = std::env::var("CESIUM_ION_TOKEN")
        .ok()
        .or_else(|| {
            std::env::var("CESIUM_ION_TOKEN_FILE")
                .ok()
                .and_then(|path| std::fs::read_to_string(path).ok())
        })
        .expect("Configure an ion token for the live smoke test");
    assert_eq!(
        std::env::var("CESIUM_ION_IMAGERY_ASSET_ID").as_deref(),
        Ok("2")
    );
    let mut session = Session::open_ion(1, token.trim()).expect("Open ion terrain");
    session.configure_retention(500.0).unwrap();
    let lat = 35.350664_f64.to_radians();
    let lon = (-117.809027_f64).to_radians();
    let n = 6_378_137.0 / (1.0 - 0.00669437999014 * lat.sin().powi(2)).sqrt();
    let up = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
    let north = [-lat.sin() * lon.cos(), -lat.sin() * lon.sin(), lat.cos()];
    let origin = [
        (n + 589.274) * up[0],
        (n + 589.274) * up[1],
        (n * (1.0 - 0.00669437999014) + 589.274) * up[2],
    ];
    let direction =
        std::array::from_fn(|i| (2000.0 * north[i] - 1000.0 * up[i]) / 5_000_000_f64.sqrt());
    let view = View {
        position: std::array::from_fn(|i| origin[i] - 2000.0 * north[i] + 1000.0 * up[i]),
        direction,
        up,
        width: 1280.0,
        height: 720.0,
        horizontal_fov: 1.3,
        vertical_fov: 0.9,
    };
    let mut seen = HashSet::new();
    let mut textured_replacement = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        let frame = session.update(&[view], 0.02).expect("Update ion terrain");
        assert!(!frame.load_failed, "Terrain or imagery failed to load");
        for tile in frame.loaded {
            let replacement = !seen.insert(tile.id);
            for primitive in tile.primitives {
                if replacement && primitive.width > 0 && primitive.height > 0 {
                    assert_eq!(
                        primitive.pixels.len(),
                        primitive.width as usize * primitive.height as usize * 4
                    );
                    assert_eq!(primitive.uvs.len() * 3, primitive.positions.len() * 2);
                    assert!(primitive.uvs.iter().all(|value| value.is_finite()));
                    assert!(primitive.normals.iter().all(|value| value.is_finite()));
                    assert_eq!(primitive.metallic, 0.0);
                    textured_replacement = true;
                }
            }
        }
        if textured_replacement && frame.selection_complete && frame.retained_tiles > 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("Textured retained terrain did not complete within 120 seconds");
}
