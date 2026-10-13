//! Cesium Native streaming and Bevy rendering, independent of any application.
//! The default coordinate adapter uses a double-precision ECEF-to-local matrix.
//! Applications with a floating origin can supply their own views and placement.
#![forbid(unsafe_code)]

use bevy::{
    asset::RenderAssetUsages,
    math::DMat4,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
pub use cesium_native_bridge::View as CesiumView;
use std::collections::{HashMap, HashSet};

mod source;
pub use source::{CesiumConfigError, CesiumSource};

/// Mark ordinary Bevy cameras that should contribute to tile selection.
#[derive(Component)]
pub struct CesiumCamera;

/// ECEF camera poses (meters), viewport dimensions (pixels) and FOV (radians).
/// External adapters update this resource before CesiumSet::Stream.
#[derive(Resource, Default)]
pub struct CesiumViews(pub Vec<CesiumView>);

/// Transform from Earth-centered Earth-fixed coordinates to local Bevy space.
/// Subtraction/rotation takes place in f64 before conversion to Bevy's f32.
/// Use a rigid transform and update it together with cameras when rebasing.
#[derive(Resource)]
pub struct CesiumGeoreference(pub DMat4);
impl Default for CesiumGeoreference {
    fn default() -> Self {
        Self(DMat4::IDENTITY)
    }
}

/// Native transform, retained in double precision for alternative spatial systems.
#[derive(Component)]
pub struct CesiumPrimitive {
    pub ecef_from_mesh: DMat4,
}

/// Native attribution HTML. The host must display it, preserving links/logos as
/// required by its data provider. No application-specific UI is imposed here.
#[derive(Resource, Default)]
pub struct CesiumCredits {
    pub html: Vec<String>,
}

#[derive(Resource, Default)]
pub struct CesiumStatus {
    pub pending: u32,
    pub load_failed: bool,
    pub resident_tiles: usize,
    pub visible_tiles: usize,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CesiumSet {
    Views,
    Stream,
    Placement,
}

struct NativeSession {
    session: cesium_native_bridge::Session,
    tiles: HashMap<u64, Vec<Entity>>,
}

pub struct CesiumPlugin {
    source: CesiumSource,
    external_coordinates: bool,
}
impl CesiumPlugin {
    pub fn new(source: CesiumSource) -> Self {
        Self {
            source,
            external_coordinates: false,
        }
    }
    /// Disable ordinary camera collection and local placement. The application
    /// supplies CesiumViews and places new CesiumPrimitive entities itself.
    pub fn with_external_coordinates(mut self) -> Self {
        self.external_coordinates = true;
        self
    }
}
impl Plugin for CesiumPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CesiumViews>()
            .init_resource::<CesiumCredits>()
            .init_resource::<CesiumStatus>()
            .init_resource::<CesiumGeoreference>()
            .configure_sets(
                Update,
                (CesiumSet::Views, CesiumSet::Stream, CesiumSet::Placement).chain(),
            );
        let result = match &self.source {
            CesiumSource::Url(url) => cesium_native_bridge::Session::open_url(url),
            CesiumSource::Ion {
                asset_id,
                access_token,
            } => cesium_native_bridge::Session::open_ion(*asset_id, access_token),
        };
        match result {
            Ok(session) => {
                app.insert_non_send(NativeSession {
                    session,
                    tiles: HashMap::new(),
                })
                .add_systems(Update, stream.in_set(CesiumSet::Stream));
                if !self.external_coordinates {
                    app.add_systems(Update, collect_views.in_set(CesiumSet::Views))
                        .add_systems(Update, place_tiles.in_set(CesiumSet::Placement));
                }
            }
            Err(_) => {
                app.world_mut().resource_mut::<CesiumStatus>().load_failed = true;
                error!("Could not initialize Cesium Native");
            }
        }
    }
}

fn collect_views(
    cameras: Query<(&Camera, &Projection, &GlobalTransform), With<CesiumCamera>>,
    geo: Res<CesiumGeoreference>,
    mut views: ResMut<CesiumViews>,
) {
    let ecef_from_world = geo.0.inverse();
    views.0.clear();
    for (camera, projection, transform) in &cameras {
        if !camera.is_active {
            continue;
        }
        let Projection::Perspective(p) = projection else {
            continue;
        };
        let Some(size) = camera.physical_viewport_size() else {
            continue;
        };
        if size.x == 0 || size.y == 0 {
            continue;
        }
        views.0.push(CesiumView {
            position: ecef_from_world
                .transform_point3(transform.translation().as_dvec3())
                .to_array(),
            direction: ecef_from_world
                .transform_vector3(transform.forward().as_dvec3())
                .normalize()
                .to_array(),
            up: ecef_from_world
                .transform_vector3(transform.up().as_dvec3())
                .normalize()
                .to_array(),
            width: size.x as f64,
            height: size.y as f64,
            vertical_fov: p.fov as f64,
            horizontal_fov: 2.0
                * ((p.fov as f64 * 0.5).tan() * size.x as f64 / size.y as f64).atan(),
        });
    }
}

