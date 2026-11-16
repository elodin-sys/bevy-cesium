# Changelog

## 0.6.0 — unreleased

- Add configurable reference ellipsoids, a Mars preset and dataset-specific spheres.
- Share planetary terrain decoding and hierarchy selection between native and WASM.
- Keep WGS84 as the default and document planetary coordinate/datum requirements.
- Breaking: `Options` adds `ellipsoid`; `NativeBackend::Prepared` becomes `Rust`.
