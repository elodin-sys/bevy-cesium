//! Standalone viewer: no Elodin, database, Python SDK or editor UI.
use bevy::{
    math::{DMat4, DVec3},
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};
use bevy_cesium::*;

fn main() {
    let source = CesiumSource::from_env()
        .expect("Invalid Cesium configuration")
        .unwrap_or_else(|| {
            CesiumSource::Url(
                "https://pelican-public.s3.amazonaws.com/3dtiles/agi-hq/tileset.json".into(),
            )
        });
    let lat = number("CESIUM_LAT", 40.03879587543084).to_radians();
    let lon = number("CESIUM_LON", -75.59670696255716).to_radians();
    let alt = number("CESIUM_ALT", 297.25783960334957);
    let radius = 6_378_137.0 / (1.0 - 0.00669437999014 * lat.sin().powi(2)).sqrt();
    let origin = DVec3::new(
        (radius + alt) * lat.cos() * lon.cos(),
        (radius + alt) * lat.cos() * lon.sin(),
        (radius * (1.0 - 0.00669437999014) + alt) * lat.sin(),
    );
    let east = DVec3::new(-lon.sin(), lon.cos(), 0.0);
    let up = DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin());
    let north = up.cross(east);
    let ecef_from_world = DMat4::from_cols(
        east.extend(0.0),
        up.extend(0.0),
        (-north).extend(0.0),
        origin.extend(1.0),
    );
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Cesium for Bevy — standalone".into(),
                resolution: (1280, 800).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.42, 0.63, 0.82)))
        .insert_resource(CesiumGeoreference(ecef_from_world.inverse()))
        .add_plugins(CesiumPlugin::new(source))
        .add_systems(Startup, setup)
        .add_systems(Update, (orbit, credits, screenshot))
        .run();
}
fn number(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .map(|v| v.parse().expect("Invalid coordinate"))
        .unwrap_or(default)
}
#[derive(Component)]
struct Attribution;
fn setup(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        CesiumCamera,
        Transform::from_xyz(130.0, 100.0, 160.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(80.0, 150.0, 60.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Text::new("AGI headquarters sample | Cesium Native | Bevy"),
        Attribution,
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            bottom: px(12),
            left: px(12),
            ..default()
        },
    ));
}
fn orbit(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut angle: Local<f32>,
    mut cameras: Query<&mut Transform, With<CesiumCamera>>,
) {
    if !keys.pressed(KeyCode::Space) {
        *angle += time.delta_secs() * 0.06;
    }
    for mut camera in &mut cameras {
        *camera = Transform::from_xyz(180.0 * angle.cos(), 110.0, 180.0 * angle.sin())
            .looking_at(Vec3::ZERO, Vec3::Y);
    }
}
fn credits(
    credits: Res<CesiumCredits>,
    status: Res<CesiumStatus>,
    mut labels: Query<&mut Text, With<Attribution>>,
) {
    for mut text in &mut labels {
        text.0 = format!(
            "AGI headquarters sample | Cesium Native | Bevy | {} visible tiles{}\n{}",
            status.visible_tiles,
            if status.load_failed {
                " | load failed"
            } else {
                ""
            },
            credits.html.join("\n")
        );
    }
}
fn screenshot(
    mut commands: Commands,
    time: Res<Time>,
    mut done: Local<bool>,
    status: Res<CesiumStatus>,
) {
    if !*done
        && time.elapsed_secs() > 15.0
        && status.visible_tiles > 0
        && let Ok(path) = std::env::var("CESIUM_SCREENSHOT")
    {
        *done = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}
