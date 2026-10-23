//! Real Cesium Native selection over a local parent/child fixture.
use bevy::{asset::AssetApp, prelude::*};
use bevy_cesium::*;
use std::time::{Duration, Instant};

fn visible_primitives(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<&Visibility, With<CesiumPrimitive>>()
        .iter(app.world())
        .filter(|v| **v == Visibility::Visible)
        .count()
}
fn wait_for(app: &mut App, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.update();
        assert!(!app.world().resource::<CesiumStatus>().load_failed);
        if visible_primitives(app) == count {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "expected {count} visible primitives, got {}",
        visible_primitives(app)
    );
}
#[test]
fn reference_view_keeps_fine_lod_when_live_camera_moves_away() {
    let near = CesiumView {
        position: [6_378_200., 5., -5.],
        direction: [-1., 0., 0.],
        up: [0., 0., 1.],
        width: 1280.,
        height: 720.,
        horizontal_fov: 1.4,
        vertical_fov: 0.9,
    };
    let far = CesiumView {
        position: [6_478_137., 5., -5.],
        ..near
    };
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .insert_resource(CesiumViews(vec![near]))
        .insert_resource(CesiumDetailViews(vec![near]))
        .add_plugins(
            CesiumPlugin::new(CesiumSource::Url(format!(
                "file://{}/crates/cesium-native-bridge/tests/data/lod.json",
                env!("CARGO_MANIFEST_DIR")
            )))
            .with_external_coordinates(),
        );
    // The parent has three primitives; the refined child has one.
    wait_for(&mut app, 1);
    app.world_mut().resource_mut::<CesiumViews>().0 = vec![far];
    for _ in 0..30 {
        app.update();
        assert_eq!(visible_primitives(&mut app), 1);
    }
    *app.world_mut().resource_mut::<CesiumSelectionMode>() = CesiumSelectionMode::Fixed;
    for _ in 0..30 {
        app.update();
        assert_eq!(visible_primitives(&mut app), 1);
    }
    // Explicit release lets Native choose the coarse parent again.
    *app.world_mut().resource_mut::<CesiumSelectionMode>() = CesiumSelectionMode::Live;
    app.world_mut()
        .resource_mut::<CesiumDetailViews>()
        .0
        .clear();
    wait_for(&mut app, 3);
}
