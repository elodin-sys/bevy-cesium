use bevy::{asset::AssetApp, prelude::*};
use bevy_cesium::*;
use std::time::{Duration, Instant};

#[test]
fn gltf_material_modes_reach_bevy_without_changing_defaults() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .add_plugins(
            CesiumPlugin::new(CesiumSource::Url(format!(
                "file://{}/tests/data/materials.json",
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
            let materials = app.world().resource::<Assets<StandardMaterial>>();
            assert_eq!(materials.len(), 3);
            let mut modes = [false; 3];
            for (_, mat) in materials.iter() {
                assert_eq!(mat.metallic, 1.0);
                assert_eq!(mat.perceptual_roughness, 1.0);
                assert_eq!(mat.emissive, LinearRgba::BLACK);
                match mat.alpha_mode {
                    AlphaMode::Opaque => {
                        modes[0] = true;
                        assert!(mat.unlit);
                        assert!(!mat.double_sided);
                    }
                    AlphaMode::Blend => {
                        modes[1] = true;
                        assert!(!mat.unlit);
                        assert_eq!(mat.base_color.alpha(), 0.4);
                    }
                    AlphaMode::Mask(cutoff) => {
                        modes[2] = true;
                        assert!(!mat.unlit);
                        assert_eq!(cutoff, 0.3);
                        assert!(mat.double_sided);
                    }
                    _ => panic!("Unexpected material mode"),
                }
            }
            assert_eq!(modes, [true; 3]);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Material fixture did not load");
}
