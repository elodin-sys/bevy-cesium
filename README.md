# Cesium for Bevy

Terrain and 3D Tiles streaming for Bevy **0.19**, using Cesium Native **0.65.0**.
Maintained by Elodin; experimental, independently of Cesium.

`cesium-native-bridge` loads and decodes data; `bevy-tiles` publishes meshes and
materials; `bevy-cesium` connects them. The host supplies cameras, lighting,
coordinates and attribution. Rendering runs locally on the GPU.

## Build and run

```sh
nix develop
just prepare      # cached native SDK; or set CESIUM_NATIVE_DIR
just install
just viewer       # public sample, no token required
just test
just check
```

Validated locally on Apple Silicon macOS. `CESIUM_TILESET_URL` selects a tileset.
For ion terrain and imagery, set `CESIUM_ION_ASSET_ID=1`,
`CESIUM_ION_IMAGERY_ASSET_ID=2` and `CESIUM_ION_TOKEN` outside source control.
Ion imagery is currently one process-wide overlay using the terrain token.
`CESIUM_LAT`, `CESIUM_LON` and `CESIUM_ALT` configure the viewer's local origin.

## Integration

Add `CesiumPlugin::new(source)`, mark cameras with `CesiumCamera`, and provide
`CesiumGeoreference` (ECEF to local coordinates). Positions stay double precision
until local placement. Sources can be URLs or ion assets; `CesiumSource::from_env()`
reads the environment without exposing credentials in errors.

Floating-origin hosts use `.with_external_coordinates()`, supply ECEF
`CesiumViews` and place new `CesiumPrimitive` entities themselves. Schedule camera
motion before `CesiumSet::Views`, placement in `Placement`, and capture checks
after `Publish`: **Views → Stream → Placement → Publish**.
[API reference](https://elodin-sys.github.io/bevy-cesium/bevy_tiles/).

## Terrain stability

CPU-loaded tiles can still be awaiting GPU uploads. The renderer keeps the
previous selection until every replacement mesh/material is prepared, then
publishes it together. Imagery revisions replace their predecessors without
exposing both. This prevents upload gaps and parent/child overlap at publication;
it is separate from camera render order and shader-pipeline warm-up.

```rust,ignore
app.add_plugins(CesiumPlugin::new(source)
    .with_maximum_screen_error(16.0)
    .with_unused_cache_bytes(256 * 1024 * 1024)
    .with_lod_retention_margin(500.0));
```

Retention preserves the refinement paths of acquired tiles, not just their cached
bytes. Approaching can add detail; retreating does not discard it while the tile's
bounds intersect any selection camera, including the configured spatial margin.
The margin is measured in ECEF meters using conservative bounding spheres. Once
outside **every** camera plus margin, detail may be released. No-camera scheduling
gaps keep existing requirements. Retention defaults to disabled (`0`).

Native geographic views enforce those paths without changing dataset errors or
saving an ever-growing list of past camera poses. Descendants supersede ancestor
requirements. Active retained tiles can exceed the unused-cache budget; GPU memory
is additional. This policy favors quality over a hard memory limit. It does not
pin detail after leaving the region or make a live flight independent of its history.

## Fixed captures and CI

Populate the full required camera path **before loading**:

```rust,ignore
app.insert_resource(CesiumDetailViews(reference_camera_path))
    .insert_resource(CesiumSelectionMode::Fixed);
```

Fixed mode excludes live preview cameras from selection. Keep reference poses,
physical viewport sizes, FOV, screen-error threshold and input assets fixed.
After `Publish`, wait for `CesiumStatus::ready_for_capture()`: Native geometry
and imagery complete, no known content failure, GPU publication complete, and
eight unchanged complete updates. Enforce a timeout and warm camera pipelines.
A missing child can leave its parent visible interactively while blocking capture.
`pending == 0` alone is insufficient: transfers or imagery may still be loading.

Tests use committed local assets: near/far motion, margin release, multiple
cameras, zero unused-cache budget, scheduling gaps, failed loads, delayed GPU
preparation, and content comparison across independent sessions. For visual
parent/child and imagery transitions with two camera viewports:

```sh
CESIUM_STABILITY_OUTPUT=/tmp/terrain-stability \
  cargo run --release --example terrain_stability
```

In Elodin's RC-jet integration, both the Editor and sensor renderer use 500 m
retention and 16 px screen error. Inactive sensor cameras still contribute their
terrain requirements. `ELODIN_CESIUM_CAPTURE_VIEWS` supplies a JSON array of ECEF
reference views; fixed captures wait for readiness or fail after
`ELODIN_CESIUM_CAPTURE_TIMEOUT` seconds (default 120). Elodin pins this library to
an exact Git revision; the RC-jet integration branch can remain local.

## Scope

This stage provides streamed terrain/imagery, geographic placement, stable
publication and spatial LOD retention. Ion data can change between runs.
Repeatable CI needs versioned local inputs and a controlled GPU/rendering setup;
these tests establish content/selection stability, not cross-machine pixel parity.
Saved terrain packages, persistent caching, strict offline/hybrid modes and terrain
collision queries are separate work. The host must display `CesiumCredits` with
the provider's required links/logos. Lighting and exposure remain host decisions.
