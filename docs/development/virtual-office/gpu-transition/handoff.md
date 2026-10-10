# GPU transition — current handoff

Maintain this one current record; replace stale working notes rather than accumulating a session
transcript. Canonical design/status belongs in maintained docs, and raw evidence belongs in retained
artifacts. Update after each coherent checkpoint and before stopping or changing context.

## 01 progress

**Assigned:** 01, by the user, on 2026-10-10. **Checkpoint A is complete.**

- `eea8652` pins rustc 1.99.0 (`b940084d7`) in `rust-toolchain.toml` and tracks the existing
  `Cargo.lock`, which is now verified. `cargo fetch --locked` and `cargo build --locked
  --workspace --all-targets` both succeed, with no compiler warnings. `cargo test --locked -p
  monarch-engine` passes 28/28, and 25 repeated runs had no failure. `spatial-lib` has no tests.
- The baseline is not lint-clean. `cargo fmt --check` reports 3 import-order diffs (in
  `monarch-engine` `core.rs` and `prelude.rs`). Warning-denying Clippy finds 31 warnings, all in
  the three legacy crates. `RUSTDOCFLAGS='-D warnings' cargo doc` passes.
- `58e9648` adds old-writer fixtures in `crates/morbid-app/tests/fixtures/legacy-v0/`. They were
  captured from a scratch directory, and the user's save was not opened.
- Hardware: this machine is the nominated Ryzen 9 5950X + RTX 5070 Ti (driver 615.71.09, KDE
  Wayland). Only the compositor and Discord use the GPU.

**Next:** Checkpoint B, starting with the `tools/evidence` runner.

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
5. **Elevation-scale conflict.** Decide its meaning in Checkpoint C; see
   [acceptance](acceptance.md#open-conflicts-with-existing-behavior).
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