fn local_transform(world_from_ecef: DMat4, ecef_from_mesh: DMat4) -> Transform {
    let (scale, rotation, translation) =
        (world_from_ecef * ecef_from_mesh).to_scale_rotation_translation();
    Transform {
        translation: translation.as_vec3(),
        rotation: rotation.as_quat(),
        scale: scale.as_vec3(),
    }
}
fn place_tiles(
    geo: Res<CesiumGeoreference>,
    mut tiles: Query<(Ref<CesiumPrimitive>, &mut Transform)>,
) {
    for (tile, mut transform) in &mut tiles {
        if geo.is_changed() || tile.is_changed() {
            *transform = local_transform(geo.0, tile.ecef_from_mesh);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stream(
    mut commands: Commands,
    mut native: NonSendMut<NativeSession>,
    views: Res<CesiumViews>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut visibility: Query<&mut Visibility>,
    mut credits: ResMut<CesiumCredits>,
    mut status: ResMut<CesiumStatus>,
) {
    let Ok(frame) = native.session.update(&views.0, time.delta_secs()) else {
        status.load_failed = true;
        return;
    };
    credits.html = frame.credits;
    status.load_failed = frame.load_failed;
    status.pending = frame.pending;
    status.visible_tiles = frame.visible.len();
    for tile in frame.loaded {
        let mut entities = Vec::new();
        for p in tile.primitives {
            let matrix = DMat4::from_cols_array(&p.transform);
            let positions = p.positions.as_chunks::<3>().0.to_vec();
            let normals = p.normals.as_chunks::<3>().0.to_vec();
            let uvs = p.uvs.as_chunks::<2>().0.to_vec();
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_indices(Indices::U32(p.indices));
            let texture = if p.width > 0 && p.height > 0 {
                Some(images.add(Image::new(
                    Extent3d {
                        width: p.width,
                        height: p.height,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    p.pixels,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::default(),
                )))
            } else {
                None
            };
            let material = StandardMaterial {
                base_color: Color::linear_rgba(
                    p.base_color[0],
                    p.base_color[1],
                    p.base_color[2],
                    p.base_color[3],
                ),
                base_color_texture: texture,
                double_sided: p.double_sided,
                cull_mode: if p.double_sided {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                metallic: p.metallic,
                perceptual_roughness: p.roughness,
                emissive: LinearRgba::rgb(p.emissive[0], p.emissive[1], p.emissive[2]),
                alpha_mode: match p.alpha_mode {
                    1 => AlphaMode::Mask(p.alpha_cutoff),
                    2 => AlphaMode::Blend,
                    _ => AlphaMode::Opaque,
                },
                ..default()
            };
            let entity = commands
                .spawn((
                    Name::new("Cesium tile"),
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.add(material)),
                    Transform::IDENTITY,
                    CesiumPrimitive {
                        ecef_from_mesh: matrix,
                    },
                    Visibility::Hidden,
                ))
                .id();
            entities.push(entity);
        }
        native.tiles.insert(tile.id, entities);
    }
    // A tile can finish loading and be evicted in the same update.
    for id in frame.removed {
        if let Some(entities) = native.tiles.remove(&id) {
            for entity in entities {
                commands.entity(entity).despawn();
            }
        }
    }
    status.resident_tiles = native.tiles.len();
    let visible: HashSet<_> = frame.visible.into_iter().collect();
    for (id, entities) in &native.tiles {
        let desired = if visible.contains(id) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        for entity in entities {
            if let Ok(mut current) = visibility.get_mut(*entity) {
                if *current != desired {
                    *current = desired;
                }
            } else {
                commands.entity(*entity).insert(desired);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::{DQuat, DVec3};
    #[test]
    fn retains_submeter_offsets_at_earth_scale() {
        let origin = DVec3::new(6_378_137.0, 20_000_000.0, -30_000_000.0);
        let rotation = DQuat::from_rotation_x(0.6);
        let world_from_ecef = DMat4::from_rotation_translation(rotation, origin).inverse();
        let offset = DVec3::new(0.125, -0.25, 0.375);
        let pose = DMat4::from_translation(origin + rotation * offset);
        let local = local_transform(world_from_ecef, pose);
        assert!((local.translation.as_dvec3() - offset).length() < 1e-6);
    }
    #[test]
    fn rebasing_updates_existing_tiles() {
        let mut app = App::new();
        app.init_resource::<CesiumGeoreference>()
            .add_systems(Update, place_tiles);
        let e = app
            .world_mut()
            .spawn((
                CesiumPrimitive {
                    ecef_from_mesh: DMat4::from_translation(DVec3::new(6_378_137.125, 0.0, 0.0)),
                },
                Transform::IDENTITY,
            ))
            .id();
        app.world_mut().resource_mut::<CesiumGeoreference>().0 =
            DMat4::from_translation(DVec3::new(-6_378_137.0, 0.0, 0.0));
        app.update();
        assert_eq!(
            app.world().get::<Transform>(e).unwrap().translation.x,
            0.125
        );
        app.world_mut().resource_mut::<CesiumGeoreference>().0 =
            DMat4::from_translation(DVec3::new(-6_378_127.0, 0.0, 0.0));
        app.update();
        assert_eq!(
            app.world().get::<Transform>(e).unwrap().translation.x,
            10.125
        );
    }
}
