# bevy-tiles

Shared Bevy scene contract and renderer used by `bevy-cesium` and `caevy`.
No Cesium Native, CXX or network dependency.

Backends implement `Backend::open` and `Backend::update`, returning owned
geometry, material properties, visibility changes, credits and loading status.
`TilesPlugin<B>` creates the Bevy entities. Applications own lighting,
environment maps, exposure, tonemapping, credential storage and attribution UI.

Existing `Cesium*` scene types are retained for application compatibility.
Caevy re-exports these same types under `Caevy*` names. Use one backend per
application. A common interface is not a promise of identical format support,
tile selection, cache accounting or GPU output.

Coordinates are ECEF meters in double precision. Hosts can provide a local
transform or supply views and place geometry through the external-coordinate
adapter. Base-color texture bytes are sRGB; material factors and emissive values
are linear. Normals and UVs are supplied by the backend.
