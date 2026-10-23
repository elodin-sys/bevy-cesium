//! Shared scene boundary and rendering for interchangeable 3D Tiles backends.
//! The default coordinate adapter uses a double-precision ECEF-to-local matrix.
//! Applications with a floating origin can supply their own views and placement.
#![forbid(unsafe_code)]

use bevy::{
    asset::RenderAssetUsages,
    math::DMat4,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    transform::helper::TransformHelper,
};
mod readiness;
use readiness::{ReadyTiles, TileAssets};
mod contract;
pub use contract::*;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

mod source;
pub use source::{CesiumConfigError, CesiumSource, GeographicImagery};

/// Mark ordinary Bevy cameras that should contribute to tile selection.
#[derive(Component)]
pub struct CesiumCamera;

/// ECEF camera poses (meters), viewport dimensions (pixels) and FOV (radians).
/// External adapters update this resource before CesiumSet::Stream.
#[derive(Resource, Default)]
pub struct CesiumViews(pub Vec<CesiumView>);

/// Views whose detail requirements must remain selected even when live cameras
/// move away. Populate with the full capture path before warming a CI scene.
/// Remove views only once their terrain is outside the application's required
/// region. This retains selection quality, not merely cached tile bytes.
#[derive(Resource, Default)]
pub struct CesiumDetailViews(pub Vec<CesiumView>);

/// Fixed selection ignores live cameras so a preview camera cannot alter a
/// capture's terrain requirements. Supply the complete path in CesiumDetailViews.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum CesiumSelectionMode {
    #[default]
    Live,
    Fixed,
}

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
    /// Requested visible tiles not yet published because GPU assets are pending.
    pub pending_render_tiles: usize,
    /// Redacted diagnostic, suitable for host UI. No URLs or credentials.
    pub error: Option<&'static str>,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CesiumSet {
    Views,
    Stream,
    Placement,
    /// Publish visibility after the host has placed newly created entities.
    Publish,
}

struct NativeSession<B: Backend> {
    session: B,
    tiles: HashMap<u64, Vec<Entity>>,
    replacements: HashMap<u64, Vec<Entity>>,
    selected: HashSet<u64>,
    published: HashSet<u64>,
    removed: HashSet<u64>,
}

pub struct TilesPlugin<B: Backend> {
    backend: PhantomData<fn() -> B>,
    options: Options,
    source: CesiumSource,
    external_coordinates: bool,
}
impl<B: Backend> TilesPlugin<B> {
    pub fn new(source: CesiumSource) -> Self {
        Self {
            source,
            backend: PhantomData,
            options: Options::default(),
            external_coordinates: false,
        }
    }
    pub fn with_unused_cache_bytes(mut self, bytes: usize) -> Self {
        self.options.unused_cache_bytes = bytes;
        self
    }
    pub fn with_maximum_screen_error(mut self, pixels: f64) -> Self {
        self.options.maximum_screen_error = pixels;
        self
    }
    /// Disable ordinary camera collection and local placement. The application
    /// supplies CesiumViews and places new CesiumPrimitive entities itself.
    pub fn with_external_coordinates(mut self) -> Self {
        self.external_coordinates = true;
        self
    }
}
impl<B: Backend> Plugin for TilesPlugin<B> {
    fn build(&self, app: &mut App) {
        readiness::install(app);
        app.init_resource::<CesiumViews>()
            .init_resource::<CesiumDetailViews>()
            .init_resource::<CesiumSelectionMode>()
            .init_resource::<CesiumCredits>()
            .init_resource::<CesiumStatus>()
            .init_resource::<CesiumGeoreference>()
            .configure_sets(
                Update,
                (
                    CesiumSet::Views,
                    CesiumSet::Stream,
                    CesiumSet::Placement,
                    CesiumSet::Publish,
                )
                    .chain(),
            );
        let result = if !self.source.valid() {
            Err(BackendError("Invalid source configuration"))
        } else if self.options.maximum_screen_error.is_finite()
            && self.options.maximum_screen_error > 0.0
            && self.options.unused_cache_bytes as u64 <= i64::MAX as u64
        {
            B::open(&self.source, self.options)
        } else {
            Err(BackendError("Invalid streaming options"))
        };
        match result {
            Ok(session) => {
                app.insert_non_send(NativeSession {
                    session,
                    tiles: HashMap::new(),
                    replacements: HashMap::new(),
                    selected: HashSet::new(),
                    published: HashSet::new(),
                    removed: HashSet::new(),
                })
                .add_systems(Update, stream::<B>.in_set(CesiumSet::Stream))
                .add_systems(Update, publish::<B>.in_set(CesiumSet::Publish));
                if !self.external_coordinates {
                    app.add_systems(Update, collect_views.in_set(CesiumSet::Views))
                        .add_systems(Update, place_tiles.in_set(CesiumSet::Placement));
                }
            }
            Err(error) => {
                let mut status = app.world_mut().resource_mut::<CesiumStatus>();
                status.load_failed = true;
                status.error = Some(error.0);
                error!("Could not initialize 3D Tiles backend");
            }
        }
    }
}

