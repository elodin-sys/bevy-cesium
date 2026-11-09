//! Token-free browser smoke test using local HTTP assets and the real renderer.
#[cfg(not(target_family = "wasm"))]
fn main() {
    eprintln!("Run `just install web` and `just web` for the browser fixture.");
}

#[cfg(target_family = "wasm")]
fn main() {
    browser::run();
}

#[cfg(target_family = "wasm")]
mod browser {
    use bevy::{math::DMat4, prelude::*};
    use bevy_cesium::*;

    pub fn run() {
        let location = web_sys::window().expect("browser window").location();
        let source =
            web_sys::Url::new_with_base("data/tileset.json", &location.href().expect("page URL"))
                .expect("fixture URL")
                .href();
        let ecef_from_world = DMat4::from_cols(
            bevy::math::DVec3::Y.extend(0.0),
            bevy::math::DVec3::X.extend(0.0),
            (-bevy::math::DVec3::Z).extend(0.0),
            bevy::math::DVec3::new(6_378_137.0, 0.0, 0.0).extend(1.0),
        );
        App::new()
            .add_plugins(DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy-Cesium browser fixture".into(),
                    canvas: Some("#terrain".into()),
                    fit_canvas_to_parent: true,
                    ..default()
                }),
                ..default()
            }))
            .insert_resource(ClearColor(Color::srgb(0.1, 0.15, 0.2)))
            .insert_resource(CesiumGeoreference(ecef_from_world.inverse()))
            .add_plugins(CesiumPlugin::new(CesiumSource::Url(source)).with_initial_load_wait())
            .add_systems(Startup, setup)
            .add_systems(Update, report.after(CesiumSet::Publish))
            .run();
    }

    #[derive(Component)]
    struct StatusLabel;

    fn setup(mut commands: Commands) {
        commands.spawn((
            Camera3d::default(),
            CesiumCamera,
            Transform::from_xyz(20.0, 30.0, 30.0).looking_at(Vec3::new(5.0, 0.0, 5.0), Vec3::Y),
        ));
        commands.spawn((
            DirectionalLight::default(),
            Transform::from_xyz(20.0, 50.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        commands.spawn((
            Text::new("Loading local tile fixture..."),
            TextFont {
                font_size: FontSize::Px(20.0),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                top: px(16),
                left: px(16),
                ..default()
            },
            StatusLabel,
        ));
    }

    fn report(
        status: Res<CesiumStatus>,
        mut labels: Query<&mut Text, With<StatusLabel>>,
        mut reported: Local<bool>,
    ) {
        for mut label in &mut labels {
            label.0 = if status.load_failed {
                "Terrain failed to load".into()
            } else if status.ready_for_capture() {
                "Ready: local HTTP tile rendered by Bevy".into()
            } else {
                "Loading local tile fixture...".into()
            };
        }
        if status.ready_for_capture() && !*reported {
            info!(
                "browser terrain fixture ready: {} visible tiles",
                status.visible_tiles
            );
            *reported = true;
        }
    }
}
