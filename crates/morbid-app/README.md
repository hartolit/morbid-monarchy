# bevy-baseline

The Bevy application the GPU transition started from (package `bevy-baseline`, formerly
`morbid-app`). It composes the Bevy engine with `monarch-engine`'s CPU simulation, the landscape
renderer, the developer tools and chunk persistence. Prompt 06 replaced it with the native
client (`apps/morbid`, package `morbid-app`) and removes it; until then only the evidence
runner's baseline driver imports it.

## Run it

```sh
cargo run --locked --release -p bevy-baseline
```

The application opens or creates `world_data/save.redb` **relative to the working directory**
before it opens the window. Launched from the repository root, it uses the user's own world. For
experiments, run it from a scratch directory, or use `cargo evidence bench`, which gives every run
a fresh store.

## Composition

`bevy_baseline::run` builds the interactive application with `build_app(AppOptions::interactive())`.
`AppOptions` selects the world directory, the asset directory, the window size and presentation,
and the input mode. In `InputMode::Scripted`, no keyboard or mouse system changes the world. The
caller sets the observer pose and applies tools through [`baseline`](src/baseline.rs), which also
exposes the work counters (`RenderCounters`, `PersistenceCounters`) and the accounting of retained
cell payloads. Only the evidence runner's `legacy-baseline` driver uses that module.

## Saved data

Chunks are rows of the redb table `spatial_chunks`, written only when the LRU cache evicts them or
at exit. Since 05 `world_store::legacy` owns their bytes and the table (`LegacyStore`), tested
against the rows the old writer wrote
([`legacy-v0`](../../adapters/world-store/tests/fixtures/legacy-v0/README.md));
[`legacy_format`](src/legacy_format.rs) only converts between them and `CellChunk`. A row that
cannot be read or decoded is left unloaded and reported, never replaced by generated terrain. The
baseline's other persistence failures (saving on eviction only, stale reads while a save is
pending, an exit flush of the save queue alone) remain until prompt 06 replaces it with the
runtime's residency and checkpoints over `world-store`. See
[world format](../../docs/reference/world-format.md).