fn collect_views(
    cameras: Query<(Entity, &Camera, &Projection), With<CesiumCamera>>,
    transforms: TransformHelper,
    geo: Res<CesiumGeoreference>,
    mut views: ResMut<CesiumViews>,
) {
    let ecef_from_world = geo.0.inverse();
    views.0.clear();
    for (entity, camera, projection) in &cameras {
        // Update precedes global transform propagation. Resolve the current
        // local hierarchy rather than selecting LOD from last frame's pose.
        let Ok(transform) = transforms.compute_global_transform(entity) else {
            continue;
        };
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
fn stream<B: Backend>(
    mut commands: Commands,
    mut native: NonSendMut<NativeSession<B>>,
    views: Res<CesiumViews>,
    detail_views: Res<CesiumDetailViews>,
    selection_mode: Res<CesiumSelectionMode>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut credits: ResMut<CesiumCredits>,
    mut status: ResMut<CesiumStatus>,
) {
    if *selection_mode == CesiumSelectionMode::Fixed && detail_views.0.is_empty() {
        status.load_failed = true;
        status.error = Some("Fixed terrain selection requires reference views");
        return;
    }
    let selection_views: Vec<_> = if *selection_mode == CesiumSelectionMode::Fixed {
        detail_views.0.clone()
    } else {
        views.0.iter().chain(&detail_views.0).copied().collect()
    };
    let frame = match native.session.update(&selection_views, time.delta_secs()) {
        Ok(frame) => frame,
        Err(error) => {
            status.load_failed = true;
            status.error = Some(error.0);
            return;
        }
    };
    credits.html = frame.credits;
    status.load_failed = frame.load_failed;
    status.error = if frame.load_failed {
        frame
            .error
            .or(status.error)
            .or(Some("Tileset loading failed"))
    } else {
        None
    };
    status.pending = frame.pending;
    native.selected = frame.visible.into_iter().collect();
    for tile in frame.loaded {
        // Coalesce repeated updates, but keep the currently published version
        // alive until the replacement has completed GPU asset preparation.
        if let Some(previous) = native.replacements.remove(&tile.id) {
            for entity in previous {
                commands.entity(entity).despawn();
            }
        }
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
                unlit: p.unlit,
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
            let mesh = meshes.add(mesh);
            let material = materials.add(material);
            let entity = commands
                .spawn((
                    Name::new("Cesium tile"),
                    TileAssets {
                        mesh: mesh.id(),
                        material: material.id(),
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                    CesiumPrimitive {
                        ecef_from_mesh: matrix,
                    },
                    Visibility::Hidden,
                ))
                .id();
            entities.push(entity);
        }
        native.replacements.insert(tile.id, entities);
        native.removed.remove(&tile.id);
    }
    native.removed.extend(frame.removed);
}

fn publish<B: Backend>(
    mut commands: Commands,
    mut native: NonSendMut<NativeSession<B>>,
    ready: Option<Res<ReadyTiles>>,
    mut visibility: Query<&mut Visibility>,
    mut status: ResMut<CesiumStatus>,
) {
    let ready = ready
        .as_ref()
        .map(|r| r.0.lock().expect("tile readiness lock poisoned"));
    let prepared = |entities: &[Entity]| {
        ready
            .as_ref()
            .is_none_or(|set| entities.iter().all(|e| set.contains(e)))
    };
    // Promote imagery revisions atomically per tile. Never expose both revisions.
    let completed: Vec<_> = native
        .replacements
        .iter()
        .filter(|(_, entities)| prepared(entities))
        .map(|(&id, _)| id)
        .collect();
    for id in completed {
        let entities = native.replacements.remove(&id).unwrap();
        if let Some(previous) = native.tiles.insert(id, entities) {
            for entity in previous {
                commands.entity(entity).despawn();
            }
        }
    }
    // Keep the entire previous selection while any new tile is not ready.
    // Mixing a partial child set with its parent causes holes or z-fighting.
    status.pending_render_tiles = native
        .selected
        .iter()
        .filter(|id| !native.tiles.contains_key(id) || native.replacements.contains_key(id))
        .count();
    if status.pending_render_tiles == 0 {
        native.published = native.selected.clone();
    }
    let removed: Vec<_> = native
        .removed
        .iter()
        .filter(|id| !native.published.contains(id))
        .copied()
        .collect();
    for id in removed {
        native.removed.remove(&id);
        for entities in [native.tiles.remove(&id), native.replacements.remove(&id)]
            .into_iter()
            .flatten()
        {
            for entity in entities {
                commands.entity(entity).despawn();
            }
        }
    }
    for (id, entities) in &native.tiles {
        let desired = if native.published.contains(id) {
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
    status.resident_tiles = native.tiles.len();
    status.visible_tiles = native.published.len();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetApp;
    use bevy::math::{DQuat, DVec3};

    struct ReplacingTile(u8);
    impl Backend for ReplacingTile {
        fn open(_: &CesiumSource, _: Options) -> Result<Self, BackendError> {
            Ok(Self(0))
        }
        fn update(&mut self, _: &[CesiumView], _: f32) -> Result<Frame, BackendError> {
            let tile = || Tile {
                id: 42,
                primitives: vec![Primitive {
                    transform: DMat4::IDENTITY.to_cols_array(),
                    positions: vec![0., 0., 0., 1., 0., 0., 0., 1., 0.],
                    normals: vec![0., 0., 1., 0., 0., 1., 0., 0., 1.],
                    uvs: vec![0., 0., 1., 0., 0., 1.],
                    indices: vec![0, 1, 2],
                    base_color: [1.; 4],
                    pixels: vec![10, 20, 30, 255],
                    width: 1,
                    height: 1,
                    double_sided: false,
                    unlit: false,
                    metallic: 0.,
                    roughness: 1.,
                    emissive: [0.; 3],
                    alpha_mode: 0,
                    alpha_cutoff: 0.5,
                }],
            };
            let frame = match self.0 {
                0 => Frame {
                    loaded: vec![tile()],
                    visible: vec![42],
                    ..default()
                },
                1 => Frame {
                    loaded: vec![tile(), tile()],
                    visible: vec![42],
                    ..default()
                },
                _ => Frame {
                    removed: vec![42],
                    ..default()
                },
            };
            self.0 += 1;
            Ok(frame)
        }
    }

    #[test]
    fn imagery_replacements_do_not_leave_duplicate_tile_entities() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .add_plugins(
                TilesPlugin::<ReplacingTile>::new(CesiumSource::Url("fixture".into()))
                    .with_external_coordinates(),
            );
        app.update();
        let previous = app
            .world_mut()
            .query_filtered::<Entity, With<CesiumPrimitive>>()
            .single(app.world())
            .unwrap();
        app.update();
        let current = app
            .world_mut()
            .query_filtered::<Entity, With<CesiumPrimitive>>()
            .single(app.world())
            .unwrap();
        assert_ne!(previous, current);
        assert!(app.world().get_entity(previous).is_err());
        assert_eq!(
            app.world().get::<Visibility>(current),
            Some(&Visibility::Visible)
        );
        assert_eq!(app.world().resource::<CesiumStatus>().resident_tiles, 1);
        let material = &app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(current)
            .unwrap()
            .0;
        let image = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(material)
            .unwrap()
            .base_color_texture
            .as_ref()
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(image)
                .unwrap()
                .data
                .as_deref(),
            Some([10, 20, 30, 255].as_slice())
        );
        app.update();
        assert!(app.world().get_entity(current).is_err());
        assert_eq!(app.world().resource::<CesiumStatus>().resident_tiles, 0);
    }

    #[derive(Default)]
    struct ControlledBackend {
        next: Frame,
        views: Vec<CesiumView>,
    }
    impl Backend for ControlledBackend {
        fn open(_: &CesiumSource, _: Options) -> Result<Self, BackendError> {
            Ok(Self::default())
        }
        fn update(&mut self, views: &[CesiumView], _: f32) -> Result<Frame, BackendError> {
            self.views = views.to_vec();
            Ok(std::mem::take(&mut self.next))
        }
    }
    fn controlled_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .add_plugins(
                TilesPlugin::<ControlledBackend>::new(CesiumSource::Url("fixture".into()))
                    .with_external_coordinates(),
            )
            .init_resource::<ReadyTiles>();
        app
    }
    fn packet(id: u64) -> Tile {
        let mut tile = ReplacingTile(0).update(&[], 0.).unwrap().loaded.remove(0);
        tile.id = id;
        tile
    }
    fn step(app: &mut App, loaded: Vec<Tile>, visible: Vec<u64>, removed: Vec<u64>) {
        app.world_mut()
            .non_send_mut::<NativeSession<ControlledBackend>>()
            .session
            .next = Frame {
            loaded,
            visible,
            removed,
            ..default()
        };
        app.update();
    }
    fn acknowledge(app: &mut App, id: u64) {
        let entities = app
            .world()
            .non_send::<NativeSession<ControlledBackend>>()
            .replacements[&id]
            .clone();
        app.world()
            .resource::<ReadyTiles>()
            .0
            .lock()
            .unwrap()
            .extend(entities);
    }
    fn visible_entities(app: &mut App) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<(Entity, &Visibility), With<CesiumPrimitive>>()
            .iter(app.world())
            .filter(|(_, v)| **v == Visibility::Visible)
            .map(|(e, _)| e)
            .collect()
    }
    #[test]
    fn imagery_keeps_old_render_until_replacement_is_gpu_ready() {
        let mut app = controlled_app();
        step(&mut app, vec![packet(42)], vec![42], vec![]);
        assert!(visible_entities(&mut app).is_empty());
        acknowledge(&mut app, 42);
        step(&mut app, vec![], vec![42], vec![]);
        let old = visible_entities(&mut app);
        assert_eq!(old.len(), 1);
        step(&mut app, vec![packet(42)], vec![42], vec![]);
        assert_eq!(visible_entities(&mut app), old);
        for _ in 0..5 {
            step(&mut app, vec![], vec![42], vec![]);
            assert_eq!(visible_entities(&mut app), old);
            assert_eq!(
                app.world().resource::<CesiumStatus>().pending_render_tiles,
                1
            );
        }
        acknowledge(&mut app, 42);
        step(&mut app, vec![], vec![42], vec![]);
        let new = visible_entities(&mut app);
        assert_eq!(new.len(), 1);
        assert_ne!(new, old);
        assert!(app.world().get_entity(old[0]).is_err());
    }
    #[test]
    fn lod_children_replace_parent_together_even_if_native_evicts_parent() {
        let mut app = controlled_app();
        step(&mut app, vec![packet(1)], vec![1], vec![]);
        acknowledge(&mut app, 1);
        step(&mut app, vec![], vec![1], vec![]);
        let parent = visible_entities(&mut app);
        step(&mut app, vec![packet(2), packet(3)], vec![2, 3], vec![1]);
        acknowledge(&mut app, 2);
        step(&mut app, vec![], vec![2, 3], vec![]);
        assert_eq!(visible_entities(&mut app), parent);
        acknowledge(&mut app, 3);
        step(&mut app, vec![], vec![2, 3], vec![]);
        assert_eq!(visible_entities(&mut app).len(), 2);
        assert!(app.world().get_entity(parent[0]).is_err());
        step(&mut app, vec![], vec![], vec![2, 3]);
        assert!(visible_entities(&mut app).is_empty());
        assert_eq!(app.world().resource::<CesiumStatus>().resident_tiles, 0);
    }
    #[test]
    fn superseded_and_evicted_pending_revisions_are_cleaned_up() {
        let mut app = controlled_app();
        step(&mut app, vec![packet(1), packet(1)], vec![1], vec![]);
        assert_eq!(
            app.world_mut()
                .query::<&CesiumPrimitive>()
                .iter(app.world())
                .count(),
            1
        );
        step(&mut app, vec![], vec![], vec![1]);
        assert_eq!(
            app.world_mut()
                .query::<&CesiumPrimitive>()
                .iter(app.world())
                .count(),
            0
        );
    }
    #[test]
    fn detail_views_survive_live_camera_motion_until_explicit_release() {
        let view = CesiumView {
            position: [1., 2., 3.],
            direction: [0., 0., -1.],
            up: [0., 1., 0.],
            width: 800.,
            height: 600.,
            horizontal_fov: 1.,
            vertical_fov: 0.8,
        };
        let mut app = controlled_app();
        app.world_mut()
            .resource_mut::<CesiumDetailViews>()
            .0
            .push(view);
        let mut live = view;
        live.position = [100., 200., 300.];
        app.world_mut().resource_mut::<CesiumViews>().0.push(live);
        step(&mut app, vec![], vec![], vec![]);
        let native = app.world().non_send::<NativeSession<ControlledBackend>>();
        assert_eq!(native.session.views.len(), 2);
        assert_eq!(native.session.views[1].position, view.position);
        app.world_mut().resource_mut::<CesiumViews>().0[0].position = [900., 900., 900.];
        step(&mut app, vec![], vec![], vec![]);
        let native = app.world().non_send::<NativeSession<ControlledBackend>>();
        assert_eq!(native.session.views[1].position, view.position);
        *app.world_mut().resource_mut::<CesiumSelectionMode>() = CesiumSelectionMode::Fixed;
        step(&mut app, vec![], vec![], vec![]);
        let native = app.world().non_send::<NativeSession<ControlledBackend>>();
        assert_eq!(native.session.views.len(), 1);
        assert_eq!(native.session.views[0].position, view.position);
        app.world_mut()
            .resource_mut::<CesiumDetailViews>()
            .0
            .clear();
        step(&mut app, vec![], vec![], vec![]);
        assert_eq!(
            app.world().resource::<CesiumStatus>().error,
            Some("Fixed terrain selection requires reference views")
        );
        *app.world_mut().resource_mut::<CesiumSelectionMode>() = CesiumSelectionMode::Live;
        step(&mut app, vec![], vec![], vec![]);
        assert_eq!(
            app.world()
                .non_send::<NativeSession<ControlledBackend>>()
                .session
                .views[0]
                .position,
            [900., 900., 900.]
        );
        assert!(!app.world().resource::<CesiumStatus>().load_failed);
    }

    #[test]
    fn camera_selection_uses_current_parent_pose_before_global_propagation() {
        let mut app = App::new();
        app.init_resource::<CesiumViews>()
            .init_resource::<CesiumGeoreference>()
            .add_systems(Update, collect_views);
        let parent = app
            .world_mut()
            .spawn(Transform::from_xyz(100., 0., 0.))
            .id();
        app.world_mut().spawn((
            CesiumCamera,
            Camera {
                viewport: Some(bevy::camera::Viewport {
                    physical_size: UVec2::new(800, 600),
                    ..default()
                }),
                ..default()
            },
            Projection::Perspective(PerspectiveProjection::default()),
            Transform::from_xyz(5., 0., 0.),
            ChildOf(parent),
        ));
        app.update();
        assert_eq!(
            app.world().resource::<CesiumViews>().0[0].position,
            [105., 0., 0.]
        );
        app.world_mut()
            .get_mut::<Transform>(parent)
            .unwrap()
            .translation
            .x = 200.;
        app.update();
        assert_eq!(
            app.world().resource::<CesiumViews>().0[0].position,
            [205., 0., 0.]
        );
    }

    struct NeverOpen;
    impl Backend for NeverOpen {
        fn open(_: &CesiumSource, _: Options) -> Result<Self, BackendError> {
            panic!("invalid options must not contact a backend")
        }
        fn update(&mut self, _: &[CesiumView], _: f32) -> Result<Frame, BackendError> {
            unreachable!()
        }
    }
    #[test]
    fn invalid_quality_is_rejected_before_opening_the_source() {
        for error in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut app = App::new();
            app.add_plugins(
                TilesPlugin::<NeverOpen>::new(CesiumSource::Ion {
                    asset_id: 42,
                    access_token: "secret".into(),
                })
                .with_maximum_screen_error(error),
            );
            let status = app.world().resource::<CesiumStatus>();
            assert!(status.load_failed);
            assert_eq!(status.error, Some("Invalid streaming options"));
        }
    }
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
