# world-runtime

CPU orchestration of the live world, independent of the GPU API. It owns the identities that
cross the boundary (world, session epoch, command, tick, revision), the `PhysicalWorld` port the
GPU owner implements, command admission with retry and conflict rules, the fixed logical clock,
and the consumer of what the live world reports.

| Module | Owns |
| --- | --- |
| `ids` | `WorldId`, `SessionEpoch`, `CommandId`, `Tick`, `ChunkRevision`, and their two-word form. |
| `port` | `PhysicalWorld` and its batches, tickets, typed refusals, outcomes and counters. |
| `receipts` | Bounded receipts: a retry with the same identity applies once, reuse with another command conflicts, acknowledged identities expire. |
| `ledger` | The outcome consumer: important results (receipts, residency) are reserved and kept; cosmetic events pass a bounded ring with every drop counted. |
| `clock` | The fixed tick clock and its catch-up cap. |
| `runtime` | A standalone `bevy_ecs` world and schedule (`advance_clock → submit_due_ticks → poll_world → consume_outcomes`) over a boxed port. |

The composition root chooses the port: `cargo evidence test gpu` composes it with
`gpu_world::world::GpuWorld` headlessly today, and the native application does the same with a
window in prompt 06. `cargo evidence check static` keeps GPU, window, UI, store and engine
crates out of this package's dependency closure; the standalone ECS is its only framework.
