# spatial-lib

Generic CPU containers for the baseline's chunked world:

- `chunk::math`: chunk keys (`ChunkKey`, three signed components) and rectangular views of them;
- `grid`: a toroidal cell array that maps an absolute window onto a fixed buffer;
- `chunk::manager`: the active-window and preload policy. It retains chunk payloads in an active
  map and an LRU cache, and emits loads and evictions;
- `chunk::storage`: the `ChunkStorage` trait and, with feature `redb-storage`, its redb backend
  (table `spatial_chunks`).

Each part has a destination in the GPU transition. The key and view math moves into the
framework-free world model. The manager is replaced by residency metadata over GPU slots in prompt
05. The redb backend becomes the saved-world store in 05. Keep new consumers out of this crate.
The crate has no tests of its own; its behavior is exercised through `monarch-engine` and the
legacy fixtures in `morbid-app`.
