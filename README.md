# Cesium for Bevy — experimental

A reusable Bevy 0.19 plugin backed by Cesium Native 0.65.0. Maintained by Elodin as a reusable library, independently of the Editor; it is not an official Cesium integration.

- `cesium-native-bridge`: owned Rust data and a private CXX boundary to Cesium Native. Native pointers stay private. Sessions stay on their creating thread.
- `bevy-cesium`: streaming, mesh/texture creation, tile visibility and eviction, ECEF camera selection and attribution data.
- `examples/viewer.rs`: ordinary Bevy application; no Elodin, Python SDK or database.

## Run from this repository

```sh
nix develop path:.
just prepare      # build the pinned native SDK; cached outside this checkout
just install
just viewer       # public AGI headquarters sample; no token needed
just test
just check
```

An existing SDK may be selected with `CESIUM_NATIVE_DIR`. Building requires C++20 and CMake in addition to Rust. The native SDK script uses `uv` for its small build patch. This first extraction is being validated on Apple Silicon macOS; other platforms are not yet certified. Keep API tokens outside source control.

The demo orbits the AGI sample (hold Space to pause). `CESIUM_TILESET_URL` selects another source. `CESIUM_LAT`, `CESIUM_LON` (degrees), and `CESIUM_ALT` (meters above the WGS84 ellipsoid) set its local origin. Alternatively set `CESIUM_ION_ASSET_ID` and `CESIUM_ION_TOKEN`. `CESIUM_SCREENSHOT=/tmp/cesium.png` captures the loaded viewport.

## Application API

Add `CesiumPlugin::new(CesiumSource::Url(url))`, provide a `CesiumGeoreference` ECEF-to-world matrix, and mark participating cameras with `CesiumCamera`. Tile poses remain f64 until transformed into the local frame. The default adapter is intended for local scenes; it does not implement planet-scale camera navigation.

`CesiumSource::from_env()` optionally reads `CESIUM_TILESET_URL` or `CESIUM_ION_ASSET_ID` / `CESIUM_ION_TOKEN`. It returns `None` when disabled and a redacted configuration error for incomplete settings. Ion takes precedence; invalid ion settings do not silently fall back to the URL. Programmatic configuration remains available. This validates configuration syntax, not ion credentials.

Applications with their own floating-origin system use `.with_external_coordinates()`: populate `CesiumViews` before `CesiumSet::Stream`, then place newly spawned `CesiumPrimitive` entities in `CesiumSet::Placement`. Each primitive retains its f64 `ecef_from_mesh` transform. This is how Elodin consumes the same plugin without imposing its spatial types on other users.

The host must display `CesiumCredits` in accordance with its data provider's requirements, including applicable links and logos. The minimal demo displays returned HTML as text and is only an attribution example for the public sample; it is not a general HTML attribution renderer. `CesiumStatus` exposes pending loads, failures and tile counts without logging signed URLs.

## Current limits

Textured triangle meshes support base-color textures (including the selected UV set), metallic/roughness factors, emissive factors, opaque/blended/masked alpha and double-sided surfaces. Additional PBR texture maps and material extensions are not implemented. Raster imagery overlays, full glTF materials, picking, collisions and a continuous surface-to-globe experience are not implemented. The Falcon video used Elodin's own Earth renderer and a video dissolve; it is not evidence of a Cesium globe transition. No multi-platform release or API stability is claimed yet.

The C++ dependency and narrow Rust `unsafe extern` declarations remain confined to the bridge. The Bevy plugin forbids unsafe Rust. Memory and threading safety still depend on the native implementation; extraction does not remove that responsibility.

Licensed under Apache-2.0. Initial code extracted from Elodin's experimental Cesium integration; Cesium Native and its dependencies retain their own licenses.

## Local validation (2026-10-13)

On Apple Silicon macOS: standalone release viewer displays the public AGI sample; eleven tests pass (ECEF precision/rebasing, headless Bevy loading, native textured fixture, material modes/factors, nonzero UV sets, environment configuration and input/error handling); `cargo fmt` and Clippy with `-D warnings` pass. The Elodin consumer builds with `just install editor` and renders the same sample in its multiple viewports. The standalone Nix development shell starts successfully. These checks do not certify Windows/Linux, ion credentials, or raster overlays.

The [ECEF end-to-end test](tests/ecef_end_to_end.rs) drives two real Bevy camera components through Cesium Native selection and Bevy mesh placement, rebases the origin by several kilometers and back, checks geometry within 1 mm in ECEF, and disables one viewport without duplicating the loaded mesh. It runs headless on a local georeferenced fixture. GPU pixel output, globe-scale terrain and network/ion behavior are outside this automated test.
