# morbid-app

The current desktop application. It composes the Bevy engine with `monarch-engine`'s CPU
simulation, the landscape renderer, the developer tools and chunk persistence. It is the baseline
for the GPU transition, and prompt 06 replaces it with the native client. Nothing else in the
workspace imports it, except the evidence runner's baseline driver.

## Run it

```sh
cargo run --locked --release -p morbid-app
```

The application opens or creates `world_data/save.redb` **relative to the working directory**
before it opens the window. Launched from the repository root, it uses the user's own world. For
experiments, run it from a scratch directory, or use `cargo evidence bench`, which gives every run
a fresh store.

## Composition

`morbid_app::run` builds the interactive application with `build_app(AppOptions::interactive())`.
`AppOptions` selects the world directory, the asset directory, the window size and presentation,
and the input mode. In `InputMode::Scripted`, no keyboard or mouse system changes the world. The
caller sets the observer pose and applies tools through [`baseline`](src/baseline.rs), which also
exposes the work counters (`RenderCounters`, `PersistenceCounters`) and the accounting of retained
cell payloads. Only the evidence runner's `legacy-baseline` driver uses that module.

## Saved data

Chunks are bitcode-encoded `CellChunk`s in the redb table `spatial_chunks`, written only when the
LRU cache evicts them or at exit. See [world format](../../docs/reference/world-format.md).
[`tests/fixtures/legacy-v0`](tests/fixtures/legacy-v0/README.md) holds rows written by this save
path. `examples/capture_legacy_fixtures.rs` produced them.
