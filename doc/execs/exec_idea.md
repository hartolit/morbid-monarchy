# Workflow: @idea (God Tier Idea Discovery + Execution Planning)

Use this workflow when the user triggers `@idea` or requests broad exploration, feasibility analysis, and implementation planning.

## Non-Negotiable Rules
1. **Single-Idea Boundary:** one active idea file is the only executable source of truth.
2. **Evidence First:** do not converge on solutions before inventorying evidence and gaps.
3. **Approval Gate:** never execute code changes until user says `Execute`.
4. **Living History:** preserve decision path (including rejected options), not just final plan.
5. **Containment Over Spread:** new tooling must be isolated to the right domain/module; do not bloat core runtime pathways.
6. **Release Safety:** diagnostics/debug features must be behind explicit compile-time or runtime feature gates.
7. **Complete-or-Do-Not-Claim-Done:** no half-built flows, dead code paths, or TODO placeholders in delivered scope.

## Workflow Steps
1. **Create/Select Active Idea File First**
   - Path: `doc/ideas/YYYY-MM-DD-short-title.md`.
   - Initialize with raw prompt/context before proposing solutions.

2. **Enforce Idea Boundary Isolation (Critical)**
   - Keep executable guidance for the active idea only in that file.
   - Do NOT place workflow/tooling improvements in unrelated idea files.
   - If contamination exists, sanitize it immediately before continuing.

3. **Capture Evidence Inventory**
   - Record available evidence: logs, counters, traces, benchmark data, prior reports.
   - Record missing evidence required to discriminate between options.

4. **Map Problem Space**
   - Expand broad request into candidate problem statements.
   - List architecture constraints from `doc/architecture.md` and domain boundaries.
   - Capture unknowns and score uncertainty (`high`, `medium`, `low`).

5. **Generate Multiple Options (No Early Collapse)**
   - Propose at least 2-3 options.
   - For each option, document:
     - architecture fit,
     - performance risk profile,
     - regression vectors/blast radius,
     - complexity/cost.

6. **Feasibility + Risk Verdict**
   - Label each option: `Feasible`, `Feasible with Mitigations`, or `High Regression Risk`.
   - If all options are high risk, provide safest fallback path.

7. **Decision Checkpoint (If Ambiguous)**
   - If 2+ options remain plausible, define discriminating experiments.
   - State falsification signals for each plausible option.

8. **Execution Mode Selection (Credit + Throughput Control)**
   - Default to **single-pass vertical slice** for bounded work.
   - Use phased delivery only when risk is structurally high or user explicitly requests phases.
   - If phased, cap to minimal gates (e.g., Design -> Implement -> Validate), not micro-iterations.

9. **Draft Strict Execution Plan**
   - Produce checklist (`[ ]`) for preferred option.
   - Include measurable validation gates and explicit regressions to avoid.
   - Include explicit **module ownership map** (exact files/modules to add/modify).
   - Include explicit **feature-flag plan** for debug/diagnostic surfaces.

10. **Prompt Output Contract (Diagnostics/Performance Ideas)**
    - Output a copy-ready `@audit` prompt template populated from captured evidence.
    - Include a `Capture Validity` section that marks data as valid/invalid and explains why.

11. **Wait for Approval**
    - Do NOT execute implementation changes yet.
    - Wait for user to say `Execute`.

12. **Execution + Documentation Discipline**
    - Execute checklist in order once approved.
    - Continuously update the same active idea file with discoveries and decisions.
    - If acceptance gates fail, stop and report failure explicitly (do not silently proceed).

13. **Definition of Done Gate (Mandatory Before Completion)**
    - Cargo checks pass for affected crates.
    - Planned runtime or diagnostic flows are complete and wired (no dead paths).
    - Instrumentation returns non-zero/sane values in at least one known-active scenario.
    - Output clearly maps back to initial goal and rejection criteria.

14. **Closeout**
    - Append concise summary to `doc/CHANGELOG.md`.
    - Keep changelog concise (recent milestones only).

## Idea Document Contract (Consolidated; SSOT)
Use this exact structure in every idea file:

```md
# Idea: <title>

## Raw Idea Input
<original user idea/thought>

## User Goal (Plain Language)
<single paragraph: what success feels like for the user>

## Context
<what system/domain this touches>

## Evidence Inventory
- Available evidence:
  - ...
- Missing evidence required for decision:
  - ...

## Constraints (from doc/architecture.md)
- ...

## Assumptions
- ...

## Unknowns / Questions
- [ ] [high|medium|low] ...

## Goal -> Signal Traceability
| Goal | Required signal | Collection point | Validation threshold |
| --- | --- | --- | --- |
| ... | ... | ... | ... |

## Options Explored
### Option A - <name>
- Architecture fit:
- Performance risk:
- Regression vectors:
- Complexity/cost:
- Verdict: Feasible | Feasible with Mitigations | High Regression Risk

### Option B - <name>
- Architecture fit:
- Performance risk:
- Regression vectors:
- Complexity/cost:
- Verdict: Feasible | Feasible with Mitigations | High Regression Risk

## Preferred Direction
<selected option and why>

## Decision Checkpoint (if 2+ options remain plausible)
- Discriminating experiment:
- Falsification signal for Option A:
- Falsification signal for Option B:

## Delivery Mode
- Selected mode: Single-pass | Phased
- Rationale:

## Execution Plan (Approval Gate)
- [ ] ...
- [ ] ...

## Module Ownership Map
- `path/to/file_or_module`: reason for ownership

## Feature Flag Plan (Debug/Diagnostics)
- Compile-time flags:
- Runtime flags:
- Release-default behavior:

## Prompt Output Contract (Diagnostics/Performance Ideas)
- Copy-ready `@audit` prompt template:
  - Symptom signature:
  - Reproduction steps:
  - Environment/build:
  - Captured evidence summary:
  - Regressions to avoid:
  - Capture validity (valid/invalid + reason):

## Regressions to Avoid
- ...

## Validation Plan
- [ ] ...

## Definition of Done Checklist
- [ ] Cargo checks pass for affected crates
- [ ] No half-built runtime flows or dead paths
- [ ] Signals are non-zero/sane in active scenario
- [ ] Implementation maps to stated user goal

## Execution Log (Living History)
- <timestamp> <decision/change>
- <timestamp> <finding>

## Final Outcome
<what shipped / what was rejected / next steps>
```
