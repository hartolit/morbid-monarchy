# 01 — Preserve behavior, measure the baseline and establish seams

Execute after 00 is complete. This stage establishes evidence and framework-free semantics before
GPU algorithms are changed. Do not implement the GPU solver or remove the current application yet.

## Read and scope

Read the current handoff, acceptance/source audit, architecture, workflow, performance and simulation
correctness practices, and the GPU/world-format/budget references. Inspect all 28 source tests and
the actual input/render/storage consumers. Scope follows those responsibilities, not only listed
files. Keep the original runnable app as an isolated baseline until 06.

## Checkpoint A — A buildable, identified baseline

Inspect available Rust/dependency versions and select a mutually compatible supported set. Preserve
the manifest-declared baseline before upgrades; choose the smallest compatibility correction needed
to build it. Record any unavoidable change separately from performance work. Commit an actual
`rust-toolchain.toml` and application `Cargo.lock`; remove the lockfile ignore. Do not copy stale
version declarations from retired docs or assume today's latest libraries work together.

Build/list/run the existing focused tests. Preserve failed tests as baseline failures, not silently
fixed expectations. Retain an identifiable source checkpoint/artifact and old-writer save fixtures
before changing semantics. Capture fixtures in temporary stores, never the user's only world.
Record any baseline build failure, exact diagnostic and bounded corrective action.

## Checkpoint B — Real measurements and test intentions

Create `tools/evidence` as a Rust development leaf with the alias and command contracts defined in
workflow. Implement source/config/environment identity, pass/fail/unavailable/not-run states,
nonzero nested failure propagation and test discovery/count checking. Implement useful static/docs/
CPU lanes now. Unknown/unimplemented future GPU suites must fail explicitly, not return placeholder
success. Provide runner/parser negative tests. No Python/shell implementation of the evidence core.

Instrument the existing CPU simulation/render transfer boundary without changing its decisions.
Measure full-grid copies/uploads, retained payload memory, frame work and body candidate growth.
Where the old app's renderer supports timestamps, measure them; otherwise report timing unavailable
and retain end-to-end evidence. Counters are per purpose and actual bytes, not source estimates
presented as hardware traffic.

Freeze scenario input/configuration files for idle, flow, body-contact, mixed, streaming and coupled
cases. Record geometry, camera, seed, ordered inputs, body properties, enabled features, quality,
tick duration and run lengths. Capture the qualification/scale baseline on the nominated hardware
when accessible. Do not benchmark only settled cells. Keep raw samples and baseline source identity.
If hardware is unavailable, the measurements remain unqualified and the stage's evidence gate is
not complete; a stub or software timing does not replace the target run.

Map every inspected test to a stable coverage identity, preserved expectation and destination.
Capture visual/tool feature coverage, including current rendering quality and tuning controls.
For stochastic legacy cases, record scheduling-dependent randomness; do not call a seeded scene
an exact-output oracle when the solver still reads thread RNGs.

## Checkpoint C — Extract semantic ownership without algorithm drift

Introduce `world-model` only as an adopted owner for packed-cell meaning, material identities,
coordinates, units, supported configuration values and reusable generation. Migrate the existing
consumers to these definitions and delete duplicate types/defaults. Keep Bevy, wgpu, redb and worker
lifetimes out of the model. Move suitable spatial math rather than wrapping the entire CPU chunk
manager as a generic execution port.

Preserve current cell bytes and material semantics during extraction. Test hand-reviewed field
boundaries and endian conversion independently. Identify capacity problems such as quenching at the
elevation ceiling; record their intended corrected rule for 03 rather than obscuring them inside
an extraction diff. Pure body/material reference helpers belong in the evidence leaf as needed;
no production fallback solver is added.

Confirm the architecture's actual package/dependency boundaries. Define typed commands, observations,
identities and the required runtime ports with real consumers in mind. Create runtime/adapter crates
only when the next stage adopts them; do not leave empty forwarding crates. Set physical speed,
radius, time, body/queue/working-set limits and numerical tolerances with units and rationale. Freeze
the initial performance profiles and resource envelopes before GPU tuning; record any justified
calibration change now, not after candidate failure.

## Checkpoint D — Enforceable working practices

Implement repository checks for current dependency direction, reference-code isolation, package
introductions, local links and obsolete rule files. Set meaningful warning-denying checks for new/
migrated code. Record finite baseline exceptions only when necessary, with exact owner and removal
stage; no global allow that silently shelters new code. Preserve the normal build while migrating.

Add CPU/static CI using the same runner. Describe the GPU lane without commandeering an existing
self-hosted runner or pretending hardware exists in hosted CI. Establish assembled-shader validation
as a required lane once shaders are introduced. Select a compatible wgpu/window/UI family based on
resolved APIs; pin it when used, rather than scattering unverified version guesses across docs.

## Acceptance and evidence

A01: source baseline, all test intentions and visual/tool surface retained. Evidence runner passes
its own negative cases and cannot report a missing suite as pass. Semantic extraction is used by
current consumers and preserves fixture meanings. Baseline measurements identify their hardware
and limits; frozen profiles and tolerances precede optimization.

Run affected original/model tests, runner tests, docs/dependency checks, selected format/Clippy/
rustdoc checks and the baseline profiling scenario. The expensive final full-system gate remains
07. Preserve measured failures and unqualified items honestly.

Commit each coherent checkpoint. Update status/handoff with exact commands, artifact location,
package decisions, physical limits and next action. Stop before 02; no GPU physics implementation.
