//! Headless integration of the real camera adapter, native streamer and tile
//! placement. The fixture is local so CI needs neither network nor ion token.
use bevy::{
    asset::AssetApp,
    camera::Viewport,
    math::{DMat4, DVec3},
    prelude::*,
    transform::TransformPlugin,
};
use bevy_cesium::*;
use std::time::{Duration, Instant};

const RADIUS: f64 = 6_378_137.0;

fn local_frame(origin: DVec3) -> DMat4 {
    // Local east/up/south at latitude=0, longitude=0, with a movable origin.
    DMat4::from_cols(
        DVec3::Y.extend(0.0),
        DVec3::X.extend(0.0),
        (-DVec3::Z).extend(0.0),
        origin.extend(1.0),
    )
    .inverse()
}

fn camera_pose(world_from_ecef: DMat4, ecef: DVec3) -> Transform {
    Transform::from_translation(world_from_ecef.transform_point3(ecef).as_vec3()).looking_at(
        world_from_ecef
            .transform_point3(DVec3::new(RADIUS, 5.0, -5.0))
            .as_vec3(),
        world_from_ecef.transform_vector3(DVec3::Z).as_vec3(),
    )
}

fn assert_views(app: &App, expected: &[DVec3]) {
    let views = &app.world().resource::<CesiumViews>().0;
    assert_eq!(views.len(), expected.len());
    for position in expected {
        let view = views
            .iter()
            .find(|view| (DVec3::from_array(view.position) - position).length() < 0.001)
            .expect("Bevy camera must reach Cesium in ECEF with millimeter precision");
        let direction = (DVec3::new(RADIUS, 5.0, -5.0) - position).normalize();
        assert!((DVec3::from_array(view.direction) - direction).length() < 1e-5);
        assert_eq!((view.width, view.height), (1280.0, 720.0));
    }
}

#[test]
fn ecef_cameras_stream_and_place_tiles_across_origin_changes() {
    let mut app = App::new();
    let origin = DVec3::new(RADIUS + 0.125, -0.25, 0.375);
    let mut frame = local_frame(origin);
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .insert_resource(CesiumGeoreference(frame))
        .add_plugins(CesiumPlugin::new(CesiumSource::Url(format!(
            "file://{}/tests/data/tileset.json",
            env!("CARGO_MANIFEST_DIR")
        ))));
    let positions = [
        DVec3::new(RADIUS + 63.0, 5.0, -5.0),
        DVec3::new(RADIUS + 93.0, 8.0, -3.0),
    ];
    let cameras = positions.map(|position| {
        app.world_mut()
            .spawn((
                Camera {
                    viewport: Some(Viewport {
                        physical_size: UVec2::new(1280, 720),
                        ..default()
                    }),
                    ..default()
                },
                Projection::Perspective(PerspectiveProjection {
                    fov: 0.9,
                    ..default()
                }),
                CesiumCamera,
                camera_pose(frame, position),
            ))
            .id()
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.world().resource::<CesiumStatus>().visible_tiles == 0 {
        assert!(
            Instant::now() < deadline,
            "ECEF fixture was not selected by the real camera adapter"
        );
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_views(&app, &positions);
    let tile = app
        .world_mut()
        .query_filtered::<Entity, With<CesiumPrimitive>>()
        .single(app.world())
        .unwrap();
    let initial_mesh = app.world().get::<Mesh3d>(tile).unwrap().0.id();
    let native_pose = app
        .world()
        .get::<CesiumPrimitive>(tile)
        .unwrap()
        .ecef_from_mesh;

    for shift in [
        DVec3::ZERO,
        DVec3::new(8192.0, -4096.0, 2048.0),
        DVec3::ZERO,
    ] {
        frame = local_frame(origin + shift);
        app.world_mut().resource_mut::<CesiumGeoreference>().0 = frame;
        for (camera, position) in cameras.iter().zip(positions) {
            *app.world_mut().get_mut::<Transform>(*camera).unwrap() = camera_pose(frame, position);
        }
        // Transform propagation runs after Update. Check a settled frame.
        app.update();
        app.update();
        assert_views(&app, &positions);
        let transform = app.world().get::<Transform>(tile).unwrap();
        let reconstructed = frame.inverse() * transform.to_matrix().as_dmat4();
        for vertex in [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z] {
            assert!(
                (reconstructed.transform_point3(vertex) - native_pose.transform_point3(vertex))
                    .length()
                    < 0.001,
                "Tile geometry moved in ECEF after rebasing"
            );
        }
        assert_eq!(
            app.world().get::<Mesh3d>(tile).unwrap().0.id(),
            initial_mesh
        );
        assert_eq!(
            *app.world().get::<Visibility>(tile).unwrap(),
            Visibility::Visible
        );
        assert_eq!(app.world().resource::<CesiumStatus>().resident_tiles, 1);
    }
    app.world_mut()
        .get_mut::<Camera>(cameras[1])
        .unwrap()
        .is_active = false;
    app.update();
    assert_views(&app, &positions[..1]);
    assert_eq!(app.world().resource::<CesiumStatus>().visible_tiles, 1);
}
