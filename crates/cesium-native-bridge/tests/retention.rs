//! Offline regressions against the real selector, with an intentionally zero-byte cache.
use cesium_native_bridge::{Frame, Session, Tile, View};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

fn near() -> View {
    View {
        position: [6_378_200., 5., -5.],
        direction: [-1., 0., 0.],
        up: [0., 0., 1.],
        width: 1280.,
        height: 720.,
        horizontal_fov: 1.4,
        vertical_fov: 0.9,
    }
}
fn far() -> View {
    View {
        position: [6_478_137., 5., -5.],
        ..near()
    }
}
struct Scene {
    session: Session,
    tiles: HashMap<u64, Tile>,
}
impl Scene {
    fn new(margin: f64) -> Self {
        Self::fixture(margin, "lod.json")
    }
    fn fixture(margin: f64, name: &str) -> Self {
        Self::url(
            margin,
            &format!("file://{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR")),
        )
    }
    fn url(margin: f64, url: &str) -> Self {
        let mut session = Session::open_url(url).unwrap();
        session.configure(16., 0).unwrap();
        session.configure_retention(margin).unwrap();
        Self {
            session,
            tiles: HashMap::new(),
        }
    }
    fn step(&mut self, views: &[View]) -> (Frame, usize) {
        let mut frame = self.session.update(views, 0.02).unwrap();
        assert!(!frame.load_failed);
        for tile in std::mem::take(&mut frame.loaded) {
            self.tiles.insert(tile.id, tile);
        }
        let count = frame
            .visible
            .iter()
            .map(|id| self.tiles[id].primitives.len())
            .sum();
        for id in &frame.removed {
            self.tiles.remove(id);
        }
        (frame, count)
    }
    fn wait(&mut self, views: &[View], count: usize) -> Frame {
        self.wait_with_delay(views, count, 5)
    }
    fn wait_with_delay(&mut self, views: &[View], count: usize, delay: u64) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let (f, n) = self.step(views);
            if n == count && f.selection_complete {
                return f;
            }
            std::thread::sleep(Duration::from_millis(delay));
        }
        panic!("Expected {count} primitives");
    }
}
#[test]
fn acquired_detail_survives_distance_cache_pressure_and_camera_gaps() {
    let mut scene = Scene::new(500.);
    scene.wait(&[near()], 1);
    for views in [&[far()][..], &[][..], &[far(), far()][..]] {
        for _ in 0..50 {
            let (f, n) = scene.step(views);
            assert_eq!(n, 1, "fine child must not coarsen");
            assert_eq!(
                f.retained_tiles, 1,
                "stationary history must not accumulate"
            );
        }
    }
    scene.session.configure_retention(0.).unwrap();
    assert_eq!(scene.wait(&[far()], 3).retained_tiles, 0);
}
#[test]
fn release_requires_leaving_every_camera_with_a_spatial_margin() {
    let mut scene = Scene::new(500.);
    scene.wait(&[near()], 1);
    // The terrain is just behind this camera, inside the 500 m release margin.
    let edge = View {
        position: [6_378_237., 5., -5.],
        direction: [1., 0., 0.],
        ..near()
    };
    for _ in 0..20 {
        let (f, n) = scene.step(&[edge]);
        assert_eq!((f.retained_tiles, n), (1, 1));
    }
    let away = View {
        position: [6_380_137., 5., -5.],
        ..edge
    };
    for _ in 0..20 {
        let (f, n) = scene.step(&[away, far()]);
        assert_eq!(
            (f.retained_tiles, n),
            (1, 1),
            "second camera still requires terrain"
        );
    }
    let (f, _) = scene.step(&[away]);
    assert_eq!(f.retained_tiles, 0);
    // Once released, returning from far away is allowed to select the coarse parent.
    scene.wait(&[far()], 3);
}
#[test]
fn independent_runs_select_identical_content_after_warmup() {
    let mut signatures = Vec::new();
    for delay in [0, 3, 11] {
        let mut scene = Scene::new(500.);
        let f = scene.wait_with_delay(&[near(), far()], 1, delay);
        let mut signature = Vec::new();
        for id in f.visible {
            for p in &scene.tiles[&id].primitives {
                signature.extend(p.transform.map(f64::to_bits));
                for values in [
                    &p.positions[..],
                    &p.normals,
                    &p.uvs,
                    &p.base_color,
                    &p.emissive,
                ] {
                    signature.extend(values.iter().map(|x| u64::from(x.to_bits())));
                }
                signature.extend(
                    [p.metallic, p.roughness, p.alpha_cutoff].map(|x| u64::from(x.to_bits())),
                );
                signature.extend([
                    u64::from(p.width),
                    u64::from(p.height),
                    u64::from(p.double_sided),
                    u64::from(p.unlit),
                    u64::from(p.alpha_mode),
                ]);
                signature.extend(p.indices.iter().map(|x| u64::from(*x)));
                signature.extend(p.pixels.iter().map(|x| u64::from(*x)));
            }
        }
        assert!(!signature.is_empty());
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(delay));
            assert_eq!(scene.step(&[far()]).1, 1);
        }
        signatures.push(signature);
    }
    assert!(signatures.windows(2).all(|pair| pair[0] == pair[1]));
}
#[test]
fn invalid_retention_settings_are_rejected() {
    let mut scene = Scene::new(0.);
    for bad in [-1., f64::INFINITY, f64::NAN] {
        assert!(scene.session.configure_retention(bad).is_err());
    }
}

