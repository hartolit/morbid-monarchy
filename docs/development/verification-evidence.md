# Verification evidence

This document owns how evidence is collected and what it establishes. Commands are defined in
[workflow](workflow.md); timing/resource values are owned by
[performance budgets](../reference/performance-budgets.md). 01 delivered the documentation,
static and CPU lanes, adapter inspection and the baseline benchmark; the GPU suites and the
connected scenario are pending, and workflow lists each command's state. An unimplemented or
unavailable suite cannot return success.

## Evidence lanes

| Lane | Establishes | Does not establish |
| --- | --- | --- |
| Documentation/static | Links, manifest boundaries, compilation/lints, actual shader assembly and validation. | Runtime physics, GPU races, device lifetime or performance. |
| Pure CPU contracts | Packing, units, identity, deterministic microcases, runtime policy, storage failure decisions. | Execution of a shader or runtime portability. |
| Actual GPU microcontracts | ABI roundtrips, small physical cases, pass ordering, zero/repeated/edge dispatch, staged readbacks. | Large-world timing or visible quality. |
| GPU coupled scenarios | Production cells, bodies, queries, sparse scheduling and observations working together. | Persistence unless the real store/restart path is exercised. |
| Storage/streaming faults | Snapshot retention, stale completion rejection, atomic checkpoint publication and restart from supported bytes. | Recovery of unsnapshotted state after device loss. |
| Interactive native client | Working input/UI/camera, visible physical world, resource-sharing and presentation. | Correctness merely from visual plausibility. |
| Performance qualification | Profile-specific measured time, bytes, waits, allocations and memory. | Other hardware, driver, resolution or changed workloads. |

Static/CPU checks run without a GPU. GPU tests explicitly select an adapter and fail the requested
qualification if none is available. Software adapters may be useful for extra correctness coverage
but must be labelled and cannot qualify the user's discrete-GPU performance target. No automatic
“skip success,” fake GPU implementation or blanket `#[ignore]` for required tests.

## Source and environment identity

Every report carries a schema version; tested source commit or archive digest; dirty diff digest;
lockfile/toolchain; scenario/configuration hashes; compiler/build profile; adapter/backend/driver;
requested and available GPU limits/features; command line; start/end time; test selection/counts;
and overall result. Timed runs also carry resolution, quality, frame pacing, warm-up and samples.
Retain the actual stdout/stderr or relevant diagnostic files without private world data.

Use result states `passed`, `failed`, `unavailable` and `not-run`. Every required item must be
`passed` for the enclosing qualification to pass. A failure exit code must survive nested commands.
Compare discovered tests with the coverage registry so an empty filter cannot satisfy a gate.

## Behavioral coverage

The sprint's source audit names all 28 existing tests. 01 maps each to a preserved or intentionally
revised behavioral requirement and destination. Keep this mapping machine-readable in the evidence
owner once implementation starts. A renamed test retains the same coverage identity.

Add independent golden cell words, analytical fluid/capacity cases, controlled RNG cases and exact
integer totals. For body motion, record tolerances in units before running the candidate. Include
resting penetration, slope progression, jump/grounding, energy/restitution bounds, flotation,
crater/rut limits and conservation where the physical rule promises it.

Dense and sparse GPU execution must agree under the same chosen semantics. Test reactivation after
an edit, climate change or chunk activation; active donors beside sleeping receivers; tile/chunk
seams; translated negative coordinates; and fully active worlds. Do not use the same erroneous
scheduler as both test subject and oracle.

### Deliberate failure specimens

The test harness must prove that it rejects at least these faults: wrong packed-field offset;
unmatched donor/receiver transfer; stale slot/body generation; missing event overflow signal;
old save revision replacing a newer one; and a required GPU suite being unavailable. Use local
fault injection or small deliberately invalid specimens. Do not weaken production validation to
inject a failure. A suite that cannot detect an introduced violation is not sufficient evidence.

