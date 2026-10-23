//! The backend's CPU-ready notification is not a GPU-ready notification.
use bevy::{
    asset::AssetId,
    pbr::PreparedMaterial,
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        erased_render_asset::ErasedRenderAssets, mesh::RenderMesh, render_asset::RenderAssets,
    },
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Component, Clone)]
pub(crate) struct TileAssets {
    pub mesh: AssetId<Mesh>,
    pub material: AssetId<StandardMaterial>,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct ReadyTiles(pub Arc<Mutex<HashSet<Entity>>>);

#[derive(Resource, Default)]
struct Candidates(Vec<(Entity, TileAssets)>);

pub(crate) fn install(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        // ECS-only tests and servers have no GPU preparation stage.
        return;
    }
    let ready = ReadyTiles::default();
    app.insert_resource(ready.clone());
    app.sub_app_mut(RenderApp)
        .insert_resource(ready)
        .init_resource::<Candidates>()
        .add_systems(ExtractSchedule, extract_candidates)
        .add_systems(Render, report_ready.after(RenderSystems::PrepareAssets));
}

fn extract_candidates(
    tiles: Extract<Query<(Entity, &TileAssets)>>,
    mut candidates: ResMut<Candidates>,
) {
    candidates.0 = tiles
        .iter()
        .map(|(e, assets)| (e, assets.clone()))
        .collect();
}

fn report_ready(
    candidates: Res<Candidates>,
    meshes: Res<RenderAssets<RenderMesh>>,
    materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    ready: Res<ReadyTiles>,
) {
    let mut result = ready.0.lock().expect("tile readiness lock poisoned");
    result.clear();
    result.extend(candidates.0.iter().filter_map(|(e, assets)| {
        (meshes.get(assets.mesh).is_some() && materials.get(assets.material).is_some())
            .then_some(*e)
    }));
}