#[test]
fn refinement_can_increase_again_without_retaining_broad_ancestors() {
    let mut scene = Scene::fixture(500., "lod-deep.json");
    let middle = scene.wait(&[near()], 1);
    assert_eq!(middle.retained_tiles, 1);
    let close = View {
        position: [6_378_140., 5., -5.],
        ..near()
    };
    let fine = scene.wait(&[close], 3);
    assert_eq!(fine.retained_tiles, 1);
    assert_ne!(middle.visible, fine.visible);
    for _ in 0..40 {
        let (frame, n) = scene.step(&[far()]);
        assert_eq!(n, 3);
        assert_eq!(
            frame.visible, fine.visible,
            "keep the grandchild, not the coarse root"
        );
        assert_eq!(frame.retained_tiles, 1);
    }
    scene.session.configure_retention(0.).unwrap();
    let root = scene.wait(&[far()], 3);
    assert_ne!(root.visible, fine.visible);
}

#[test]
fn many_retained_tiles_keep_their_detail_and_release_together() {
    // Two distant grids exercise aggregation of distinct retention volumes. The original
    // one-child fixtures cannot catch a dropped tile or growing retention set.
    let directory =
        std::env::temp_dir().join(format!("cesium-retention-grid-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(directory);
    std::fs::write(
        fixture.0.join("fine.gltf"),
        include_str!("data/triangle.gltf"),
    )
    .unwrap();
    std::fs::write(
        fixture.0.join("coarse.gltf"),
        include_str!("data/materials.gltf"),
    )
    .unwrap();
    let mut children = Vec::new();
    for x in 0..8 {
        for y in 0..8 {
            let x = (x % 4) * 100 - 150 + if x < 4 { -4000 } else { 4000 };
            let y = y * 100 - 350;
            children.push(format!(
                r#"{{
                "transform": [1,0,0,0,0,1,0,0,0,0,1,0,{x},{y},0,1],
                "boundingVolume": {{"box": [5,-5,0,20,0,0,0,20,0,0,0,10]}},
                "geometricError": 100, "content": {{"uri": "coarse.gltf"}},
                "children": [{{
                    "boundingVolume": {{"box": [5,-5,0,20,0,0,0,20,0,0,0,10]}},
                    "geometricError": 0, "content": {{"uri": "fine.gltf"}}
                }}]
            }}"#
            ));
        }
    }
    let json = format!(
        r#"{{"asset": {{"version": "1.1"}}, "geometricError": 1000000,
        "root": {{
            "boundingVolume": {{"box": [0,0,0,4500,0,0,0,400,0,0,0,10]}},
            "transform": [0,1,0,0,0,0,1,0,1,0,0,0,6378137,0,0,1],
            "geometricError": 1000000, "refine": "REPLACE", "children": [{}]
        }} }}"#,
        children.join(",")
    );
    let path = fixture.0.join("tileset.json");
    std::fs::write(&path, json).unwrap();
    let mut scene = Scene::url(500., &format!("file://{}", path.display()));
    let left = View {
        position: [6_379_137., -3995., -5.],
        ..near()
    };
    let right = View {
        position: [6_379_137., 4005., -5.],
        ..near()
    };
    let fine = scene.wait(&[left, right], 64);
    assert_eq!(fine.retained_tiles, 64);
    for views in [&[far()][..], &[][..], &[far(), far()][..]] {
        for _ in 0..100 {
            let (frame, count) = scene.step(views);
            assert_eq!(count, 64);
            assert_eq!(frame.visible, fine.visible);
            assert_eq!(frame.retained_tiles, 64);
        }
    }
    let remaining = scene.wait(&[right], 32);
    assert_eq!(remaining.retained_tiles, 32);
    assert!(remaining.visible.iter().all(|id| fine.visible.contains(id)));
    let away = View {
        direction: [1., 0., 0.],
        ..far()
    };
    let (released, _) = scene.step(&[away]);
    assert_eq!(released.retained_tiles, 0);
    // After leaving every view, the same distant camera may use coarse assets.
    let coarse = scene.wait(&[far()], 64 * 3);
    assert!(coarse.visible.iter().all(|id| !fine.visible.contains(id)));
}
