# gpu-world

The live world on the GPU. One `GpuWorld` is one session on one device: it opens the adapter
named by its stable identifier, requests the limits the largest supported plan needs, allocates
a checked resource plan, compiles its kernels from assembled WGSL, and executes ticks of ordered
commands against resident chunks. It implements `world_runtime::port::PhysicalWorld`; the
runtime decides what to ask and when, and this crate decides how the device does it.

| Module | Owns |
| --- | --- |
| `context` | Adapter selection, the device, granted limits and features, loss and uncaptured-error health, counted buffer creation, the one submission timeline (at most three submissions in flight). |
| `plan` | Every buffer's size, usage and binding for a window, checked against device limits and the budgets before anything is allocated; old/new overlap of a resize. |
| `layout` | The GPU records (`gpu_record!`), cell field constants derived from `world_model::cell::WorldCell::LAYOUT`, codes, and the generated WGSL declarations. |
| `shader` | WGSL assembly from `shaders/` with a line map, naga validation and file/line diagnostics. |
| `resources` | Device buffers, pipelines (compiled once per session) and the bind group. |
| `staging` | Upload and readback pools: reservation generations, map delivery mailbox, reuse. |
| `slots` | Chunk slots, the window map and slot records (metadata only). |
| `commands`, `readback` | Command records in window-local coordinates; identity-checked decoding of results, events and timestamps. |
| `world` | `GpuWorld`: tick encoding (`world/tick.rs`), nonblocking polling, loss and the orderly stop (`world/poll.rs`), residency operations and resize. |
| `faults` | Fault injection for conformance runs only: device loss, held or reordered deliveries, held completions, event floods. |

Shaders live in `shaders/` (`shared/` cell accessors, `world/` the command kernels, `faults/`
the pressure-injection kernel). Each assembly starts with the generated declarations, so a
layout has one owner. `cargo evidence check static` validates every assembly and writes the
assembled sources to its run directory; `cargo evidence test gpu --suite transfers` executes
them on an explicit adapter.

The command kernels in this stage are the material brush (`apply_edit`, the baseline tools'
column rules) and the column query. Material simulation (prompt 03) and bodies (04) add kernels
here and reuse the same tick, command and observation path.
