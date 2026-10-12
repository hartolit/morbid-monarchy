# GPU transition — current handoff

Maintain this one current record; replace stale working notes rather than accumulating a session
transcript. Canonical design/status belongs in maintained docs, and raw evidence belongs in retained
artifacts. Update after each coherent checkpoint and before stopping or changing context.

## 01 boundary

**State:** 01 complete (2026-10-12), assigned by the user on 2026-10-10. Checkpoints A–D are
committed on `experimental`. 02 has not started and needs its own assignment.

**Next action:** assign [02-gpu-runtime-and-transfer-lifecycle.md](02-gpu-runtime-and-transfer-lifecycle.md).
Nothing in 01 is left to resume.

## 01 record

| Field | Record |
| --- | --- |
| Assigned stage / checkpoint | 01 — baseline, contracts and seams. A–D complete. |
| Source state | `experimental`, last tested commit `fa38fdd`, clean tree; the commit after it fills in this record only. No unrelated work was present; a pre-existing GitHub Desktop stash was left untouched. |
| Actual adoption | **A:** `eea8652` pins Rust 1.99.0 and tracks `Cargo.lock`; `58e9648` captures old-writer fixtures. **B:** `tools/evidence` (`monarchy-evidence`) with the `cargo evidence` alias; docs/static/CPU lanes; adapter inspection; GPU suites and `run` fail explicitly; 18 frozen scenario files; the `legacy-baseline` driver; work counters in the engine, renderer and persistence; `coverage.toml` maps all 28 baseline tests; `lint-baseline.toml` holds 3 exceptions. The measured baseline is `e8d49db`. The visual reference (three frames from the qualification scenarios, `tools/evidence/baselines/legacy-visual/`) and the presentation and tuning defaults ([acceptance](acceptance.md#baseline-presentation-and-tuning-reference-01)) record the rendering and tool surface. **C:** `7a84a80` adds `world-model`, migrates every consumer and deletes the duplicates; `legacy_format` keeps the old row bytes. **D:** `0c166e3` adds `deny.toml`, `cargo deny` and `cargo machete` at pinned versions, the CI workflow and its pin check, and the package-naming check. |
| Package decisions | `world-model` exists because the engine, the application, spatial-lib and the runner all consume it. `world-runtime`, `gpu-world`, `world-store` and `world-render` are not created: no consumer exists before 02, and empty forwarding crates are not allowed. `spatial-lib` keeps only the toroidal grid, the chunk manager and the redb backend; its key math moved to `world_model::spatial`. Commands and observations are used by the evidence driver; the runtime ports are recorded as signatures in [GPU contract](../../../reference/gpu-contract.md#runtime-ports-proposed-for-02) for 02 to create. |
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
3. **Shader validation** joins `check static` when 02 adds standalone WGSL. The legacy Bevy shaders
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
