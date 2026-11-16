//! Minimal native/WASM Mars fixture; supply a real body-fixed source explicitly.
use bevy::{math::DMat4, prelude::*};
use bevy_cesium::*;

fn main() {
    let source = source_url();
    // At longitude=latitude=height=0, local axes are east/up/south.
    // A different dataset origin requires a matching body-fixed transform.
    let body_from_world = DMat4::from_cols(
        bevy::math::DVec3::Y.extend(0.),
        bevy::math::DVec3::X.extend(0.),
        (-bevy::math::DVec3::Z).extend(0.),
        Ellipsoid::MARS.cartesian(0., 0., 0.).extend(1.),
    );
    let mut window = Window {
        title: "Mars tile fixture / Bevy-Cesium".into(),
        ..default()
    };
    #[cfg(target_family = "wasm")]
    {
        window.canvas = Some("#terrain".into());
        window.fit_canvas_to_parent = true;
    }
    // Keep the same variable on both targets without a native-only lint.
    window.resolution = (1100, 720).into();
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.12, 0.08, 0.06)))
        .insert_resource(CesiumGeoreference(body_from_world.inverse()))
        .add_plugins(
            CesiumPlugin::new(CesiumSource::Url(source))
                .with_ellipsoid(Ellipsoid::MARS)
                .with_initial_load_wait(),
        )
        .add_systems(Startup, setup)
        .add_systems(Update, report.after(CesiumSet::Publish))
        .run();
}

#[cfg(not(target_family = "wasm"))]
fn source_url() -> String {
    std::env::var("CESIUM_TILESET_URL").unwrap_or_else(|_| {
        let path = std::env::current_dir()
            .expect("repository root")
            .join("tests/data/mars.json");
        format!("file://{}", path.display())
    })
}

#[cfg(target_family = "wasm")]
fn source_url() -> String {
    let location = web_sys::window().expect("browser window").location();
    web_sys::Url::new_with_base("data/mars.json", &location.href().expect("page URL"))
        .expect("fixture URL")
        .href()
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        CesiumCamera,
        Transform::from_xyz(20., 30., 30.).looking_at(Vec3::new(5., 0., 5.), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(20., 50., 20.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Text::new("Loading synthetic Mars tile fixture…"),
        TextFont {
            font_size: FontSize::Px(20.),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            left: px(16),
            ..default()
        },
    ));
}

fn report(status: Res<CesiumStatus>, mut labels: Query<&mut Text>) {
    for mut label in &mut labels {
        label.0 = if status.load_failed {
            "Mars fixture failed to load".into()
        } else if status.ready_for_capture() {
            "Mars body-fixed fixture / synthetic geometry, not measured terrain".into()
        } else {
            "Loading synthetic Mars tile fixture…".into()
        };
    }
}
