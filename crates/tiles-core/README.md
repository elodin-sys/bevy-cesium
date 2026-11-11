# tiles-core

Internal Rust loader for the Bevy-Cesium browser backend and prepared packages
on native platforms. Other native sources continue to use Cesium Native.

Imported from `vendor/tiles-core` in
[`elodin-terrain-demo` at 1896eac25355841dd725013e9579b95680be6162](https://github.com/elodin-sys/elodin-terrain-demo/tree/1896eac25355841dd725013e9579b95680be6162/vendor/tiles-core).
The original MIT/Apache-2.0 licenses are retained. The demo records its earlier
core provenance as `94b1419ccfa4d52d119a8e0ff1b5312b1533200c`.

Supports explicit 3D Tiles hierarchies, nested tilesets, glTF/GLB including
Draco meshes, and bounded quantized-mesh terrain with optional geographic
TMS imagery. This is not the full Cesium Native format/feature set.

Run `cargo test -p tiles-core` from the repository root. Fixtures are shared
with the native backend in `tests/data/`. No ion credentials are needed.