| Specimen | Stage | Where it is detected |
| --- | --- | --- |
| Wrong packed-field offset | 01 | `world-model` `cell::layout_tests::a_shifted_field_is_detected` |
| A required GPU suite or adapter unavailable | 01 | `monarchy-evidence` `tests/runner.rs`: a missing adapter is `unavailable`, an unimplemented suite or scenario `failed` |
| An empty, shrunk or unregistered test selection | 01 | `monarchy-evidence` `tests_cpu::tests` |
| A new warning, or a fixed one still excepted | 01 | `monarchy-evidence` `checks::lints::tests` |
| A forbidden dependency reached indirectly | 01 | `monarchy-evidence` `checks::contracts::tests` |
| CI dropping a lane or installing other checker versions | 01 | `monarchy-evidence` `checks::contracts::tests` |
| A required dependency checker missing | 01 | `monarchy-evidence` `checks::static_lane::tests` |
| A presentation-paced measurement run | 01 | `monarchy-evidence` `measurement::tests` |
| A short, late or gapped measurement window | 01 | `monarchy-evidence` `bench::tests` |
| An unsupported value turned into a command | 01 | `world-model` `command::tests`; `monarchy-evidence` `scenario::tests` |
| Unmatched donor/receiver transfer | 03 | pending |
| Stale slot/body generation | 02/04 | pending |
| Missing event overflow signal | 02/04 | pending |
| Old save revision replacing a newer one | 05 | pending |

## Connected scenario

`resident-world` is the named end-to-end scenario, delivered incrementally and completed in 07.
Start from known terrain with a sand bank and water basin. Drive a sphere across the bank into the
water; produce a crater/rut and a displacement wave. Apply a material brush using a GPU-resolved
ray, observe its result and a selected body/contact summary on the CPU. Move focus across chunk
boundaries, capture dirty outgoing state, load additional chunks, then return. Save a complete
checkpoint, destroy the application/store objects, reopen, and compare the promised state.

An adversarial version delays readbacks, reorders load completions, fails a store write, resizes
under pending work and then recovers. It must not lose dirty material, apply a stale completion,
report an unfinished tick as saved, or resurrect an older chunk. Use temporary stores, never a
user's only `world_data` directory.

The native presentation trace must demonstrate the same scenario and actual buffers, not a video
of unrelated CPU gameplay. The harness can drive synthetic input through the normal command
boundary; it must not patch GPU buffers behind the production owners to force a result.

## Measurement outputs

Keep run artifacts under `target/evidence/<run-id>/` or retained CI/local artifact storage. Include
`report.json`, effective configuration, raw frame/pass samples, counter/memory summaries, logs and
any relevant GPU capture or screenshot. Tooling may choose a different filename only by updating
its parser and maintained recipe together.

Counters cover upload, readback and GPU copy bytes by purpose; blocking wait count; outstanding
submissions; command/observation occupancy and age; active/resident cells/tiles; contact candidates;
rejected admissions; CPU payload retention; GPU allocations and their peaks. GPU-requested bytes
are not labelled physical VRAM residency if that is not observable.

Per-pass timestamps diagnose GPU work. End-to-end frame samples and completed-tick throughput
establish the complete workload. Measure an idle case, each physical family, streaming/save
pressure and the coupled case. Keep dense and clustered populations. Record both allocated capacity
and occupied items; unused preallocation still costs memory.

## CI and hardware access

The CPU/static CI lane exists since 01: [`.github/workflows/evidence.yml`](../../.github/workflows/evidence.yml)
runs the same runner on a hosted Ubuntu runner. The hardware lane is manual: the same `cargo
evidence` commands on the nominated machine, with the command, adapter and report path recorded
in the stage handoff. Do not assign or alter the user's self-hosted runners without authority. A
GPU-required change cannot be called qualified while that lane remains unrun.

Keep offline shader validation mandatory in ordinary CI, including assembled renderer variants.
A compilation-only Windows check is useful but not a Windows runtime qualification. Do not copy
credentials, saves or opaque profiler captures into public artifacts without review.

## Completion evidence

Each stage keeps one concise handoff with tested revision, affected coverage, actual commands and
remaining gaps. 07 publishes a permanent summary under `docs/development/` or another maintained
owner and updates status. Retain raw evidence outside the temporary sprint directory before its
removal. The summary records conclusions and limits, not thousands of log lines.
