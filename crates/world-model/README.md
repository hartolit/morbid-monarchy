# world-model

What the world means, without an engine, a GPU API, a store or a thread pool. Every other package
reads these definitions instead of keeping its own copy.

| Module | Owns |
| --- | --- |
| `cell` | `WorldCell`, the packed 64-bit column; `WorldCell::LAYOUT`, the one table of bit fields; GPU word order (`to_words`, `from_words`); the column rules for inflow, phase changes and quenching. |
| `material` | Material identities stored in the cells, and their properties: repose, crush hardness, combustibility, liquid behavior, ground softness. |
| `spatial` | Chunk size, absolute chunk keys and views, and the two named conversions between render positions and cells. |
| `units` | Units, gravity and the longest body integration step. |
| `config` | Supported ranges and defaults of everything a user, tool or scenario can set. |
| `body` | Body handles, the sphere presets, control strengths and the supported body envelope. |
| `command`, `observation` | Validated intent for the physical world, and what it reports back. |
| `generation` | Deterministic terrain from a seed and an absolute key. |

The dependency closure is `bytemuck`, `glam`, `noise` and `rand`. `cargo evidence check static`
refuses Bevy, wgpu, naga, winit, egui, redb, bitcode, rayon, flume and the other workspace crates
here. Saved bytes are not defined here either: `morbid-app`'s `legacy_format` mirrors the old
writer's encoding, and the versioned format arrives with the store in prompt 05.

Accessors and rules are `#[inline(always)]` because the CPU solvers call them per cell from
another crate. The extraction in prompt 01 kept the cell bytes and every rule unchanged. One known
defect is kept and documented on `WorldCell::quench`: magma that quenches beyond the
elevation ceiling disappears. Prompt 03 replaces it with conservative accounting.

The baseline tests for the column rules moved here with the code and keep their coverage IDs in
[`coverage.toml`](../../tools/evidence/coverage.toml). The layout tests check the field table
against values written out by hand, not against `LAYOUT` itself.
