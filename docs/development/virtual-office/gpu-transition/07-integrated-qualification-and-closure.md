# 07 — Integrated qualification, measured comparison and closure

Execute after 06. This is the full integration and evidence gate. Repair demonstrated in-scope defects
at their owner, rerun affected checks and then rerun the impacted full gate. Do not convert failures
into exclusions, defer them to a new sprint or declare completion based on compilation alone.

## Read and reconcile

Read the handoff, acceptance matrix, canonical status/architecture, verification evidence and frozen
performance budgets. Inspect the actual implementation and dependency graph, not only prior agent
summaries. Confirm all A01–A12 evidence exists and corresponds to the current interfaces. Identify
source/configuration changes that invalidate earlier results and rerun their checks.

## Checkpoint A — Complete supported-build gate

Run the final common runner on the actual chosen adapter: documentation/static checks, every supported
feature/configuration combination, warning-denying Clippy/rustdoc, complete CPU tests, assembled
compute/render shader validation and all required GPU suites. Confirm discovered coverage includes
the 28 original test intentions and all added lifecycle/physics cases. An empty filtered suite,
ignored required test or unavailable adapter is not pass.

Run the evidence runner's deliberately failing specimens and verify nested failures cannot produce
a successful enclosing report. Review dependency direction, debug/reference isolation, obsolete
files/imports, duplicate configuration, unbounded queues and finite migration exceptions. Confirm
normal builds cannot select the old CPU engine or a second physical authority.

Qualify the primary native Linux backend first. Execute any additional backend/platform actually
claimed by the application; label others unqualified rather than implying cross-platform support
from wgpu's existence. No Windows/macOS/browser qualification is added solely because a library
could support it. GPU CI absence must remain explicit; CPU CI green is not GPU acceptance.

## Checkpoint B — Connected adversarial execution

Complete `resident-world` through the production native and headless consumers, cells, bodies,
GPU-resolved edits, useful CPU outcomes, streaming, consistent save and fresh reopen. Include ordinary
and adversarial traces with fixed effective configuration and retained diagnostics.

Exercise dense overlapping body work near active tile/chunk seams while brush edits and phase changes
occur; shift/rebase the window across negative coordinates; resize under pending transfers; delay
readbacks and consumers; reorder loads; fail and retry a save; then return to previously edited chunks.
Observe bounded pressure rather than silently removing work. Verify important results survive their
promised retention and stale callbacks cannot affect a new session/slot/body.

Test orderly close separately from process interruption and controlled device loss. Reopen the last
complete checkpoint and verify the documented loss boundary and complete body/configuration state.
No screenshot alone establishes saved-data correctness. Include a fresh-start packaged application
run outside the repository working directory and a deliberately corrupt legacy import refusal.

Use bounded, stated stress durations and seeds. A capacity workload may produce a documented admission
refusal when a requested resource exceeds the supported plan; an admitted workload must not silently
truncate physics. Unexpected overflow faults must not publish partially mutated ticks as complete.

## Checkpoint C — Fair hardware comparison

Execute the frozen qualification, scale and capacity profiles and required idle/flow/body/mixed/
streaming cases on the nominated, identified hardware. Measure at least the repetitions/warm-up/sample
length specified in the budget owner. Record background contention, power/configuration, frame pacing,
resolution, quality and actual adapter features. Do not stop the user's unrelated model services
without authority merely to improve benchmark numbers.

Compare with 01's identifiable baseline using compatible physical work and presentation quality.
Where deterministic RNG, fixed body timing or intentional rule corrections preclude exact legacy
trajectories, retain scenario-equivalent comparisons and explicitly bound the claim. Use simulation-
only or matched-renderer comparisons to distinguish physics gains from changed visual workload.

Report median/p95/p99 frame and CPU work, GPU pass totals when supported, physical ticks completed,
copy/upload/readback bytes by purpose, peak CPU/GPU resource plans and measured usage where available,
recurring allocations/waits, event age/pressure, contact scaling and checkpoint/streaming overhead.
Keep raw samples. Averages and shader-only timings are insufficient.

The completion gate requires correct preserved behavior, eliminated normal full-grid transfers,
reduced simulation-related CPU work, the frozen interactive timing/resource/latency contract and no
unexplained regression beyond the documented noise policy. Do not fabricate driver memory usage
from allocation estimates; report both with accurate labels. A failed budget remains a failed gate
until repaired or the user explicitly changes scope.

## Checkpoint D — Maintained result and cleanup

Update architecture to the actual adopted owners, GPU contract to actual layouts/order, world format
to actual versions and import/recovery behavior, and development guide to tested commands. Update
status/roadmap with achieved versus unqualified capabilities and a concise measured comparison linked
to retained evidence. Retain no claim that depends only on an earlier agent's untested handoff.

Write the final acceptance summary with per-item outcome, tested source/configuration identity,
commands, artifact locations, key measurements, intentional rule/visual differences and remaining
external qualifications. Store raw captures/reports outside tracked docs; keep their retention path
or artifact identity useful after office cleanup. Do not embed a multi-page raw log in status.

Once all required acceptance passes, migrate lasting audit/coverage information into its proper
source/test/document owner. Fix every inbound link to this sprint, including status and office links.
Then remove the completed sprint directory and office entry in a reviewed local commit, following
the office lifecycle, unless the user explicitly requested retaining it. Git preserves assignments;
the machine-readable test coverage and reproducible baseline/evidence remain available elsewhere.
Run the final docs/link check after cleanup.

If any required lane is failed/unavailable/not-run, keep the sprint and its current handoff active.
Report the exact blocker and last tested state. Do not clean away unfinished acceptance or mark the
roadmap complete. No automatic push or successor sprint is authorized.

## Stop

A13–A15 and all prior items pass, canonical docs describe the real result, superseded paths are gone
and retained evidence supports the stated improvement. Report the actual final commits and checks,
the measured outcome and the scope of hardware qualification. Stop; do not invent another stage.
