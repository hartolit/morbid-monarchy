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
| `bodies` | The body slot table: generation-bearing handles, lowest-free allocation, retirement of an exhausted generation, the live extent. |
| `commands`, `readback` | Command records in window-local coordinates (rays included) and the body table's part in encoding them; identity-checked decoding of results, ray answers, body samples and diagnostics, events and timestamps. |
| `world` | `GpuWorld`: tick encoding (`world/tick.rs`), nonblocking polling, loss and the orderly stop (`world/poll.rs`), residency operations and resize. |
| `faults` | Fault injection for conformance runs only: device loss, held or reordered deliveries, held completions, event floods. |

Shaders live in `shaders/`: `shared/` the cell accessors, the rules' hashes and shuffles
(`rules.wgsl`), the column rules (`column.wgsl`) and window and tile addressing (`window.wgsl`);
`world/` the command kernels; `sim/` the material phases; `body/` the bodies (integration and
suspension, bins and the pair solver, terrain contact, deformation and displacement, rays and
body commands, observation); `faults/` the pressure-injection kernel. Each assembly starts with the generated declarations, which also carry the material
tables and rule constants generated from `world-model`, so neither a layout nor a rule constant
has a second copy. `cargo evidence check static` validates every assembly and writes the
assembled sources to its run directory; `cargo evidence test gpu --suite transfers`, `--suite
cells` and `--suite bodies` execute them on an explicit adapter.

A tick is four compute passes: the ordered commands (brush edits, column queries, totals
measurements, rays and body commands; configuration commands change the tick's parameters),
activity scheduling (which
8 × 8 tiles to visit and each cell's wake level, with the counts and indirect dispatch arguments
on the device), and the material phases `propose`, `resolve`, `react`, `commit` (granular flow,
liquids, erosion, thermal phase changes, quenching, boiling, combustion and growth). The pass and
resource table is in `docs/reference/gpu-contract.md` ("Cell phases"), and the body substeps
(integration, bins, the pair passes, terrain contact and deformation, displacement), one
compute pass after `commit`, described under "Bodies (04)". `GpuWorldConfig` chooses the starting
rules and climate, the rule seed, the first tick, sparse or dense execution (dense is the
maintained reference for the sparse scheduling), the elevation scale, the body broadphase (bins,
or all pairs for comparison) and the solver's position and velocity passes.
