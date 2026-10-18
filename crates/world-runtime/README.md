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

The composition root chooses the port: `cargo evidence test gpu` composes it with
`gpu_world::world::GpuWorld` headlessly today, and the native application does the same with a
window in prompt 06. `cargo evidence check static` keeps GPU, window, UI, store and engine
crates out of this package's dependency closure; the standalone ECS is its only framework.
