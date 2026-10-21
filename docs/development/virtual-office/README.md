# Virtual office

The office holds temporary assignments and concise handoffs. It is not a second architecture or
status system. [Workflow](../workflow.md) owns implementation procedure and evidence requirements.

## Active work

| Sprint | State | Next authorized starting point |
| --- | --- | --- |
| None | – | A new sprint, when the user assigns one. |

The GPU transition (00–07) closed on 2026-10-21; its [qualification](../qualification.md) keeps
the final acceptance, and Git keeps its prompts and handoff
(`git log -- docs/development/virtual-office/gpu-transition`).

## Execute one assignment

A sprint prompt names an outcome, owner/scope, dependencies, implementation checkpoints, checks and
stop condition. Assign prompts individually. Do not infer authorization to continue because the
next file exists. A stage may take several contexts: keep one current handoff and resume the same
stage instead of producing an incomplete replacement prompt.

The coordinator owns shared architecture, manifests and handoff edits. Parallel agents may inspect
or implement independent bounded areas only with explicit file/responsibility ownership and an
integration owner. Dependent stages do not run concurrently. A claimed independent review names
the actual reviewer/session and evidence; different invented pseudonyms do not establish consensus.

Commit by coherent tested responsibility. Record the last tested commit, actual commands/results,
remaining blockers and next action. Keep large logs under ignored `target/` or retained artifacts.
Use the whiteboard for broader questions, not ordinary defects inside the assignment.

## Close a sprint

First complete acceptance and preserve the final evidence. Move lasting choices and operating
recipes into their maintained owners; update status/roadmap and fix inbound links. Remove obsolete
production paths and final-check them before deleting their historical instructions.

Then remove the completed sprint directory and office entry in a reviewed local commit, unless the
user explicitly requests retaining the assignment. Git preserves history. Keep the office and
whiteboard. Do not remove source/evidence needed for reproducibility without retaining it elsewhere.
An unfinished or failed qualification is not a completed sprint to clean away.
