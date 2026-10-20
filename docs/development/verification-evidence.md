# Verification evidence

This document owns how evidence is collected and what it establishes. Commands are defined in
[workflow](workflow.md); timing/resource values are owned by
[performance budgets](../reference/performance-budgets.md). 01 delivered the documentation,
static and CPU lanes, adapter inspection and the baseline benchmark; 02 the shader validation
item and the `transfers` GPU suite; 03 the `cells` GPU suite; 04 the `bodies` suite and
`resident-world`; 05 the `streaming` suite; 06 the `render` suite, `resident-world` in the native
window, scripted runs of the real client and the benchmark of the native frame. Workflow lists
each command's state. An unimplemented or unavailable suite cannot return success.

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
| Wrong packed-field offset or swapped record fields, executed on a device | 02 | `transfers` GPU suite, `layout-specimens-detected` |
| A shader error reported at the wrong file or line | 02 | `gpu-world` `shader::tests::errors_point_at_the_module_file_and_line` |
| A GPU result naming another command or tick | 02 | `gpu-world` `readback::tests::a_result_from_another_command_or_tick_is_rejected_without_partial_outcomes` |
| A dependency another workspace member's features add | 02 | `monarchy-evidence` `checks::contracts::tests` |
| Unmatched donor/receiver transfer | 03 | `cells` GPU suite, `unmatched-transfer-detected` (a `react` built to drop accepted liquid: unbalanced ticks, reference mismatch, lost volume) |
| A sparse schedule missing a dependency | 03 | `cells` `dense-sparse-agreement`, `sleeping-receivers`, `seam-flow` (dense execution and the CPU reference disagree with a wrong wake reach) |
| Stale slot generation | 02 | `gpu-world` `slots::tests`; `transfers` `stale-session-and-slot`, `resize-success-and-refusal`, `device-loss-and-recreation` |
| Stale body generation | 04 | `gpu-world` `bodies::tests`; `bodies` GPU suite, `body-handles` and `controls` (removed, out-of-table and wrong-generation handles are stale for every body command; a sample of a stale handle is reported gone) |
| Missing event overflow signal | 02 | `gpu-world` `readback::tests::a_missing_overflow_signal_is_rejected`; `transfers` `event-overflow-and-reset`; `bodies` `observation-delivery` (the body producers overflow four events a tick and the ledger counts it) |
| A body slot beyond the table in a ray result | 04 | `gpu-world` `readback::tests::body_results_become_absolute_and_samples_must_echo_their_handles` (the f32-rounded sentinel the first ray kernel produced is a decode error) |
| An important body event lost in a full ring | 04 | `world-runtime` `bodies::tests` (deferral, waiting removals, late removals after slot reuse); `bodies` `observation-delivery` (hostile replays deliver what the in-order replay delivers) |
| Old save revision replacing a newer one | 05 | `world-store` `store_lifecycle` `publication_refuses_missing_older_and_out_of_order_data_and_changes_nothing`; `streaming` `newest-retained-wins` (an older revision published after a newer one is refused with `Conflict::HeadNotNewer` and changes nothing) |
| A stored row that is damaged, truncated or of a future version | 05 | `world-store` `format::tests` and `store_lifecycle` `absent_found_corrupt_and_unsupported_rows_read_differently`; `streaming` `refused-rows-stay-out` (never generated over, kept byte for byte) |
| A save reported complete without the last tick | 05 | `world-runtime` `session::tests::a_close_after_an_earlier_save_saves_its_own_last_tick`; `streaming` `orderly-shutdown` (a close that cannot save in time reports it) |
| GPU culling that drops a visible chunk or reorders draws | 06 | `render` `culling-matches-unculled` (culled frames equal the unculled path's pixels from four poses and after an out-of-view edit) |
| A renderer that recreates resources for an edit, or keeps stale bind groups after a resize | 06 | `render` `edit-visible`, `bind-groups-follow-buffers` |
| A camera that follows a body only through a CPU readback | 06 | `render` `follow-camera` (the sample on the CPU is held stale while the device pose moves) |
| A Bevy engine crate in the native client's closure | 06 | `monarchy-evidence` `checks::contracts::tests::the_native_client_closure_has_no_bevy_engine_crate`; `check static` on the resolved closure |

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
boundary; it must not patch GPU buffers behind the production owners to force a result. Since 06
`--consumer native` runs the same scenario with every session presented in the client's window
(`morbid_app::viewer`), captioned and captured per step, and the `native-client` scripts drive the
real client's input, overlay and command path and check each tool against its command outcome.

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
establish the complete workload. Since 06 a native frame sample is one presented frame: the
client's loop may run iterations that present nothing (both presentation slots busy) and still
advance the timeline, and they belong to the next presented frame's sample. Measure an idle case, each physical family, streaming/save
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
