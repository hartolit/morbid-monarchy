# GPU programming practice

This practice applies to compute, rendering, transfers and GPU lifetime—not only shader syntax.
The project-specific data and time boundary is the [GPU contract](../../reference/gpu-contract.md).

## Design ownership and layout together

Keep recurring physical data GPU-resident. Admit CPU input as bounded commands or chunk uploads;
return observations or snapshots. Do not introduce a full CPU copy for convenient picking,
collisions, GUI statistics, saving or camera following.

Maintain one authoritative writer per logical output in a phase. A packed cell has two 32-bit
words; independent invocations writing different fields can still race on the same word. Ordinary
writes to the two words are not an atomic cell update. Use an immutable input and single cell-owner
output, or a proved staged reduction. An atomic counter does not make associated payload writes
safe to consume before the required execution boundary.

Specify element size, alignment, stride, field offsets, integer widths, flags, units, endianness and
valid values. `repr(C)`/`Pod`, Rust size checks and WGSL parsing each prove only part of compatibility.
Execute a GPU encode/decode/layout probe using boundary values and padding-sensitive structures.
Avoid host booleans and implicit vector/matrix layout assumptions in shared records.

Do not require native 64-bit shader arithmetic to preserve an eight-byte cell. Two `u32` words are
a suitable baseline. Global totals can exceed 32 bits even when every cell fits; use validated
multiword/reduction arithmetic rather than overflowing diagnostics. Keep layout declarations shared
or generated from one owner, with independent golden cases to catch errors in that owner.

## Name every global dependency

For each pass, state input epoch/version, read/write resources and ranges, dispatch domain,
initialization/reset responsibility, workgroup-local barriers, next global consumer and retirement
point. Keep this account beside the encoder/shader owner, not only in a diagram.

Workgroup/storage barriers do not synchronize different workgroups. Place globally dependent
stages in ordered dispatches/passes with valid resource usage and API-established dependencies.
Do not spin inside a dispatch waiting for another workgroup; not all groups are guaranteed to be
resident together. Do not rely on CPU ECS order as a substitute for GPU command ordering.

Partial edge workgroups still have to meet uniformity requirements. An out-of-range invocation
cannot return early before a barrier that other invocations must reach. Use active masks and
initialized neutral values where collective work requires participation. Keep WGSL validation
and uniformity diagnostics enabled. [Upstream sources](../../reference/upstream-sources.md) own
references to the specification, not a claim that validation proves race freedom.

Scratch data must have a known lifetime. Clear only the region/counters required by the algorithm,
or use a validated epoch scheme. Test repeated dispatch, zero work, empty lists, counter reset,
resize and reuse. Never consume entries beyond a produced count or read stale scratch as current.

## Sparse work is a correctness change

The simplest baseline may write every destination element from immutable input. A sparse version
must preserve every untouched cell and every neighbour dependency. Swapping partially written
buffers without preserving their skipped elements is incorrect.

A supported sparse design can calculate affected outputs into scratch and commit only those
outputs after all required readers finish. The active set must include receive-only halo cells,
all influencing neighbours and longer-range dependencies of the chosen rules. Derive this radius
from actual reads, not the label “local simulation.” Compare dense and sparse results at seams,
negative coordinates, climate changes, edits and moving residency boundaries.

Compact worklists and indirect dispatch counts stay on the GPU. Reading an active count back to the
CPU every tick recreates a synchronization point. A bounded metadata dispatch is acceptable when
its cost is measured. A fully active world remains a required supported case.

## Resolve shared effects, do not race them

Transfer proposals, receiver decisions and cell commits must agree on the exact accepted quantity.
Rejected volume stays at its source. Shared body contacts and material displacement require
accumulation/reduction or per-output gather with defined ordering. Floating-point atomic arrival
order is not a deterministic solver or a conservation argument.

Count and reserve work before applying effects where a finite output pool can overflow. Do not
silently truncate contacts, transfers, visible instances or important events. Prefer prefix-sized
ranges or bounded exact batching to an arbitrary small per-bin list. Any saturation must be
reported with a defined correctness-preserving response, not merely a debug assertion.

Never promise rollback after an in-place mutation. An unrecoverable mid-tick fault must not publish
that tick as completed or save it as a valid checkpoint. Prevent this in normal supported workloads
through admission/preflight and bounded exact processing; retain diagnostic fault handling for
unexpected limits and device failure.

## Share a device, not a copied world

Simulation and rendering use one context and submission timeline. Rendering gets a read-only
presentation view and its identity/lifetime, not ownership to resize or mutate physical buffers.
Separate GPU-derived render buffers are allowed when they have a real presentation purpose and
an invalidation rule. Measure their memory and regeneration work.

Keep simulation, renderer and UI GPU dependencies compatible. Before integrating an egui/wgpu
adapter or other consumer, inspect the resolved dependency tree and actual device types. Two
incompatible wgpu versions cannot share a device merely because their APIs have similar names.

## Transfers and lifetime

Use separate upload/readback staging pools. A buffer cannot be used by the GPU while mapped for
CPU access. Await completion through owned polling/callback integration; do not block the interactive
frame waiting for a small readback. Copy a snapshot into independent staging/storage before its
source slot is reused. Keep buffers and logical IDs alive through their last submitted use.

Every completion carries world/session epoch, request/ticket and slot/body generation as applicable.
Reject delayed work after resize, reload or device recreation. Do not reuse a pool slot until its
mapping is released and prior work is complete or safely ordered by the owning timeline. Reserve
combined staging bytes, not just the number of requests.

Shutdown and explicit maintenance may wait with a timeout and clear outcome. Normal frames may
poll readiness and apply backpressure. Readback-size reduction is not a license to add a per-frame
GPU/CPU rendezvous.

## Validate limits and tooling

Query adapter limits/features and request only those needed. Check buffer allocation size and
storage binding size separately; also validate workgroup size/storage, binding count, offsets,
alignment and dispatch dimensions. A large VRAM total does not establish that a single binding is
legal. Radius 32 at the current cell size already exceeds a 128 MiB single binding.

Validate the complete plan before replacing a working allocation. Account for old/new resources
coexisting during resize. Return a typed unsupported/capacity result; preserve the current world.
Do not turn validation off or silently shrink the workload.

Use the actual assembled shader source for editor diagnostics, offline parsing/validation and
runtime compilation. Preserve useful source maps/include diagnostics. Avoid a custom shader DSL
or string-concatenation framework unless a demonstrated need outweighs it. Standalone WGSL without
Bevy imports is the proposed baseline; another source language requires an explicit toolchain and
maintainability decision, not an opportunistic second implementation.

Run small headless GPU contracts on every changed shader family. CPU fixtures, source searches,
Naga validation and a working display do not establish that physical kernels execute correctly.
Missing hardware is reported as unavailable and blocks GPU qualification.
