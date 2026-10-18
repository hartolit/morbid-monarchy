# world-runtime

CPU orchestration of the live world, independent of the GPU API. It owns the identities that
cross the boundary (world, session epoch, command, tick, revision), the `PhysicalWorld` port the
GPU owner implements, command admission with retry and conflict rules, the fixed logical clock,
and the consumer of what the live world reports.

| Module | Owns |
| --- | --- |
| `ids` | `WorldId`, `SessionEpoch`, `CommandId`, `Tick`, `ChunkRevision`, and their two-word form. |
| `port` | `PhysicalWorld` and its batches (with the tick rate and the bodies to sample), tickets, typed refusals, outcomes and counters, including each tick's material accounting (`CellReport`) and body accounting (`BodyReport`). |
| `receipts` | Bounded receipts: a retry with the same identity applies once, reuse with another command conflicts, acknowledged identities expire. |
| `ledger` | The outcome consumer: important results (receipts, residency) are reserved and kept; cosmetic events pass a bounded ring with every drop counted; ticks whose material or body accounting does not balance are counted; body outcomes go to the body monitor. |
| `bodies` | The body monitor: which bodies exist, their latest samples and how old they are, the selection of bodies to sample (the controlled body every tick, the others in turn), and body events derived from the device's counters against per-body watermarks, deferred rather than dropped when the bounded ring is full. |
| `clock` | The fixed tick clock, its catch-up cap and the discarded backlog; reconfigurable within the supported ranges. |
| `runtime` | A standalone `bevy_ecs` world and schedule (`advance_clock → submit_due_ticks → poll_world → consume_outcomes`) over a boxed port; commands may name the earliest tick they apply in. |
| `store` | The storage port (`WorldStore`) the saved-world store implements: chunk payloads and revisions, the world header, published checkpoint manifests, body sets, and typed failures that keep absent, corrupt, unsupported and unreadable data apart. |
| `workers` | Bounded threads between the owners and the store: loaders that read or generate chunks, one writer that makes payloads, body sets and publications durable in order. |
| `residency` | Which chunks are resident and where every chunk's newest content is (retained, durable, published or generated); loads that only an absent key may answer with generation, stale results discarded, outgoing chunks snapshotted and retired together, window moves, byte budgets and visible pressure. Metadata only: no resident cells. |
| `checkpoint` | Saves (requested, periodic, closing) through one owner: a world snapshot at one completed tick, the revisions and bodies it holds, publication once they are durable, receipts and retries. |
| `session` | One saved world's live session: open from a store's latest checkpoint, frames, the loss report after a lost device, and the orderly close with its honest result. |

The composition root chooses the ports: `cargo evidence test gpu` composes the runtime with
`gpu_world::world::GpuWorld` and, since 05, `world_store::store::RedbWorldStore` headlessly
(`GpuWorldConfig::resume` opens the live world at a saved checkpoint), and the native application
does the same with a window in prompt 06. `cargo evidence check static` keeps GPU, window, UI,
store and engine crates out of this package's dependency closure; the standalone ECS is its only
framework.
