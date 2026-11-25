//! The backend's CPU-ready notification is not a GPU-ready notification.
use bevy::{
    asset::AssetId,
    pbr::PreparedMaterial,
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        erased_render_asset::ErasedRenderAssets, mesh::RenderMesh, render_asset::RenderAssets,
        texture::GpuImage,
    },
};
use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, Mutex},
};

#[derive(Component, Clone, Hash)]
pub(crate) struct TileAssets {
    pub mesh: AssetId<Mesh>,
    pub material: AssetId<StandardMaterial>,
    pub image: Option<AssetId<Image>>,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct ReadyTiles(pub Arc<Mutex<HashSet<Entity>>>);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GpuPrepareSnapshot {
    pub candidates: usize,
    pub meshes: usize,
    pub images: usize,
    pub materials: usize,
    pub fingerprint: u64,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct GpuPrepareStatus(pub Arc<Mutex<GpuPrepareSnapshot>>);

#[derive(Resource, Default)]
struct Candidates(Vec<(Entity, TileAssets)>);

pub(crate) fn install(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        // ECS-only tests and servers have no GPU preparation stage.
        return;
    }
    let ready = ReadyTiles::default();
    let status = GpuPrepareStatus::default();
    app.insert_resource(ready.clone());
    app.insert_resource(status.clone());
    app.sub_app_mut(RenderApp)
        .insert_resource(ready)
        .insert_resource(status)
        .init_resource::<Candidates>()
        .add_systems(ExtractSchedule, extract_candidates)
        .add_systems(Render, report_ready.after(RenderSystems::PrepareAssets));
}

fn extract_candidates(
    tiles: Extract<Query<(Entity, &TileAssets)>>,
    mut candidates: ResMut<Candidates>,
) {
    candidates.0.clear();
    candidates
        .0
        .extend(tiles.iter().map(|(e, assets)| (e, assets.clone())));
}

fn report_ready(
    candidates: Res<Candidates>,
    meshes: Res<RenderAssets<RenderMesh>>,
    images: Res<RenderAssets<GpuImage>>,
    materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    ready: Res<ReadyTiles>,
    status: Res<GpuPrepareStatus>,
) {
    let mut result = ready.0.lock().expect("tile readiness lock poisoned");
    result.clear();
    let mut snapshot = status.0.lock().expect("tile readiness lock poisoned");
    *snapshot = GpuPrepareSnapshot {
        candidates: candidates.0.len(),
        ..default()
    };
    let mut fingerprint = DefaultHasher::new();
    for (entity, assets) in &candidates.0 {
        entity.hash(&mut fingerprint);
        assets.hash(&mut fingerprint);
        let mesh_ready = meshes.get(assets.mesh).is_some();
        let image_ready = assets.image.is_none_or(|id| images.get(id).is_some());
        let material_ready = materials.get(assets.material).is_some();
        snapshot.meshes += usize::from(mesh_ready);
        snapshot.images += usize::from(image_ready);
        snapshot.materials += usize::from(material_ready);
        if mesh_ready && material_ready {
            result.insert(*entity);
        }
    }
    snapshot.fingerprint = fingerprint.finish();
}
