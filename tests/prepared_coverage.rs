//! Exercise the native prepared-source adapter with real local reads and
//! controlled render-readiness feedback. No GPU or provider account is needed.
use bevy_cesium::*;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

#[test]
fn prepared_source_keeps_ancestor_until_zoomed_out_coverage_is_ready() {
    let mut backend = NativeBackend::open(
        &CesiumSource::Prepared {
            url: format!(
                "file://{}/tests/data/partial-coarse-coverage.json",
                env!("CARGO_MANIFEST_DIR")
            ),
            ion_access_token: None,
        },
        Options::default(),
    )
    .unwrap();
    assert!(matches!(&backend, NativeBackend::Prepared(_)));
    let near = CesiumView {
        position: [0., 0., 20.],
        direction: [0., 0., -1.],
        up: [0., 1., 0.],
        width: 1000.,
        height: 1000.,
        horizontal_fov: 1.,
        vertical_fov: 1.,
    };
    let mut loaded = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // Coarse child 1 has decoded, but deliberately has no prepared GPU assets.
        backend.set_render_ready(loaded.iter().copied().filter(|id| *id != 1));
        let frame = backend.update(&[near], 0.016).unwrap();
        assert!(!frame.load_failed, "{:?}", frame.error);
        loaded.extend(frame.loaded.iter().map(|tile| tile.id));
        if loaded.len() == 3 && frame.visible == [2] && frame.pending == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "prepared fixture did not load");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(loaded, HashSet::from([0, 1, 2]));
    let far = CesiumView {
        position: [0., 0., 1000.],
        ..near
    };
    for _ in 0..20 {
        let frame = backend.update(&[far], 0.016).unwrap();
        assert_eq!(frame.visible, [0]);
        assert!(!frame.selection_complete);
        assert!(frame.loaded.is_empty());
        assert!(frame.removed.is_empty());
        assert_eq!(frame.pending, 0);
        assert_eq!(backend.update(&[near], 0.016).unwrap().visible, [2]);
    }
    backend.set_render_ready(loaded);
    let frame = backend.update(&[far], 0.016).unwrap();
    assert_eq!(frame.visible, [1]);
    assert!(frame.selection_complete);
}
