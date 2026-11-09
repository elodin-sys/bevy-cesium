//! Browser adapter: existing Rust streaming core, existing Bevy-Cesium renderer.
#![forbid(unsafe_code)]
use crate::render::*;

/// Browser plugin using the Rust tile loader and the shared Bevy renderer.
#[cfg(target_family = "wasm")]
pub type CesiumPlugin = TilesPlugin<BrowserBackend>;
/// Browser streaming backend. See the README for its supported source formats.
pub struct BrowserBackend(tiles_core::Session);

impl Backend for BrowserBackend {
    fn open(source: &CesiumSource, options: Options) -> Result<Self, BackendError> {
        if options.lod_retention_margin != 0.0 {
            return Err(BackendError(
                "LOD retention is not supported by the browser backend",
            ));
        }
        let mut session = match source {
            CesiumSource::IonTerrain { .. } => {
                return Err(BackendError(
                    "Ion terrain with imagery is not supported by the browser backend",
                ));
            }
            CesiumSource::Url(url) => tiles_core::Session::open_url(url),
            CesiumSource::Ion {
                asset_id,
                access_token,
            } => tiles_core::Session::open_ion(*asset_id, access_token),
            CesiumSource::Terrain { url, imagery } => tiles_core::Session::open_terrain_url(
                url,
                imagery.as_ref().map(|i| tiles_core::terrain::Imagery {
                    url_template: i.url_template.clone(),
                    credit: i.credit.clone(),
                }),
            ),
        }
        .map_err(|error| BackendError(error.0))?;
        session.maximum_screen_error = options.maximum_screen_error;
        session.cache_bytes = options.unused_cache_bytes;
        Ok(Self(session))
    }

    fn update(&mut self, views: &[CesiumView], seconds: f32) -> Result<Frame, BackendError> {
        let views: Vec<_> = views
            .iter()
            .map(|v| tiles_core::View {
                position: v.position,
                direction: v.direction,
                up: v.up,
                width: v.width,
                height: v.height,
                horizontal_fov: v.horizontal_fov,
                vertical_fov: v.vertical_fov,
            })
            .collect();
        let f = self
            .0
            .update(&views, seconds)
            .map_err(|error| BackendError(error.0))?;
        let complete = f.pending == 0 && !f.load_failed && !f.visible.is_empty();
        Ok(Frame {
            loaded: f
                .loaded
                .into_iter()
                .map(|t| Tile {
                    id: t.id,
                    primitives: t
                        .primitives
                        .into_iter()
                        .map(|p| Primitive {
                            transform: p.transform,
                            positions: p.positions,
                            normals: p.normals,
                            uvs: p.uvs,
                            indices: p.indices,
                            base_color: p.base_color,
                            pixels: p.pixels,
                            width: p.width,
                            height: p.height,
                            double_sided: p.double_sided,
                            unlit: p.unlit,
                            metallic: p.metallic,
                            roughness: p.roughness,
                            emissive: p.emissive,
                            alpha_mode: p.alpha_mode,
                            alpha_cutoff: p.alpha_cutoff,
                        })
                        .collect(),
                })
                .collect(),
            visible: f.visible,
            removed: f.removed,
            credits: f.credits,
            pending: f.pending,
            retained_tiles: 0,
            selection_complete: complete,
            load_failed: f.load_failed,
            error: f.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{asset::AssetApp, prelude::*};
    use std::time::{Duration, Instant};

    #[test]
    fn unsupported_options_fail_explicitly() {
        let source = CesiumSource::Url("https://example.com/tileset.json".into());
        assert!(matches!(
            BrowserBackend::open(
                &source,
                Options {
                    lod_retention_margin: 10.0,
                    ..Default::default()
                }
            ),
            Err(BackendError(
                "LOD retention is not supported by the browser backend"
            ))
        ));
        assert!(matches!(
            BrowserBackend::open(
                &CesiumSource::IonTerrain {
                    asset_id: 1,
                    imagery_asset_id: 2,
                    access_token: "unused".into()
                },
                Options::default()
            ),
            Err(BackendError(
                "Ion terrain with imagery is not supported by the browser backend"
            ))
        ));
    }

    #[test]
    fn browser_adapter_streams_and_removes_tiles_with_shared_renderer() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .add_plugins(
                TilesPlugin::<BrowserBackend>::new(CesiumSource::Url(format!(
                    "file://{}/tests/data/tileset.json",
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
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            let status = app.world().resource::<CesiumStatus>();
            assert!(!status.load_failed, "{:?}", status.error);
            if status.visible_tiles > 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "browser adapter did not publish the fixture"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            app.world_mut()
                .query::<&CesiumPrimitive>()
                .iter(app.world())
                .count(),
            1
        );
        app.world_mut().insert_resource(CesiumSourceUpdate(None));
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&CesiumPrimitive>()
                .iter(app.world())
                .count(),
            0
        );
    }
}
