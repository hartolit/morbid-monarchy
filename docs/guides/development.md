# Develop and inspect Morbid Monarchy

## The application

The application is the native client, package `morbid-app` at `apps/morbid`, executable `morbid`
(06). It composes `world-store`, the runtime's `WorldSession`, `gpu-world` and `world-render`
over one GPU device; the [package README](../../apps/morbid/README.md) owns its options, controls
and the frame. The Bevy baseline that preceded it was removed in 06 (`9a3230c` is the last
commit that builds it).

```sh
cargo run --locked --release -p morbid-app --bin morbid
```

Worlds are `<name>.world` files in `$XDG_DATA_HOME/morbid-monarchy/worlds` (or
`~/.local/share/morbid-monarchy/worlds`) and the settings file is
`$XDG_CONFIG_HOME/morbid-monarchy/client.toml`; nothing is read or written relative to the
working directory, so the client runs the same from anywhere. The GPU adapter is chosen
explicitly (`--adapter`, the settings file or the start screen) and never falls back to another.

`world_data/save.redb` at the repository root is a save the baseline wrote. The client never opens
it; the start screen's import makes a new world from it (give seed 69 and elevation scale 0.5, the
baseline's), and repairs a copy if the original was not closed cleanly. Experiments, fixtures and
benchmarks use temporary stores: the evidence runner gives every run a new directory.

## Establish the toolchain

`rust-toolchain.toml` pins Rust 1.99.0 and `Cargo.lock` is tracked (01); rustup installs the
toolchain on first use, and every build uses `--locked`. The static lane also needs the pinned
dependency checkers:

```sh
cargo install --locked cargo-deny@0.20.2 cargo-machete@0.9.2
```

On Linux the window stack loads Wayland or X11 and xkbcommon at run time; CI installs
`pkg-config libwayland-dev libxkbcommon-dev` for the build. The window, UI and GPU versions were
chosen from the resolved APIs: see
[ADR 0002](../decisions/0002-native-composition.md#01-package-check). All GPU consumers share one
wgpu.

The maintained checks are in [workflow](../development/workflow.md):

```sh
cargo evidence check docs
cargo evidence check static
cargo evidence test cpu
cargo evidence inspect-adapter
cargo evidence test gpu --suite all --adapter vulkan-10de-2c05
cargo evidence run --scenario resident-world --adapter vulkan-10de-2c05 --consumer native
```

`inspect-adapter` prints a stable identifier for every adapter (`vulkan-10de-2c05` is the RTX 5070
Ti on the nominated machine). Pass it explicitly with `--adapter` to GPU suites, scenarios and
benchmarks; the runner never picks one. A missing GPU blocks hardware qualification, not
source/document work. Runs that open the native window (`--consumer native`, `bench`, the
`native-client` scripts) put a window on the desktop; leave it alone while they run.

## Inspect one operation

A brush stroke in the client: `play` turns the held button into one `Command::EditMaterial` per
owed tick at the cell a GPU-resolved `Command::QueryRay` hit, admitted under a receipt
(`Runtime::submit_at`). The session's frame hands due commands to `GpuWorld::submit_tick`, the
`apply_edit` kernel (`gpu-world` `shaders/world/edit.wgsl`) changes the cells and wakes their
tiles, the material phases continue from there, and `poll` decodes the result into the receipt
through the ledger, where `play` reads it for the log. The changed slot's change word makes the
next checkpoint read the chunk back; the renderer's next frame draws the changed cells from the
same buffers, ordered after the tick on the same queue (`render` `edit-visible`). Read the source owner at each boundary rather than starting
from every crate's type inventory. `resident-world` automates the physical and storage route.

For performance, select a frozen profile and follow
[the evidence protocol](../development/verification-evidence.md): `cargo evidence bench` measures
the complete native frame in the client's window. Use pass timings (the diagnostic run, the
`render` suite's frame cost) to locate a limit, then confirm the complete application. Keep
profiler capture and diagnostic modes labelled; a debug overlay that performs extra readback is
not the normal performance profile.

## Save/recovery behavior

The client saves on request (F5), on the autosave interval and when a world closes, and reports
whether the close saved the last completed tick. A failed write keeps its payload and is retried;
corrupt data is refused, never treated as an absent chunk. After a device loss the client shows
the latest complete checkpoint and how many ticks were lost, and recovery opens a new session
from that checkpoint on a new device. Outside the client, `world-store` creates, inspects and
imports stores (`world-store --help`), and `cargo evidence session` opens one headless; see
[world format](../reference/world-format.md#operating-a-saved-world-05).
