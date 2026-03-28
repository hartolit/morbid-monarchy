# Project Architecture & Constraints (Morbid Monarchy)

## Workspace Shape
1. **`monarch-engine`**: Pure Rust core crate. It is the source of truth for reusable engine/domain logic and must remain free of application bootstrap and environment-specific orchestration.
2. **`morbid-app`**: Pure Rust application crate. It is a thin consumer of `monarch-engine` and owns startup, runtime orchestration, boundary I/O, and user-facing execution flow.
3. **Future crates**: Add `mm-server`, `mm-cli`, `mm-shared`, or `mm-protocol` only when a concrete boundary requires them. Shared vocabulary crates must remain free of business logic.

## Current Reset State
- The repository is intentionally reduced to a barebones Cargo workspace.
- Preserve the crate directories and the dependency link from `morbid-app` to `monarch-engine`.
- Rebuild new functionality from this minimal baseline rather than reviving deleted implementation.

## Golden Standards (CRITICAL)
- **Dependency Direction:** `monarch-engine` must not depend on application crates. Application crates may depend on `monarch-engine`.
- **Core Purity:** Keep `monarch-engine` deterministic and portable. Avoid leaking process concerns, filesystem coupling, or presentation concerns into core domain code.
- **Configuration Discipline:** Do not scatter magic numbers. Prefer `Default`, associated constants, or dedicated config types. Use typed `TOML` configuration only when runtime configurability is genuinely required.
- **Idiomatic Rust:** Use explicit types, `Result`/`?`, ownership and borrowing, iterator-driven transforms, and narrow error boundaries. Avoid `unwrap`/`expect` outside tests or impossible invariants.
- **Module Cohesion:** Split modules by semantic responsibility, not arbitrary size targets. Keep cohesive concepts together and separate only when a real boundary appears.
- **Pure Rust Focus:** Do not introduce non-Rust runtimes, cross-language bindings, or mixed-runtime assumptions unless the user explicitly changes project scope.