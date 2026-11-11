# Bevy-Cesium

Stream terrain, aerial imagery and 3D Tiles into **Bevy 0.19**.
Native applications use **Cesium Native 0.65.0**; browser applications use a
Rust/Fetch loader. Prepared packages use the Rust loader on both platforms.
Both share the same Bevy renderer and coordinate API.

Experimental, maintained by Elodin independently of Cesium.
Use a pinned Git commit or release tag; the crate is not published to crates.io.

[Native API](https://elodin-sys.github.io/bevy-cesium/bevy_cesium/)
· [Examples](examples)

## Try it

Run from the repository root inside `nix develop`.

**Native**

```sh
just prepare   # build the SDK once, or set CESIUM_NATIVE_DIR
just install
just viewer
```

The viewer loads a public 3D Tiles sample without a token. Use
`CESIUM_TILESET_URL` for another tileset. For ion terrain and aerial imagery,
set `CESIUM_ION_ASSET_ID=1`, `CESIUM_ION_IMAGERY_ASSET_ID=2` and
`CESIUM_ION_TOKEN`. Set the location with `CESIUM_LAT`, `CESIUM_LON` and
`CESIUM_ALT`. Keep tokens out of source control.

**Browser**

```sh
just install web
just web
```

Open <http://127.0.0.1:8842> in a WebGPU-capable browser. The example loads a small
3D Tiles fixture over local HTTP and reports GPU readiness. No token or external
service is required. Browser builds do not compile the C++ bridge or link the
native SDK.

## Use it

Add `CesiumPlugin`, mark your cameras with `CesiumCamera`, and set
`CesiumGeoreference` to map ECEF (Earth-centered, Earth-fixed) coordinates into
your scene. Your application supplies lighting and displays `CesiumCredits`,
preserving any required attribution links and logos.

```rust,ignore
app.add_plugins(
    CesiumPlugin::new(source)
        .with_maximum_screen_error(16.0)
        .with_initial_load_wait(),
);
```

The backend is selected by target, without Cargo feature flags. See the
[native viewer](examples/viewer.rs) and [browser example](examples/browser.rs)
for complete applications.

Smaller screen-error values request finer detail. Initial-load waiting keeps
provisional terrain hidden until geometry and imagery are ready on the GPU.
Show loading progress, handle `CesiumStatus` errors and set a timeout in your app.
Later updates keep the previous selection visible until its replacement is ready.

For data received after startup, use `CesiumPlugin::deferred()` and insert
`CesiumSourceUpdate(Some(source))`. Replacing a source removes its old geometry;
`CesiumSourceUpdate(None)` disables streaming.

For floating-origin systems, use `.with_external_coordinates()`, supply
`CesiumViews` in ECEF and place `CesiumPrimitive` entities yourself. The update
order is `Views → Stream → Placement → Publish`.

For controlled captures, supply `CesiumDetailViews`, select
`CesiumSelectionMode::Fixed` and wait for `CesiumStatus::ready_for_capture()`.
Repeatable images also require fixed input data and a controlled renderer.

## Sources and platform limits

| Capability | Native | Browser |
| --- | --- | --- |
| Explicit 3D Tiles, nested tilesets, glTF/GLB and Draco meshes | Yes | Yes |
| Ion 3D Tiles | Yes | Yes |
| Quantized-mesh terrain URL | Yes | Yes, with limits below |
| Geographic TMS imagery on a terrain URL | Not exposed | Yes |
| `IonTerrain` with ion imagery | Yes | Unsupported |
| Nonzero LOD retention margin | Yes | Unsupported |
| Height queries (`TerrainSampler`) | Yes | Unavailable |
| Persistent HTTP cache | On disk | Browser-managed |

Pass an absolute HTTP(S) URL to `CesiumSource::Url` or `CesiumSource::Terrain` in
the browser. Child URLs resolve relative to the tileset/layer URL. Assets must be
served from the same origin or allow CORS; browser Fetch cannot read `file:` URLs.
Supply ion credentials explicitly through the source configuration.

Use `CesiumSource::Prepared { url, ion_access_token: None }` for an offline
package. Resources must remain within its directory; provider requests and
redirects are rejected. An HTTP-hosted package still needs access to its asset
server.

For cache-first Photo 3D streaming, supply an ion token and a sibling
`package.json` manifest. Cached resources are read first; missing tiles use a
fresh provider session. Tokens and provider sessions are not stored in the
package. Downloads remain transient and do not update the package. Prepared
sources use the Rust loader's limits and disable native LOD retention.
They display available cached detail immediately, even with initial-load waiting
enabled; `ready_for_capture()` still waits for a complete, error-free selection.

Browser terrain requires EPSG:4326/TMS, a maximum level of 24 and static
availability above level 6. Dynamic metadata availability and parent layers are
unsupported. Imagery must match the terrain's geographic grid; it is not
reprojected. Unsupported sources and nonzero retention margins return errors.

The browser loader implements a subset of Cesium Native. It permits four tile
jobs concurrently and responses up to 32 MiB; decoding runs on the browser
thread. The unused-tile cache budget does not cap active tiles or GPU memory.
Its source and licenses come from the [terrain demo](crates/tiles-core/README.md).

On native targets, `.with_lod_retention_margin(meters)` preserves acquired detail
until it leaves every camera view plus the margin. Retention is off by default
and increases memory use. Rendering and height queries share an HTTP cache under
`$XDG_CACHE_HOME/bevy-cesium/terrain`, falling back to
`~/.cache/bevy-cesium/terrain`.

`TerrainSampler` queries heights above the WGS84 ellipsoid independently of camera
LOD. Reuse a sampler across batches; after a query error, create a fresh one.
Rendering terrain does not provide physics collisions or ground contact.

## Development

```sh
just test
just check
```

Tests cover streaming, materials, source replacement, coordinate changes, cache
reuse and loading failures. Optional ion tests require credentials. CI tests the
Rust loader, typechecks the native library and links the WASM example without a
native SDK. Local validation includes Apple Silicon and Chrome/WebGPU.

For a visual test of tile and texture swaps, run
`cargo run --release --example terrain_stability`. For CPU selection timings,
set `CESIUM_ION_TOKEN` and run
`cargo run --release --example retention_profile -- 500`.
