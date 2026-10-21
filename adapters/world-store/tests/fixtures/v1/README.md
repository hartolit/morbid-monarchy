# Saved-world fixture, format version 1

A world written by the version-1 store, kept so that every later build is tested against bytes an
earlier build wrote: `tests/v1_fixture.rs` reads it exactly, continues it in a writing session
of a copy, and refuses damaged and unsupported copies without changing them. Treat it as
immutable. A capture made with changed code is a new fixture set, not an update to this one; a
format change keeps this file opening or migrates a copy of it.

## Provenance

| Item | Value |
| --- | --- |
| Writer | `examples/capture_v1_fixture.rs` through `RedbWorldStore`'s own operations (create, `begin_session`, `write_chunks`, `write_bodies`, `publish`, `end_session`), at commit `0ec68c5` plus the program and `content.rs` added with this directory |
| Command | `cargo run --locked -p world-store --example capture_v1_fixture -- world-v1.redb` |
| Toolchain / libraries | rustc 1.99.0 (`b940084d7`), redb 4.1.0 (from `Cargo.lock`) |
| Format | version 1; cell format, layout fingerprint, physics revision and fingerprint and generator revision of that commit (`world_model::identity`) |
| Reproducibility | Two captures produced byte-identical files |
| Size, SHA-256 | 335872 bytes, `bd92b98a8609914ff408ab84ca75d47c3dfd3d4031e9f2848a6d6b973518e207` |

## Content

`content.rs` computes everything the file holds; the tests compare the store's reads with it,
not with the store's own decoding.

| Checkpoint | Tick | Epoch | Holds |
| --- | ---: | ---: | --- |
| 1 | none | 1 | The created world: seed 69, rule seed `0x5EED0F1C`, window radius 1 at the origin, every rule, 20 °C, elevation scale 0.5; no chunk, no body. |
| 2 | 600 | 2 | Revision 1 of chunks (0, 0), (−1, −1) and (1, 0); a body set with a live slot at generation 4, a free one, a live one at generation 0 and a retired one, two bodies with every field set. |
| 3 | 1,234,567 | 3 | Revision 2 of chunk (0, 0), superseding revision 1, which reclamation removed; fire off, −4.5 °C, elevation scale 0.75; the body set again with other values. The session then closed in order. |

Chunk cells are SplitMix64 words seeded by key and revision, with cell 0 all zeros, cell 1 all
ones and cells 2 and 3 one 32-bit word each, so a swapped word or field in a reader shows.
