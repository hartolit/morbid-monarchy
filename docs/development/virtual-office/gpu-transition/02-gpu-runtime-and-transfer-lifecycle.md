# 02 — Shared GPU runtime and bounded transfer lifecycle

Execute after 01's required foundation/evidence is complete. Establish the real GPU boundary that
subsequent physics and rendering will use. A validation-only sketch is insufficient, but full
material/body simulation belongs to 03–04.

## Read and scope

Read the handoff, GPU contract, architecture, performance budgets, workflow, implementation and GPU
practices. Recheck selected wgpu APIs and the resolved dependency tree. Use `world-runtime` for
semantic orchestration and `gpu-world` for concrete execution. No app/window or database policy
inside GPU kernels; no full CPU terrain mirror as a convenient bridge.

## Checkpoint A — Context and allocation plan

Create the real headless runtime composition using standalone `bevy_ecs` where orchestration needs
it. Implement a single GPU context/submission owner and explicit adapter selection. Query features
and limits, request the required subset, and report selected backend/driver/limits. Do not silently
select a software adapter when the nominated hardware was requested.

Implement the complete initial resource plan with checked arithmetic, storage-binding versus buffer
size limits, alignment, frame/tick admission and shared staging budgets. Test a deliberately limited
capability plan as well as real-device allocation. Unsupported creation/resize returns typed refusal
without destroying a valid current world. The radius-32 packed array cannot be assumed to fit one
default storage binding; request supported limits or document the tested strategy/refusal.

Define allocation ownership, retirement, shader/pipeline caching and device-loss handling. Pool slots
and callbacks carry generations. Count requested bytes and peak old/new overlap. No magic unbounded
Vec of pending submissions or mappings.

## Checkpoint B — Executable ABI and real commands

Implement packed low/high words, shared declarations and command/result/body/chunk record layouts.
Use explicit offsets/strides and independent golden cases. Assemble the actual shader source used
at runtime; integrate offline validation and useful file/line diagnostics. Do not preserve Bevy's
shader imports or build a custom language framework to avoid a small source-module solution.

Execute small GPU layout probes, then real ordered cell initialization/edit/query commands against
a small resident allocation. A probe is explicitly a layout test, not a fake material simulation.
Connect the actual runtime command port and bounded admission/results. Same-ID retry, conflicting
reuse, stale session/slot and out-of-bounds payload cases must produce the specified outcomes.

Use a minimal offscreen buffer-consuming render/layout probe only if needed to prove future sharing.
Do not create a second device or a CPU copy/upload bridge. The full renderer is 06.

## Checkpoint C — Nonblocking transfers and observations

Implement pooled upload/readback reservations, submission tracking, completion polling, mapping,
unmapping and reuse. The frame-facing path never blocks for a result. Readbacks include identity,
source tick and a defined age. Test delayed/out-of-order callbacks and stale results after resize or
device recreation. Bounded pressure prevents admission rather than silently overwriting live work.

Provide a real CPU consumer of command/query outcomes and transfer counters in the headless host.
Implement receipt retention/acknowledgement or the equivalent bounded session-sequence rule. Define
cosmetic overflow versus important result retention; later physics must reuse this owner rather
than invent another event queue. A full result queue before command admission must be handled safely.

Handle intentional stop/drain and controlled fault injection. Completion is not durable storage,
and device recreation invalidates old session handles. Keep shader/range validation enabled.

## Checkpoint D — Measurements and conformance

Register the `transfers` GPU suite and run it on the actual selected adapter. Cover empty batches,
partial workgroups, repeated reuse, counter reset, sizes around alignment boundaries, delayed mapping,
full queues, stale generation, invalid limits, successful/refused resize and loss/recreation.
Deliberately corrupt a layout specimen and prove the test fails.

Add optional timestamp capture with correct feature checks/conversion; when unavailable, record
that limit. Count GPU/CPU copy bytes by purpose and steady-state waits/allocations. This stage must
prove bounded resource reuse, not claim an application speedup before physics exists.

## Acceptance and stop

A02/A03 are complete: actual shader roundtrip and command/query path, one context, typed limit
handling, bounded transfer lifetime, no missing-suite success, and a usable CPU result consumer.
Run focused static/model/runner and actual GPU transfer checks. Update reference layouts, status
and handoff with real APIs, commands, limits and evidence; commit coherent changes and stop before 03.
The legacy app remains only the external baseline.
