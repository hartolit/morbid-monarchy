# Develop and inspect Morbid Monarchy

## Baseline versus target

The supplied archive's application package is `morbid-app` at `crates/morbid-app`; its startup entry
is `morbid_app::run`. Its manifest currently requests Bevy and it opens `world_data/save.redb`
relative to the working directory. The baseline has no versioned world CLI, locked dependency set
or verified setup recipe. Work on a copy of user saves, not the only database.

The target keeps the application package name but moves its composition to `apps/morbid`.
`world-runtime` coordinates CPU intent/I/O; `gpu-world` simulates; `world-render` displays the same
GPU state. The package README and actual parser will own exact runtime options after 06.

## Establish the toolchain

Prompt 01 checks the available Rust toolchain and dependencies, selects compatible supported
versions and commits `rust-toolchain.toml` and `Cargo.lock`. Do not follow version numbers from the
old persona/rules files as though they were tested pins. Inspect the dependency tree before
choosing wgpu/window/UI versions: all GPU consumers must share compatible device types.

The maintained checks are in [workflow](../development/workflow.md). The planned `cargo evidence`
commands are not present until 01 implements them. Run `inspect-adapter` after that implementation,
record its returned adapter ID and use it explicitly for GPU tests. A missing GPU blocks hardware
qualification, not source/document work.

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

Trace a brush through input validation, command admission, GPU ray resolution, ordered cell edit,
reactivation, returned result, rendered change and dirty revision. Read the source owner at each
boundary rather than starting from every crate's type inventory. The connected `resident-world`
scenario provides automated coverage of that same physical/storage route.

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
added here with tested examples then. They are intentionally not invented in this package.
