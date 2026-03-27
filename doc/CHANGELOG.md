# Agentic Changelog

*Directive for AI:* When adding an entry, summarize older entries to maintain a maximum of 5 days of recent architectural changes. This file must remain concise to save token context. 

## 2026-03-26
- Normalized `.windsurfrules` to a pure Rust, Cargo-workspace-focused instruction set centered on `*-core`, thin app crates, Rust idioms, and strict boundary enforcement.
- Replaced stale mixed-stack architecture guidance in `doc/architecture.md` with the actual `mm-core` / `mm-app` workspace shape and Rust-only golden standards.
- Cleaned `doc/execs/` workflows so cleanup, restructure, newborn, audit, and idea execution paths reference Cargo validation, Rust diagnostics, and crate/module ownership rather than mixed-runtime or frontend concerns.
- Renamed the application crate from `mm-client` to `mm-app`, including the workspace member, Cargo package/library names, and crate path.
- Established the first ECS-native gameplay baseline: `mm-core` now owns player/movement components, config, simulation step, and movement systems via a reusable core plugin, while `mm-app` boots Bevy, translates keyboard input into core intent, spawns the camera/player scene, and renders the same ECS state with workspace validation and smoke tests passing.
- Extended the baseline to use a 3D-aware core movement model with planar motion and preserved Z indexing, and replaced placeholder visuals with the provided player sprite sheet plus pale ground/water tile atlas in `mm-app`.
- Removed the temporary hardcoded terrain/tile-map prototype from `mm-app` and documented a new chunked procedural world direction centered on a dedicated `mm-core::world` domain with shared chunk coordinates and split world/entity spatial indexing.
- Refined the procedural-world first slice to use hard chunk-theme boundaries, a 3x3 active chunk window, debug-first rendering, and `mm-app`-owned flat-file persistence over pure `serde` world models (`RON` for debug-oriented development, `bitcode` for release-oriented builds).
- Fully locked the procedural-world planning phase around coarse base-theme chunk output plus deterministic `ProcAsset` stamps, with strict per-chunk seeded RNG and order-independent generation requirements from the start.
- Executed the first procedural-world slice: added `mm-core::world` with planar chunk vocabulary, deterministic coarse-base generation, mutation overlays, and a pure world store; wired `mm-app` for chunk streaming, debug-gizmo rendering, live entity chunk indexing, and flat-file chunk persistence; and validated the new slice with passing workspace checks and world-specific tests.
- Consolidated the over-fragmented `mm-core::world` model into a cohesive `types.rs`, normalized `bitcode` and RNG support through workspace dependencies, deleted the dead top-level `spatial/` tree and obsolete world leaf files, and revalidated the workspace with clean checks and tests.