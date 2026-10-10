# Upstream technical sources

Reviewed on 2026-10-10. These primary sources support external API constraints, not claims about
Morbid Monarchy's executed performance. 01 selects and pins a mutually compatible toolchain/library
set; “latest” documentation below is not a dependency pin. Refresh version-specific references when
adopting the selected APIs.

## S1 — WGSL semantics

[W3C WGSL specification](https://www.w3.org/TR/WGSL/), particularly memory layout, scoped operations,
uniformity, synchronization functions and floating-point evaluation. The reviewed page identifies
itself as the 2026-09-21 Candidate Recommendation Draft. This supports the separation of workgroup
synchronization from global staged execution and the need for explicit layout/numerical contracts.

## S2 — Buffer use and mapping

[wgpu Buffer API](https://docs.rs/wgpu/latest/wgpu/struct.Buffer.html) describes buffer mapping,
CPU/GPU access and the untyped allocation boundary. Use separate staged transfers and validate shared
layout rather than treating a Rust struct or asynchronous callback as sufficient proof.

## S3 — Device limits

[wgpu Limits API](https://docs.rs/wgpu/latest/wgpu/struct.Limits.html) distinguishes storage binding
size, allocation size and compute limits. The reviewed default storage-binding size is 128 MiB;
actual adapter support and requested device limits must be queried. Allocations can still fail
below a stated size limit.

## S4 — Timing and completion

[wgpu Queue API](https://docs.rs/wgpu/latest/wgpu/struct.Queue.html) documents submission, completion
and timestamp-period conversion. [ComputePassTimestampWrites](https://docs.rs/wgpu/latest/wgpu/struct.ComputePassTimestampWrites.html)
is the pass-timing interface. The selected implementation must check supported timing features and
handle unavailable/limited timing rather than invent pass measurements.

## S5 — Standalone ECS

[bevy_ecs crate documentation](https://docs.rs/bevy_ecs/latest/bevy_ecs/) explicitly supports
standalone use. This supports retaining CPU ECS orchestration without retaining the Bevy engine.
It does not prove any particular workspace is already independent of Bevy rendering/application code.

## S6 — Profiling hypotheses

[AMD GPUOpen: Occupancy explained](https://gpuopen.com/learn/occupancy-explained/) describes the
relationship between occupancy, latency, register/resource limits and observed performance. Its
architecture-specific guidance is useful for choosing measurements; it does not establish a
universal workgroup size or prove that maximizing occupancy improves this project.

The checked-out source is the primary source for current project behavior. Its commit, the
original archive digest, source locations and test inventory are recorded separately in the
active sprint's source audit.
