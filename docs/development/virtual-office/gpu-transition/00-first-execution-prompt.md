# 00 — First execution: adopt and reconcile

Execute only this prompt. This stage changes documentation and assignment state, not application
behavior, dependencies or saved-world bytes. Stop after the adoption handoff/commit; do not start 01.

## Read

Read root `AGENTS.md`; product vision/status; architecture; development workflow and implementation/
documentation practices; this sprint's README, source audit, acceptance and handoff. Inspect current
manifests, startup, terrain/body execution, rendering and persistence. The uploaded archive may not
match the working checkout.

## Establish the actual starting point

Inspect `git status --short`, branch and commit. Preserve unrelated work. Compare the source audit
with the checkout, using symbols as well as paths. Record actual differences that affect the plan;
do not overwrite newer good work merely to reproduce the supplied archive. Identify the existing
save path and explicitly exclude the user's only database from experiments.

Confirm the requested result: GPU owns live terrain and bodies; CPU owns intent/residency/I/O;
rendering shares the GPU state; the final app uses standalone ECS but not the Bevy engine. This
request authorizes a transition sprint, not an unrelated new game or physics feature set.

## Adopt the documentation coherently

1. Reconcile `docs/architecture.md` with actual source and mark target versus current behavior.
   Keep the proposed package map, but allow 01 to replace a boundary with a module if consumers
   do not justify a crate. Do not scaffold future crates in this stage.
2. Retire the obsolete `docs/persona.md`, `docs/rules.md` and
   `docs/knowledge/rust_knowledge.md` after checking their useful intent is represented by the new
   entry point/practices. Resolve stale links and remove an empty directory only when safe.
   Do not leave two competing active rule sets or delete unrelated newer docs.
3. Preserve strict implementation quality while removing inherited blanket rules that would harm
   this project: one crate per feature, ECS as the only SoA owner, blanket inlining, and treating
   `const fn` as a guarantee of compile-time execution. Keep measured hot-path/lifecycle rules.
4. Update decision states to distinguish requested direction from adopted engineering policy.
   Initial timing/resource profiles remain proposed values pending 01 baseline calibration.
5. Confirm that source-derived claims are not written as executed tests or measured speedups.
   The existing event type/queue is not a working gameplay consumer, and there is no evidence runner.

## Fix one connected acceptance story

Retain `resident-world`: sphere across sand into water; GPU-resolved material edit; useful CPU
command/body observation; focus movement and chunk eviction; return/reload; complete checkpoint
and reopen. Its adversarial version delays transfers, reorders loads and fails a save. The same
story must reach the actual native renderer/tools in 06–07, not stop at a headless harness.

Review the acceptance matrix against actual source. Preserve all current test intentions and
interactive features, including thermal/fire/growth, possession, attract/lift and lighting tools.
An actual conflict or unsupported existing behavior is documented with evidence; do not silently
remove it from the matrix.

## Deliver and verify

Update status with the actual source baseline and this docs-only adoption. Update the handoff with
source identity, documentation changes, deviations requiring 01 attention, and exact next prompt.
Run available documentation/local-link and whitespace checks. The proposed Cargo evidence command
does not exist yet; do not claim it ran. A source-only check is sufficient for this stage.

Review the staged diff, commit the coherent documentation adoption locally, and report the actual
commit and checks. Do not change runtime code or upgrade the toolchain. Do not execute 01.
