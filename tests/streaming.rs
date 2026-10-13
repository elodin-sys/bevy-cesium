use bevy::{asset::AssetApp, prelude::*};
use bevy_cesium::*;
use std::time::{Duration, Instant};

#[test]
fn standalone_bevy_streams_a_fixture_without_elodin_or_gpu() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .add_plugins(
            CesiumPlugin::new(CesiumSource::Url(format!(
                "file://{}/crates/cesium-native-bridge/tests/data/tileset.json",
                env!("CARGO_MANIFEST_DIR")
            )))
            .with_external_coordinates(),
        );
    app.world_mut()
        .resource_mut::<CesiumViews>()
        .0
        .push(CesiumView {
            position: [6_378_200.0, 5.0, -5.0],
            direction: [-1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            width: 1280.0,
            height: 720.0,
            horizontal_fov: 1.4,
            vertical_fov: 0.9,
        });
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        app.update();
        if app.world().resource::<CesiumStatus>().visible_tiles > 0 {
            let world = app.world_mut();
            let mut query = world.query::<(
                &CesiumPrimitive,
                &Mesh3d,
                &MeshMaterial3d<StandardMaterial>,
                &Visibility,
            )>();
            let items: Vec<_> = query.iter(world).collect();
            assert_eq!(items.len(), 1);
            let (pose, mesh, material, visibility) = items[0];
            assert!((pose.ecef_from_mesh.w_axis.x - 6_378_137.0).abs() < 1e-6);
            assert_eq!(*visibility, Visibility::Visible);
            assert!(world.resource::<Assets<Mesh>>().get(&mesh.0).is_some());
            let mat = world
                .resource::<Assets<StandardMaterial>>()
                .get(&material.0)
                .unwrap();
            assert!(
                world
                    .resource::<Assets<Image>>()
                    .get(mat.base_color_texture.as_ref().unwrap())
                    .is_some()
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Standalone plugin did not display local fixture");
}
