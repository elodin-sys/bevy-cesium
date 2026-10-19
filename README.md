# Cesium for Bevy

A Rust library for displaying geographic 3D scenes in a Bevy application.
It downloads the parts of a scene needed by the cameras and turns them into
Bevy meshes and textures. Your application supplies cameras, lighting,
simulation and user interface.

Maintained by Elodin, independently of the Editor. This is an experimental
integration for Bevy 0.19 and Cesium Native 0.65.0, not an official Cesium product.

## The pieces, in plain terms

- **3D Tiles** is a format that splits a large scene into smaller files. The
  plugin loads more detail as the camera gets closer.
- **Cesium Native** is Cesium's C++ engine. It selects, downloads and decodes
  those files. A small Rust/C++ bridge connects it to this plugin.
- **Bevy** draws the scene locally on your GPU.
- **Cesium ion** is an optional service for hosting and accessing data. With
  ion, you supply an asset ID and an access token. A public tileset URL can work
  without an ion account. Ion does not render the scene for you.
- **ECEF** means positions in meters measured from the center of Earth. This
  lets the scenery and geographically located objects share one coordinate system.

## Repository structure

- `cesium-native-bridge`: owned Rust data and a private CXX boundary to Cesium Native. Native pointers stay private. Sessions stay on their creating thread.
- `bevy-tiles`: shared scene contract and Bevy renderer. No C++ dependency.
- `bevy-cesium`: Cesium Native backend for that contract.
- `examples/viewer.rs`: ordinary Bevy application; no Elodin, Python SDK or database.

<img width="1280" height="720" alt="image" src="https://github.com/user-attachments/assets/c504a675-1fb4-4dd5-9a97-1b4628dfe8d3" />



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

`.with_maximum_screen_error(pixels)` controls refinement and
`.with_unused_cache_bytes(bytes)` controls eviction pressure. Defaults are 16 px
and 256 MiB. Active tiles can exceed the budget; GPU allocations are additional.
The backend cache accounting differs, so equal settings are not equal memory usage.

The shared renderer uses sRGB base-color textures and linear material factors.
Lights, environment maps, exposure and tonemapping belong to the host; neither
plugin installs or adjusts them. This does not establish GPU pixel parity.

Ion access is delegated to Cesium Native. Supply an asset ID and a scoped token
through `CesiumSource::Ion` or the environment. Credential storage and UI belong
to the host. A successful build is not validation of a live ion account or asset.

The shared API also accepts `CesiumSource::Terrain` with optional
`GeographicImagery`. The Native adapter currently rejects imagery overlays
explicitly: the SDK supports overlays, but this bridge does not expose them.
Terrain without an overlay is passed to Native; a full terrain/Google workflow
has not been validated in this plugin.

## Usage examples

### Integration into Elodin Editor

https://github.com/user-attachments/assets/8cc6c6b6-20e7-4b96-a51c-ef1f1659bdb0
