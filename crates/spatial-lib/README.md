# spatial-lib

Generic CPU containers for the baseline's chunked world:

- `grid`: a toroidal cell array that maps an absolute window onto a fixed buffer;
- `chunk::manager`: the active-window and preload policy. It retains chunk payloads in an active
  map and an LRU cache, and emits loads and evictions;
- `chunk::storage`: the `ChunkStorage` trait and, with feature `redb-storage`, its redb backend
  (table `spatial_chunks`). It stores bytes; `morbid-app`'s `legacy_format` defines them.

Chunk keys and views are `world_model::spatial` types; prompt 01 moved them out of this crate.
Each remaining part has a destination in the GPU transition. The manager is replaced by residency
metadata over GPU slots in prompt 05. The redb backend becomes the saved-world store in 05. Keep
new consumers out of this crate. The crate has no tests of its own; its behavior is exercised
through `monarch-engine` and the legacy fixtures in `morbid-app`.
