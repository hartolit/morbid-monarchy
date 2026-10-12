# Develop and inspect Morbid Monarchy

## Baseline versus target

In the current checkout the application package is `morbid-app` at `crates/morbid-app`, with startup
entry `morbid_app::run`. Its manifest requests Bevy. On startup, before the window opens, it opens
`world_data/save.redb` relative to the working directory, and `Database::create` creates the file
if it is missing. The baseline has no versioned world CLI, tracked dependency lock or verified
setup recipe.

The `world_data/save.redb` at the repository root is the user's only world. Never launch an
experiment, fixture capture or benchmark from the repository root. Run the baseline from a scratch
working directory, so that it opens a disposable `world_data/`, and capture old-writer fixtures
into temporary stores. If a task genuinely needs the user's data, work on a copy; never open or
write the original.

The target keeps the application package name but moves its composition to `apps/morbid`.
`world-runtime` coordinates CPU intent/I/O; `gpu-world` simulates; `world-render` displays the same
GPU state. The package README and actual parser will own exact runtime options after 06.

## Establish the toolchain

`rust-toolchain.toml` pins Rust 1.99.0 and `Cargo.lock` is tracked (01); rustup installs the
toolchain on first use, and every build uses `--locked`. The static lane also needs the pinned
dependency checkers:

```sh
cargo install --locked cargo-deny@0.20.2 cargo-machete@0.9.2
```

The window, UI and GPU versions are chosen from the resolved APIs, not guessed: see
[ADR 0002](../decisions/0002-native-composition.md#01-package-check). All GPU consumers share one
wgpu.

The maintained checks are in [workflow](../development/workflow.md):

```sh
cargo evidence check docs
cargo evidence check static
cargo evidence test cpu
cargo evidence inspect-adapter
```

`inspect-adapter` prints a stable identifier for every adapter (`vulkan-10de-2c05` is the RTX 5070
Ti on the nominated machine). Pass it explicitly with `--adapter` to GPU suites and benchmarks; the
runner never picks one. A missing GPU blocks hardware qualification, not source/document work.

## Target interactive operation

After 06, the native application must provide an ordinary launch path, explicit world/configuration
selection and visible save/device status. A development launch remains:

```sh
cargo run --locked --release -p morbid-app
```

This command is a target recipe until the native app and its defaults are implemented and tested.
The app must find packaged assets independently of the current directory, display unsupported
adapter/allocation requests as actionable errors, and never silently choose a CPU simulation.

The user can navigate/follow a body, spawn wood/metal spheres, possess/control/jump, attract/lift,
edit terrain/sand/water/oil/magma/ice/fire, change supported simulation and lighting settings, and
resize the active world safely. Cursor previews and statistics must not require a full CPU terrain
mirror. The UI distinguishes submitted edits, applied results and durable saves.

## Inspect one operation

In the baseline, a brush runs `handle_brush_input`, then `raymarch_grid` against the CPU grid, then
`set_cell`/`wake_cell`; the next render sync uploads the whole grid. The target route (04–06) is
input validation, command admission, GPU ray resolution, ordered cell edit, reactivation, returned
result, rendered change and dirty revision. Read the source owner at each boundary rather than
starting from every crate's type inventory. The connected `resident-world` scenario, built up from
02 through 07, will automate that physical/storage route; it does not exist yet. Today the baseline
driver issues the same `world_model` commands from the frozen scenarios.

For performance, select a frozen profile and follow [the evidence protocol](../development/verification-evidence.md).
Use pass timings to locate a limit, then confirm the complete application. Keep profiler capture
and diagnostic modes labelled; a debug overlay that performs extra readback is not the normal
performance profile.

## Save/recovery behavior

Checkpoint and legacy-import operations must work on explicit paths and expose progress/errors.
An orderly save includes still-resident dirty terrain and bodies. Failed writes retain their
payload; corrupt data is not treated as an absent chunk. After device/process failure, recovery
states the last completed durable checkpoint and possible loss since it.

The exact import/recovery commands are delivered with the storage/app parser in 05–06 and must be
added here with tested examples then. They are intentionally not invented in these docs.
