# Agentic Changelog

## 2026-03-28
- Reset the repository to a refreshed barebones Cargo workspace.
- Preserved only the two crate shells, the workspace manifests, minimal architecture/changelog docs, and the dependency link from `morbid-app` to `monarch-engine`.
- Replaced `monarch-engine` with a minimal no-op plugin skeleton and reduced `morbid-app` to a minimal Bevy app that depends on that plugin.

## 2026-03-29
- Restored the layered hybrid dependency baseline: `monarch-engine` now carries the pure ECS/math/serialization/randomness stack for authoritative simulation types, while `morbid-app` retains full Bevy plus persistence formats to render engine-owned pixel data through Bevy images and sprites.