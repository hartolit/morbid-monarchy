# Agentic Changelog

## 2026-04-03
- Replaced the placeholder engine world store with a toroidal active pixel grid in `monarch-engine`, including active-window config, deterministic chunk generation, world-state movement, and persistence-facing chunk/entity data types.
- Replaced the placeholder `morbid-app` bootstrap with a working Bevy example that renders the active pixel window into an `Image`, supports keyboard-driven player movement, and shows the player position over the streamed grid.
- Removed the old engine plugin shell so `morbid-app` owns Bevy startup and rendering while `monarch-engine` remains the authoritative source for world-state structures.
- Split the toroidal runtime into a larger loaded buffer and a centered visible window so the player no longer snaps backward when chunk recentering occurs.
- Added app-side async chunk streaming with `AsyncComputeTaskPool` and `bitcode` chunk files under `runtime/world/chunks`, while keeping filesystem access out of `monarch-engine`.
- Replaced the side-view procedural terrain with a smarter top-down biome generator that produces dirt, rock, and water regions suitable for RimWorld-style world traversal.
- Fixed a debug overflow panic in the top-down noise generator by using wrapping seed arithmetic and added regression tests for chunk generation at large world coordinates.

## 2026-03-28
- Reset the repository to a refreshed barebones Cargo workspace.
- Preserved only the two crate shells, the workspace manifests, minimal architecture/changelog docs, and the dependency link from `morbid-app` to `monarch-engine`.
- Replaced `monarch-engine` with a minimal no-op plugin skeleton and reduced `morbid-app` to a minimal Bevy app that depends on that plugin.

## 2026-03-29
- Restored the layered hybrid dependency baseline: `monarch-engine` now carries the pure ECS/math/serialization/randomness stack for authoritative simulation types, while `morbid-app` retains full Bevy plus persistence formats to render engine-owned pixel data through Bevy images and sprites.