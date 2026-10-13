# GPU transition — current handoff

Maintain this one current record; replace stale working notes rather than accumulating a session
transcript. Canonical design/status belongs in maintained docs, and raw evidence belongs in retained
artifacts. Update after each coherent checkpoint and before stopping or changing context.

## 02 boundary

**State:** 02 complete (2026-10-13), assigned by the user on 2026-10-13. Checkpoints A–D are
committed on `experimental`. 03 has not started and needs its own assignment.

**Next action:** assign [03-gpu-cell-simulation.md](03-gpu-cell-simulation.md). Nothing in 02 is
left to resume.

## 02 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 02 — shared GPU runtime and bounded transfer lifecycle. A–D complete. |
| Source state | `experimental`: `1440bf5` (world-runtime and model additions), `6d334a0` (gpu-world, evidence integration, the `transfers` suite), then the documentation commit and this record. The tree was clean before 02; no unrelated work was present. |
| Actual adoption | **A:** `world-runtime` (identities, the `PhysicalWorld` port, receipts, ledger, clock, standalone `bevy_ecs` runtime) and `gpu-world` `context`/`plan`/`resources`: adapter selection by the stable identifier (which `inspect-adapter` now takes from `gpu-world`), limits raised from WebGPU defaults only to the radius-32 plan's needs, error-scoped counted allocation, one timeline (≤ 3 in flight), loss and uncaptured-error health, pipelines compiled once per session. **B:** `layout` (records declared once, WGSL generated; cell constants from `WorldCell::LAYOUT`), `shader` (assembly with line map, naga validation), ordered `apply_edit`/`query_column` kernels over resident slots, identity-checked decoding. **C:** `staging` (four pools, generation-checked map mailbox), `slots` (slot lifecycle, window map), resize with device-to-device migration, the ledger as the CPU consumer, fault injection, stop/drain. **D:** `cargo evidence test gpu --suite transfers` (28 cases, one `gpu-suite` process each, registry-checked), `static.shaders`, per-package dependency rules, pass timestamps. Superseded: evidence's own adapter enumeration (now `gpu-world`'s), the "02 requests a larger binding or splits the array" open choice (a larger binding is requested; otherwise refused). |
| Package decisions | `world-runtime` under `crates/`, `gpu-world` under `adapters/` ([ADR 0002](../../../decisions/0002-native-composition.md#02-package-check)). GPU contracts are not cargo tests (hosted CI has no GPU): the suite lives in the evidence leaf and drives the public APIs; layout probes assemble from `gpu-world`'s exported declarations and run through `submit_external`. `world-store` and `world-render` are not created. |
| Physical limits | Unchanged from 01. New capacities with their source: 256 commands and 64 cosmetic events per tick, 32 chunk-upload and 32 capture buffers, 96 slot changes per tick ([GPU contract](../../../reference/gpu-contract.md#observations-and-reliability)). Radius 32 needs a 138,444,800-byte storage binding; under WebGPU defaults it is refused and radius 31 runs. |
| Decisions / deviations | Edits take a centre cell (the ray is 04's) and wake nothing (activity is 03's). Body, attract/lift and temperature commands are refused as `NotSupported` until 03–04. The only cosmetic producer is fault injection until 04. Uploads activate before a tick's commands and captures copy after them. A retired slot's generation advances at retirement. wgpu 29.0.4 panics from `Device::poll` on a backend-reported loss, so the poll is isolated with `catch_unwind` and turned into the lost state. Dependency rules use `cargo tree -p` because workspace metadata unifies the Bevy baseline's features into `bevy_ecs`. The process runner gained step timeouts. |
| Checks | At `f740577`, clean, 2026-10-13 08:51–08:52 UTC, with `CARGO_BUILD_JOBS=4`: `cargo evidence check docs` passed (150 local links in 50 files, 7 packages with READMEs); `cargo evidence check static` passed (all 12 items, including `static.shaders` for the 2 assemblies and the per-package dependency rules; Clippy 7 diagnostics, all covered by the 3 recorded exceptions); `cargo evidence test cpu` passed (147 tests: 96 registered CPU behaviors, 28 of them baseline, and 51 runner tests); `cargo evidence test gpu --suite transfers --adapter vulkan-10de-2c05` passed (29 items: the registry check and all 28 cases); `cargo evidence test gpu --suite cells --adapter vulkan-10de-2c05` failed as intended (unregistered until 03). Reports in `target/evidence/final-02-{check-docs,check-static,test-cpu,transfers,cells-unregistered}/`. Each intermediate commit was tested before it was made: `1440bf5`'s exact tree in a scratch copy (formatting, Clippy, `test cpu` with its 71-entry registry; only the runner test that needs a git commit identity failed there, as the copy had none) and `6d334a0` with all four lanes. |
| Hardware / measurements | RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.71.09), Ryzen 9 5950X, Linux 7.2.8. Granted: storage binding 138,444,800 B, buffer 256 MiB, `TIMESTAMP_QUERY` when requested (period 1 ns); the adapter offers 2,147,483,644 B and 1 TiB. Measured transfer lifecycle, resize overlap and planned allocations against the envelopes: [performance budgets](../../../reference/performance-budgets.md#measured-transfer-lifecycle-02). Per-case JSON in `target/evidence/<run>/gpu-transfers/` on that machine only. No application performance claim: no physics runs on the GPU yet. |
| Acceptance | A02 satisfied: production shader roundtrip and golden words, partial dispatch, default-limit and allocation refusal, successful and refused resize without loss, one context. A03 satisfied: real CPU consumer (runtime ledger), delayed and reordered deliveries, held completions, full pools, stale session and slot, same-ID retry and conflict, no frame waits, executed on the GPU. A04–A15 not run. |
| Data safety | No case opens a store; worlds are generated or synthetic in memory. `world_data/save.redb` was neither opened nor changed (8,237,617,152 bytes, modified 2026-10-11 14:24 local time, as at the end of 01). |
| Blockers | None for 03. |
| Next action / stop | Stopped at the end of 02. Assign 03. |

### Carried into later stages (02)

1. **Edits wake nothing** (03): `apply_edit` changes cells without activity metadata; 03 adds
   reactivation to the same command path.
2. **Tool sources and sinks** (03): the sand brush keeps the baseline rule that compacts sand into
   sandstone only up to the elevation ceiling, so sand beyond it is lost, like the quench defect;
   03's accounting classifies tool sources and sinks.
3. **Brush ray, body commands and cosmetic producers** (04): edits target a centre cell; spawn,
   control, attract/lift are refused; contacts and immersion become the ledger's first producers.
4. **Residency policy and revisions** (05): slots, uploads, captures, retirement and resize exist
   as mechanisms; focus movement, dirty revisions, retained snapshots and the store do not. GPU
   edits do not advance a chunk's revision yet.
5. **A real driver loss is untested**: loss is injected with `Device::destroy`; the
   `catch_unwind` path for a backend-reported loss has no specimen on this machine.
6. **Software adapters** are refused without consent; no software adapter exists here, so that
   refusal is not executed.
7. **Per-tick allocations inside wgpu** (07): about 103 per tick on the submit path; application
   pools allocate nothing after warm-up.

## 01 boundary (closed)

01 completed on 2026-10-12; its record follows.

## 01 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 01 — baseline, contracts and seams. A–D complete. |
| Source state | `experimental`, last tested commit `fa38fdd`, clean tree; the commit after it fills in this record only. No unrelated work was present; a pre-existing GitHub Desktop stash was left untouched. |
| Actual adoption | **A:** `eea8652` pins Rust 1.99.0 and tracks `Cargo.lock`; `58e9648` captures old-writer fixtures. **B:** `tools/evidence` (`monarchy-evidence`) with the `cargo evidence` alias; docs/static/CPU lanes; adapter inspection; GPU suites and `run` fail explicitly; 18 frozen scenario files; the `legacy-baseline` driver; work counters in the engine, renderer and persistence; `coverage.toml` maps all 28 baseline tests; `lint-baseline.toml` holds 3 exceptions. The measured baseline is `e8d49db`. The visual reference (three frames from the qualification scenarios, `tools/evidence/baselines/legacy-visual/`) and the presentation and tuning defaults ([acceptance](acceptance.md#baseline-presentation-and-tuning-reference-01)) record the rendering and tool surface. **C:** `7a84a80` adds `world-model`, migrates every consumer and deletes the duplicates; `legacy_format` keeps the old row bytes. **D:** `0c166e3` adds `deny.toml`, `cargo deny` and `cargo machete` at pinned versions, the CI workflow and its pin check, and the package-naming check. |
| Package decisions | `world-model` exists because the engine, the application, spatial-lib and the runner all consume it. `world-runtime`, `gpu-world`, `world-store` and `world-render` are not created: no consumer exists before 02, and empty forwarding crates are not allowed. `spatial-lib` keeps only the toroidal grid, the chunk manager and the redb backend; its key math moved to `world_model::spatial`. Commands and observations are used by the evidence driver; the runtime ports are recorded as signatures in [GPU contract](../../../reference/gpu-contract.md#runtime-ports-implemented-in-02) for 02 to create. |
| Physical limits | Fixed in [GPU contract](../../../reference/gpu-contract.md#supported-physical-envelope-fixed-in-01) and owned by `world_model::{config, body, units}`: tick 10–240 Hz, catch-up 1–16, body substep ≤ 1/120 s dividing the tick, elevation scale 0.01–5.0 as physical configuration, body radius 2–8, relative density 0.01–25, mass 0.01–1000, speed ≤ 600 world units/s, ≤ 4,096 bodies, brush radius 0–64 and strength 1–255, active radius 1–32. Tolerances are the baseline tests' own, per test in the registry. |
| Decisions / deviations | The elevation scale is physical configuration ([world format](../../../reference/world-format.md#existing-packed-cell)); the acceptance conflict is closed. The run protocol counts a 1,800-tick window instead of 1,800 frames; the calibration and its reason are recorded in the [performance budgets](../../../reference/performance-budgets.md#regression-and-improvement-policy) before any candidate. Measurement windows update continuously when unfocused, because Bevy's default throttles them to 60 Hz. The quench ceiling defect is kept and documented for 03. `cargo deny` found advisories after the baseline was measured: `0e948bb` updates six locked crates at patch level (generator output unchanged), and `deny.toml` accepts four advisories without a compatible fix, each with its reason and stage. `0e948bb` also took the staged deletion of `runtime/hooks.rs` without its module declaration, so that commit alone does not build; `ecd9212` completes the removal. The first full baseline at `79e7ead` was rejected (compositor pacing) and is kept locally as `target/evidence/rejected-baseline-*-79e7ead`. |
| Checks | At `fa38fdd`, clean, 2026-10-12 20:01–20:10 UTC, with `CARGO_BUILD_JOBS=4`: `cargo evidence check docs` passed (137 local links in 48 files); `cargo evidence check static` passed (all 11 items, including `cargo deny`, `cargo machete` and the CI pin check; 7 Clippy diagnostics, all covered by the 3 recorded exceptions); `cargo evidence test cpu` passed (98 tests: all 49 registered behaviors, 28 of them baseline, and 49 runner tests). Reports in `target/evidence/final-{docs,static,cpu}/`. Baseline: `cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --output target/evidence/baseline-qualification`, the same for `scale`; both passed at `6e108d6`. Drift after the extraction: `cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --case flow --case body-contact --case coupled --output target/evidence/drift-7a84a80` at `7a84a80` passed, 5/5 valid runs per case, none contended; simulation and body time per tick within 2% of the baseline, frame medians 3–6% lower ([performance budgets](../../../reference/performance-budgets.md#measured-legacy-baseline-01)). |
| Hardware / measurements | Ryzen 9 5950X, RTX 5070 Ti (`vulkan-10de-2c05`, NVIDIA 615.71.09), KDE Wayland, 1920×1080 always-on-top window, no vsync. Summaries in `tools/evidence/baselines/legacy-6e108d6/`; raw samples only in `target/evidence/baseline-{qualification,scale}/` on that machine. Results in the [performance budgets](../../../reference/performance-budgets.md#measured-legacy-baseline-01). The capacity profile is frozen and not run on the baseline. |
| Acceptance | A01 satisfied for 01's scope: source baseline, all 28 test intentions and the tool surface retained; the runner's negative cases pass and a missing suite cannot pass; consumers use the model and the fixtures keep their meaning; the baseline names its hardware and limits; profiles and tolerances are frozen before optimization. A02–A15 not run. |
| Data safety | Every bench and visual-reference run used a new world directory under its run directory and a scratch working directory; since `6e108d6` they are deleted after validation, and the leftovers of earlier pilot runs were removed. The fixture store test works on a copy and removes it. `world_data/save.redb` was never opened by 01. It changed on 2026-10-11 at 14:24 local time (8,074,563,584 → 8,237,617,152 bytes) while no 01 work was running. |
| Blockers | None for 02. |
| Next action / stop | Stopped at the end of 01. Assign 02. |

### Carried into later stages

1. **Measured defects for their owners** (source audit, "Found while measuring"): chunks that stream
   in are never woken (03, A05); the brush cursor marks all four material assets modified every
   frame (06); a brush frame and a tick frame each upload the whole grid (06); the unfocused-window
   policy of the native client (06).
2. **Persistence baseline failures** for 05, unchanged by 01: eviction-only saves without a dirty
   flag, stale reads while a save is pending, an exit flush that omits active and cached chunks, and
   read or decode errors that fall through to generation.
3. **Shader validation** joined `check static` in 02 (`static.shaders`). The legacy Bevy shaders
   under `crates/morbid-app/assets` are validated only at runtime and are removed in 06.
4. **The capacity profile** runs against the GPU runtime (04–07), never against the baseline.

## 00 boundary

**State:** 00 complete (2026-10-10). This was a documentation-only adoption. No runtime code,
manifest, dependency, toolchain or saved-world byte changed. Nothing was built, tested, run on a
GPU or measured, and no acceptance item has passed.

**Baseline:** branch `experimental`, commit `a9c1539` (clean before 00). Source and shaders last
changed in `e1fa1d2`. The supplied archive (`morbid-monarchy-experimental.zip`, SHA-256
`3c677631…871439`) is not in the repository. 00 compared the audit's symbols and its 28 tests with
the checkout instead of comparing digests. The adoption commit is the commit that last changed this
file (`git log -1 -- docs/development/virtual-office/gpu-transition/handoff.md`).

**Confirmed user direction:** GPU physical authority over resident terrain and bodies; CPU owns
intent, residency and I/O; rendering shares the GPU-resident state; the final application keeps
standalone `bevy_ecs` and drops the Bevy engine. This authorizes a transition sprint, not a new game
or physics feature set. The package map, exact algorithms and initial numeric budgets are proposed
engineering choices, not measured outcomes.

**Next action at the end of 00:** assign
[01-baseline-contracts-and-seams.md](01-baseline-contracts-and-seams.md); that has since happened (see above).

## 00 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 00 — first execution: adopt and reconcile. Complete; no partial checkpoint remains. |
| Source state | `experimental` at `a9c1539` before 00. There was no uncommitted or unrelated work; local untracked state is limited to the ignored `Cargo.lock`, `target/` and `world_data/`, all left untouched. |
| Actual adoption | Architecture now separates the target from a [current baseline](../../../architecture.md#current-baseline) table, and the package map stays proposed. ADR, decision-index and budget states distinguish requested direction, adopted policy and proposed values. The [source audit](source-audit.md) has a checkout reconciliation, corrected rows and dispositions for the retired rule files. The implementation practice gained explicit naming, typed-error, `// SAFETY:` and cold-path rules carried over from those files. [Acceptance](acceptance.md) has a source-reconciled feature inventory, an open conflict and the native-client wording for `resident-world`. Status, the development guide, workflow, world format, roadmap and the office/sprint indexes reflect the checkout. |
| Decisions / deviations | The retired `docs/persona.md`, `docs/rules.md` and `docs/knowledge/rust_knowledge.md` had already been deleted in `a9c1539`. 00 verified the mapping of their intent instead of deleting them. The audit's "four layer entities" was corrected to four per active chunk. Items for 01 are listed below. |
| Checks | See [checks run in 00](#checks-run-in-00). |
| Hardware / measurements | None. No adapter was enumerated; the GPU and performance lanes are not run. |
| Acceptance | A01–A15 not run. |
| Data safety | The user's `world_data/save.redb` (8,074,563,584 bytes, git-ignored) was neither opened nor modified. No application was launched. |
| Blockers | None for 01. |
| Next action / stop | 00 stopped; 01 was then assigned. |

### Deviations requiring 01 attention

1. **Dependency baseline.** An untracked, ignored `Cargo.lock` resolves Bevy 0.19.0, `bevy_egui`
   0.41.0 (egui 0.35.0), wgpu 29.0.4, redb 4.1.0 and bitcode 0.6.9. An offline `--locked`
   metadata check could not finish because some packages were not cached. Treat this lockfile as
   a candidate resolution. Commit it, and remove the ignore, only after a successful `--locked`
   build. The active toolchain is rustc 1.99.0 stable with no pin.
2. **Save safety.** The baseline opens or creates `world_data/save.redb` relative to the working
   directory at startup. Launch the baseline only from a scratch working directory, and capture
   old-writer fixtures into temporary stores (see the
   [development guide](../../../guides/development.md#baseline-versus-target)).
3. **Persistence baseline failures** that 01 records but does not fix: chunks are saved only on
   LRU eviction, without a dirty flag; a chunk reloaded while its save is pending reads stale or
   absent data; the exit flush leaves out active and cached chunks and in-flight saves. These
   are the fixtures behind A08 and A09.
4. **CPU live-grid consumers** to include in the feature inventory and in command/observation
   design: brush ray march, preview, spawn height and attract/lift targets; brush edits; observer
   walking floor samples; full-grid statistics sums. The residency focus freezes while the
   observer is detached.
5. **Elevation-scale conflict.** Decide its meaning in Checkpoint C (decided in 01: physical
   configuration; see [acceptance](acceptance.md#resolved-conflicts-with-existing-behavior)).
6. **Clock limits.** The live tick-rate (10–240 Hz) and catch-up (1–16) controls need a supported
   range, plus a rule for how body substeps divide a tick.
7. **Coordinates.** Fixture the axis mapping recorded in
   [world format](../../../reference/world-format.md#existing-packed-cell), including the
   misleading `to_ivec2` comment and how nonzero-`z` keys are handled.
8. **Renderer baseline.** Count draws and entities as part of the baseline (four per active chunk;
   676 at radius 6). The UI permits radius 32, which exceeds a default 128 MiB storage binding.
9. **Test discovery.** `spatial-lib` has zero tests. The registry should expect exactly the 28
   `monarch-engine` tests. `runtime/hooks.rs` contains only comments; remove it or replace it with
   a real consumer.

### Checks run in 00

These were run on the final staged adoption diff. There is no repository docs checker yet (01
delivers `cargo evidence check docs`), so 00 used a temporary script outside the repository.

| Check | Result |
| --- | --- |
| Local Markdown link and heading-anchor check over every `*.md` (temporary Python script, run with `python3 -I`) | Passed: 88 local links in 41 files, 0 failures. Baseline before editing: 71 links, 0 failures. In a negative test, the script rejected a missing file and a missing anchor. |
| `git diff --cached --check`, plus a trailing-whitespace scan of `docs/` and `AGENTS.md` | Passed: no errors. |
| Scope | Every staged path is under `docs/`. The SHA-256 of the local `Cargo.lock` and the size/mtime of `world_data/save.redb` are unchanged. |
| Source reconciliation | Inspection only: the 28 `#[test]` names and every audited symbol were located in `a9c1539`. |
| `cargo metadata --locked --offline` | Exited 101: an uncached locked package (`accesskit_consumer` 0.35.0) could not be fetched offline. This is not a build. |
| Not run | `cargo build`, `test`, `fmt`, `clippy` and `doc`; `cargo evidence` (it does not exist yet); any application launch or GPU execution. |

## Record at each stage boundary

Use the fields below with actual values after execution. “Not run,” “unavailable” and “none” are
valid when accurate; they are not a successful qualification. Do not invent commits or reviewers.

| Field | Record |
| --- | --- |
| Assigned stage / current checkpoint | Exact prompt and last completed checkpoint; do not advance because the next file exists. |
| Source state | Branch, last tested commit, uncommitted changes and unrelated work preserved. |
| Actual adoption | Owner/API/data path now canonical; consumers updated and superseded path removed. |
| Decisions / deviations | New evidence and its consequence; canonical doc/ADR updated, including any changed rule or support limit. |
| Checks | Exact commands, selected tests/counts, outcome and tested source/configuration identity. |
| Hardware / measurements | Actual adapter/backend, effective profile and retained report path; distinguish estimates from measurements. |
| Acceptance | Applicable A-items satisfied, failed or unqualified, with evidence location. |
| Data safety | Temporary store/fixture use, outstanding snapshot/save obligations and recovery implications. |
| Blockers | Concrete failure or missing external capability; owner and last tested state, not a speculative concern. |
| Next action / stop | Exact resumable task inside the same stage, or next prompt requiring a new assignment. |

For 07 closure, preserve the final accepted evidence and lasting coverage in their maintained owners
before removing this temporary handoff/sprint. Keep it active when any required gate remains open.
