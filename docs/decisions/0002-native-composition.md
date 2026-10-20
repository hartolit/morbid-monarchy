# 0002 — Native composition and meaningful boundaries

**Date:** 2026-10-10, checked against consumers on 2026-10-12 and 2026-10-13. **State:** the
native application and standalone `bevy_ecs` are the user-requested direction, adopted in prompt
00. 01 created the first package of the map and recorded which others wait for a consumer
([below](#01-package-check)); 02 created the runtime and the GPU owner
([below](#02-package-check)).

## Decision

Retain `bevy_ecs` for CPU orchestration but replace the Bevy engine application/rendering path with
a native application, shared wgpu execution context and renderer. Separate semantic contracts,
runtime policy, GPU execution, persistence mechanism, rendering and application composition where
those boundaries enforce actual ownership or dependency isolation.

Use private modules for material families, body algorithms and pass internals. Do not retain the
old “one crate per distinct feature” rule. Do not make a platform-neutral wrapper around every GPU
handle. GPU execution and rendering can intentionally share GPU-specific types while runtime
intent/persistence contracts remain independent of them.

## Alternatives

Bevy can host GPU compute and a GPU-authoritative simulation; removing it is not a prerequisite
for residency. Retaining its rendering could reduce replacement work. The requested native target
instead favors direct resource control and a smaller application-specific composition. Preserve
useful shaders/math and feature coverage rather than reimplementing everything without inspection.

A single monolithic GPU engine would avoid interface work but combine physical policy, storage,
windowing and rendering. Conversely, a crate per kernel adds API and build friction without useful
independence. The proposed middle boundary map is checked against actual consumers in 01.

## Transition and reversal trigger

Keep the old app only as a temporary baseline until native cutover in 06. Headless GPU tests use
the actual new physical implementation. Remove legacy production flags, engine imports and duplicate
paths after cutover; Git/artifacts retain the baseline. A small CPU reference remains a development
consumer, not an app.

## 01 package check

- **`world-model` — created.** The CPU engine, the application, `spatial-lib` and the evidence
  runner all consume the same cell, material, coordinate, unit, configuration and generation
  definitions, so one framework-free crate replaces their copies. `check static` keeps engines,
  GPU APIs, windows, UI, stores, codecs and worker pools out of its closure.
- **`world-runtime`, `gpu-world`, `world-store`, `world-render` — not created.** Nothing consumes
  `world-runtime` or `gpu-world` before 02, `world-store` before 05 or `world-render` before 06.
  Each is created by the stage that first uses it, not as an empty forwarding crate. 01 recorded
  the runtime ports as signatures; 02 replaced them with the implemented port in the
  [GPU contract](../reference/gpu-contract.md#runtime-ports-implemented-in-02).
- **`spatial-lib` — reduced.** Its key and view math moved to `world_model::spatial`. The toroidal
  grid, the chunk manager and the redb backend stay until 05 replaces them.
- **Window, UI and GPU family — selected, pinned when used.** The lock already resolves wgpu
  29.0.4, winit 0.30.13 and egui 0.35.0 together. `egui-wgpu` 0.35.0 requires wgpu `^29.0` and
  winit `^0.30.13`, and `egui-winit` 0.35.0 requires winit `^0.30.13`, so the native client can
  use one wgpu device for simulation, rendering and UI. 06 adds those crates at these versions;
  `monarchy-evidence` already uses wgpu 29.0.4 for adapter inspection.

## 02 package check

- **`world-runtime` — created** under `crates/`, consumed by the `transfers` suite's headless
  composition and, from 06, the native client. Its only framework is standalone `bevy_ecs`
  (`default-features = false, features = ["std"]`), whose closure is eight `bevy_*` crates and no
  engine, renderer or window crate; `check static` refuses any other.
- **`gpu-world` — created** under `adapters/`, implementing `world-runtime`'s `PhysicalWorld`
  port. It exposes wgpu types at its GPU-specific edges (`WorldView`, `submit_external`) for
  consumers on the same device, as decided above.
- **Not created.** `world-store` waits for 05 and `world-render` for 06. The runtime's storage
  port is created with the store.

## 05 package check

- **`world-store` — created** under `adapters/`, implementing `world-runtime`'s `WorldStore`
  port over one redb file per world. Its consumers: the runtime's storage workers inside
  `WorldSession`, the `world-store` command, the evidence runner's `streaming` suite and
  `session` command, and the baseline application, whose `spatial_chunks` rows `world_store::legacy`
  now owns until 06. Its closure is redb, bitcode (the legacy rows only), `world-model` and
  `world-runtime`; `check static` refuses engines, GPU APIs, windows, UI, worker pools and the
  baseline crates in it, and refuses `world-store` in the closures of `world-model`,
  `world-runtime` and `gpu-world`.
- **The storage port lives in `world-runtime`** (`store`), beside the workers, residency and
  checkpoint owners that call it, so the runtime is tested against an in-memory store and the GPU
  owner never sees storage.
- **`spatial-lib` — reduced again.** Its redb backend moved to `world_store::legacy`; the grid and
  chunk manager serve only the baseline until 06.
- **Not created.** `world-render` waits for 06.

## 06 package check

- **`world-render` — created** under `crates/`, consumed by the native client and the evidence
  runner's `render` suite and visible scenario. It reads `gpu-world`'s view and records into the
  GPU owner's encoders on its timeline; it imports `world-runtime` only for identities and the
  resident window. `check static` refuses windows, UI, stores, engines and the baseline in its
  closure.
- **`morbid-app` — moved and replaced.** The package name now belongs to the native client at
  `apps/morbid` (library `morbid_app`, executable `morbid`): winit 0.30.13, egui, egui-wgpu and
  egui-winit 0.35.0 over the one wgpu 29.0.4 device the GPU owner opens, as the 01 check
  foresaw. Its closure has no Bevy engine crate; the standalone ECS arrives only through
  `world-runtime`.
- **Removed.** The Bevy baseline application, `monarch-engine` and `spatial-lib` (its grid and
  chunk manager, superseded by the residency owner), with no fallback flag; `9a3230c` is the last
  commit that builds them. The CPU reference of the cell rules stays in `monarchy-evidence`.

If a proposed package has no actual boundary/consumer, use a module before creating it. A dependency
cycle, duplicate wgpu device family or interface forcing world copies is evidence that the chosen
boundary needs correction.
