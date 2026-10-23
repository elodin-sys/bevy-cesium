//! Offline render regression scene. No Cesium service or credentials required.
//! A tiny upload budget forces assets to become ready across multiple frames.
use bevy::{
    camera::Viewport,
    math::DMat4,
    prelude::*,
    render::{
        render_asset::RenderAssetBytesPerFrame,
        view::screenshot::{Screenshot, save_to_disk},
    },
};
use bevy_tiles::*;

struct Fixture(u64);
impl Backend for Fixture {
    fn open(_: &CesiumSource, _: Options) -> Result<Self, BackendError> {
        Ok(Self(0))
    }
    fn update(&mut self, _: &[CesiumView], _: f32) -> Result<Frame, BackendError> {
        let tick = self.0;
        self.0 += 1;
        let detailed = (tick / 240) % 2 == 1;
        let mut frame = Frame {
            visible: if detailed { vec![2, 3] } else { vec![1] },
            selection_complete: tick >= 60,
            ..default()
        };
        if tick.is_multiple_of(240) {
            if detailed {
                frame.loaded = vec![
                    tile(2, -10., 0., [50, 180, 90, 255]),
                    tile(3, 0., 10., [50, 180, 90, 255]),
                ];
                frame.removed = vec![1];
            } else {
                frame.loaded = vec![tile(1, -10., 10., [60, 130, 220, 255])];
                frame.removed = vec![2, 3];
            }
        }
        // Same-ID imagery update: the old material must remain visible.
        if tick % 240 == 120 {
            frame.loaded = if detailed {
                vec![
                    tile(2, -10., 0., [220, 180, 60, 255]),
                    tile(3, 0., 10., [220, 180, 60, 255]),
                ]
            } else {
                vec![tile(1, -10., 10., [180, 90, 200, 255])]
            };
        }
        Ok(frame)
    }
}
fn tile(id: u64, left: f32, right: f32, color: [u8; 4]) -> Tile {
    Tile {
        id,
        primitives: vec![Primitive {
            transform: DMat4::IDENTITY.to_cols_array(),
            positions: vec![
                left, 0., -10., right, 0., -10., right, 0., 10., left, 0., 10.,
            ],
            normals: vec![0., 1., 0., 0., 1., 0., 0., 1., 0., 0., 1., 0.],
            uvs: vec![0., 0., 1., 0., 1., 1., 0., 1.],
            indices: vec![0, 2, 1, 0, 3, 2],
            base_color: [1.; 4],
            pixels: color.repeat(256 * 256),
            width: 256,
            height: 256,
            double_sided: true,
            unlit: true,
            metallic: 0.,
            roughness: 1.,
            emissive: [0.; 3],
            alpha_mode: 0,
            alpha_cutoff: 0.5,
        }],
    }
}
fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Terrain transitions — two ordered cameras".into(),
                resolution:
                    bevy::window::WindowResolution::new(960, 480).with_scale_factor_override(1.0),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(1., 0., 1.)))
        .insert_resource(RenderAssetBytesPerFrame::new(1024))
        .add_plugins(
            TilesPlugin::<Fixture>::new(CesiumSource::Url("fixture".into()))
                .with_initial_load_wait(),
        )
        .add_systems(Startup, setup)
        .add_systems(Update, inspect.after(CesiumSet::Publish))
        .run();
}
fn setup(mut commands: Commands) {
    for index in 0..2 {
        commands.spawn((
            Camera3d::default(),
            Camera {
                order: index as isize,
                viewport: Some(Viewport {
                    physical_position: UVec2::new(index * 480, 0),
                    physical_size: UVec2::splat(480),
                    ..default()
                }),
                clear_color: if index == 0 {
                    ClearColorConfig::Default
                } else {
                    ClearColorConfig::None
                },
                ..default()
            },
            CesiumCamera,
            Transform::from_xyz(0., 20., if index == 0 { 0.01 } else { 8. })
                .looking_at(Vec3::ZERO, Vec3::Z),
        ));
    }
}
fn inspect(mut commands: Commands, mut frame: Local<u64>, status: Res<CesiumStatus>) {
    *frame += 1;
    if (*frame).is_multiple_of(120) {
        info!(
            "frame={} visible={} preparing={}",
            *frame, status.visible_tiles, status.pending_render_tiles
        );
    }
    if let Ok(directory) = std::env::var("CESIUM_STABILITY_OUTPUT") {
        if [
            30, 59, 65, 110, 121, 122, 123, 130, 239, 241, 242, 243, 250, 370, 481, 482, 490,
        ]
        .contains(&*frame)
        {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(format!("{directory}/frame-{:04}.png", *frame)));
        }
        if *frame == 510 {
            commands.write_message(AppExit::Success);
        }
    }
}
