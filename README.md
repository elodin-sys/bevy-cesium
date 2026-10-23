# Cesium for Bevy

Load real terrain, aerial imagery and 3D Tiles into a Bevy application.
Cesium Native downloads and selects the data; Bevy renders it locally.

Built for **Bevy 0.19** and **Cesium Native 0.65.0**. Experimental,
maintained by Elodin independently of Cesium. Tested locally on Apple Silicon macOS.

## Try it

```sh
nix develop
just prepare   # build the native SDK once, or set CESIUM_NATIVE_DIR
just install
just viewer    # public 3D Tiles example; no token needed
```

For Cesium ion terrain and aerial imagery, set `CESIUM_ION_ASSET_ID=1`,
`CESIUM_ION_IMAGERY_ASSET_ID=2` and `CESIUM_ION_TOKEN` before launching.
Keep the token out of source control. The viewer's location is set with
`CESIUM_LAT`, `CESIUM_LON` and `CESIUM_ALT`. Use `CESIUM_TILESET_URL` for another tileset.

## Use it in an app

Add `CesiumPlugin`, mark your cameras with `CesiumCamera`, and set
`CesiumGeoreference` to map Earth coordinates into your scene.
The [standalone viewer](examples/viewer.rs) is a complete example.
Your app supplies lighting and displays the source attribution from `CesiumCredits`.

```rust,ignore
app.add_plugins(
    CesiumPlugin::new(source)
        .with_maximum_screen_error(16.0)
        .with_lod_retention_margin(500.0),
);
```

The screen-error setting controls detail: smaller values request finer terrain.
The retention setting keeps acquired detail until it leaves all camera views,
with an extra margin in meters. Retention is off by default; this example enables it.

## Terrain stability

- **Flickering:** downloaded terrain may still be waiting for the GPU. Bevy-Cesium
  keeps the old terrain visible until its replacement is ready, then swaps it in
  together. Texture updates follow the same rule, avoiding temporary holes and
  overlapping versions during the swap.
- **LOD (level of detail):** terrain normally becomes more detailed up close and
  coarser farther away. With retention enabled, moving away keeps the detail
  already acquired until the area leaves every camera view, including the margin.
  This uses more memory; the unused-cache budget does not cap retained terrain.
- **Several cameras:** detail needed by any supplied camera is kept. A temporary
  gap with no cameras does not clear it.

Elodin's RC-jet integration uses this library for both Editor and sensor views.
It enables a **500 m margin** and **16 px screen error**, and keeps sensor camera
requirements active between captured images. Elodin pins an exact Git revision
of Bevy-Cesium so an upstream change cannot silently change its renderer.

## Controlled captures

For a known camera path, supply all reference views before loading:

```rust,ignore
app.insert_resource(CesiumDetailViews(reference_camera_path))
    .insert_resource(CesiumSelectionMode::Fixed);
```

This makes terrain selection depend on those views instead of the live preview.
Wait for `CesiumStatus::ready_for_capture()` before saving an image: terrain and
imagery must be loaded, ready on the GPU, and unchanged for eight updates.
Set a timeout and allow the camera's rendering pipelines to warm up too.

In Elodin, `ELODIN_CESIUM_CAPTURE_VIEWS` selects the reference-view JSON file.
Captures fail on loading errors or after `ELODIN_CESIUM_CAPTURE_TIMEOUT`
(120 seconds by default). Camera positions, viewing angles and image sizes
must cover the intended flight and sensors.

**Repeatable CI (automated tests) still needs fixed input data.** Online ion data
can change. Identical pixels also require a controlled rendering environment.

## Progress

- [x] Stream terrain, aerial imagery and 3D Tiles into Bevy.
- [x] Keep the previous terrain visible until replacement geometry and textures
  are ready on the GPU.
- [x] Retain acquired detail while any camera still needs the area, with a margin.
- [x] Support multiple cameras and preserve detail during gaps between captures.
- [x] Support fixed reference views and signal when terrain is ready to capture.
- [x] Test loading failures, detail retention and GPU transitions with local assets;
  validate the integration locally in RC-jet.
- [ ] Cache downloaded terrain and imagery on disk for reuse between runs.
- [ ] Save a complete terrain package for a chosen region and camera path,
  with a version and file checksums to verify its contents.
- [ ] Load that package without network access; report missing or damaged files.
- [ ] In Elodin, replay RC-jet in CI with the same package and camera poses,
  wait for rendering to be ready, then compare captures.
- [ ] During interactive use, load the package first and download extra terrain
  when the camera leaves its coverage.
- [ ] Add terrain-height queries for simulation ground contact, separately from rendering.

## Development

`just test` and `just check` run the tests and code checks. Tests cover detail
retention and release, multiple cameras, incomplete loads and delayed GPU uploads.
For a visual example with two views and terrain/texture swaps:

```sh
CESIUM_STABILITY_OUTPUT=/tmp/terrain-stability \
  cargo run --release --example terrain_stability
```

For custom coordinate systems such as Elodin's floating origin, use
`.with_external_coordinates()`, supply `CesiumViews` in ECEF (Earth-centered,
Earth-fixed coordinates), and place `CesiumPrimitive` entities yourself.
The update order is `Views → Stream → Placement → Publish`; move cameras before
`Views`, place terrain in `Placement`, and check capture readiness after `Publish`.
See the [API reference](https://elodin-sys.github.io/bevy-cesium/bevy_tiles/).
Ion imagery currently supports one shared overlay per process, using the terrain token.
