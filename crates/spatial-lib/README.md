# spatial-lib

Generic CPU containers for the baseline's chunked world:

- `grid`: a toroidal cell array that maps an absolute window onto a fixed buffer;
- `chunk::manager`: the active-window and preload policy. It retains chunk payloads in an active
  map and an LRU cache, and emits loads and evictions.

Chunk keys and views are `world_model::spatial` types; prompt 01 moved them out of this crate.
Prompt 05 moved the redb backend of the baseline's `spatial_chunks` table to `world-store`'s
`legacy` module, its one owner, and replaced the manager in the new runtime with
`world_runtime::residency`, which keeps metadata over GPU slots instead of cell arrays. What is
left serves only the Bevy baseline and goes with it in prompt 06. Keep new consumers out of this
crate. The crate has no tests of its own; its behavior is exercised through `monarch-engine`.
