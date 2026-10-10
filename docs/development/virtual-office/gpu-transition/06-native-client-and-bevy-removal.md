# 06 — Native client, shared rendering and Bevy engine removal

Execute after 05. Deliver the usable standalone application and remove the Bevy engine from its
normal dependency/runtime path. Keeping `bevy_ecs` is intentional. A headless success, a bare debug
window or retaining the old app behind a fallback flag does not complete this stage.

## Read and scope

Read the handoff, acceptance A10–A12, architecture, GPU contract, performance practice and the
captured visual/tool inventory. Inspect the existing renderer/shaders, input, development tools,
lighting controls, startup and persistence consumers. Reuse the selected compatible wgpu/window/UI
family; do not change dependency versions merely to avoid understanding the current API.

## Checkpoint A — Real native composition

Make `apps/morbid` the real `morbid-app` package. Construct the same runtime, GPU owner and storage
adapter tested headlessly, then a native window/surface and `world-render` using their shared device.
There is one submission/resource timeline, not independently polled devices or a world copy between
engines. Replace Bevy app/plugin, task and input mechanisms with explicit owned native mechanisms.

Handle startup/adapter refusal, surface configuration/loss, minimize/resize and device-loss recovery
without invalid zero-size allocations or busy loops. Reuse 05's save/store/recovery command behavior.
Verify assets resolve from packaged locations when the process starts outside the repository root.
Keep workers cancellable/joinable and shutdown ownership coherent.

CPU ECS schedules orchestration and intent, not GPU physical poses. Rendering can consume ordered
GPU results without waiting for CPU observation. Handle controlled-body camera follow directly from
GPU state or through a documented measured visual prediction/interpolation strategy. A synchronous
pose readback each frame is not a solution.

## Checkpoint B — Terrain, bodies and presentation

Implement the required terrain/material layers, sphere rendering, directional/ambient lighting,
shadows and source-derived atmosphere/postprocessing controls. Separate physical data from derived
normals, instance/visibility lists and render targets. Read the real resident world/body buffers;
do not rebuild a full CPU mesh/transform world every frame.

Remove Bevy shader imports and render-world plumbing. Share only the GPU declarations that genuinely
need one ABI owner. Validate the final assembled render shaders, binding layouts and supported target
formats. Shader recompilation, bind group creation and geometry allocation must not occur routinely
because a cell changed when buffer bindings themselves did not change.

Address the measured cost of uncullable layers and repeated neighbour sampling. Choose visibility,
indirect work, normal caching or another derived strategy only against actual bottleneck evidence.
A derived cache must invalidate on physical changes and streaming; it must not become another
physical authority. Count its memory/update work and test seams and out-of-view reactivation.

Preserve a recognizable usable scene and feature-comparable quality. Byte-identical Bevy lighting
is not required; required effects/tools cannot disappear to improve timing. Record intentional
visual differences with comparable captures and their cost. Compare simulation-only or quality-
matched paths separately when the renderers are inherently different.

## Checkpoint C — Working tools, not menu placeholders

Connect observer motion, possession/release, driven movement/grounded jump, zoom/tether/cursor,
attract/lift and both sphere presets to the production command/body path. Tool selection must lead
to a working operation; handlers that only alter UI state are incomplete.

Provide actual water/lava/oil/ice/fire/sand and raise/lower terrain brushes. Resolve targeting on the
GPU and show asynchronous accepted/applied/rejected outcomes without implying an unreceived result.
Preserve simulation-family toggles, ambient-temperature control, brush settings, active-world resize,
lighting controls and useful statistics/compass/overlay behavior. Use the frozen inventory to catch
missed controls and accessible key/mouse bindings.

Present real command/contact/immersion outcomes, observed tick age, resource/queue pressure and
save/recovery status. A debug overlay may expose chosen contacts/physical heights/activity to make
failure visible. It must draw actual solver data, not a visually repaired substitute. Keep diagnostics
bounded and distinguish qualification/debug overhead from normal operation.

Route save/load/exit through the existing owner. Recoverable errors are actionable and do not silently
reset worlds, drop pending input or force the user to inspect a source log to understand save failure.

## Checkpoint D — Cut over and delete superseded production paths

After the native consumer passes focused checks, remove the old Bevy app and engine-specific
plugins/material wrappers, CPU cell/body simulation authority, duplicate configuration, obsolete
GPU upload path and replaced CPU chunk manager. Preserve extracted useful types/generation and
small independent references in the evidence leaf. Do not keep historical production code compiled
behind a compatibility feature or call renamed engine code a completed migration.

Inspect the resolved dependency graph: no Bevy engine/render/app/window/input/time/task integration
should remain in the native production closure. `bevy_ecs` and its legitimate internal dependencies
are allowed. A wildcard search for the string `bevy` is not an accurate dependency gate. Update the
lockfile, CI, launcher, assets and developer recipe together; remove stale paths and finite exceptions
whose owner was just retired.

## Checkpoint E — Interactive evidence

Register and execute the `render` GPU suite using the actual renderer offscreen for deterministic
checks where possible. Exercise the real window interactively for input/focus/resize/close and the
entire feature matrix. Offscreen pixels alone do not prove controls work. Capture reproducible scene
settings and visual evidence through the real client.

Run `resident-world` visibly with the same commands/state as the headless scenario. Inspect capture
or resource/counter evidence for shared buffers, no full-world CPU transfers and bounded submissions.
Measure the complete native frame including GPU simulation, rendering, staging, UI and frame pacing;
report any prequalification budget misses for correction, not hidden quality reductions.

## Acceptance and stop

A10–A12 are complete: usable native client, source-derived features and real interactions, correct
shared-resource rendering, coherent storage/recovery UX and no Bevy engine/CPU production authority.
Update actual run/configuration commands, architecture, status, dependency checks and handoff.
Commit coherent checkpoints and stop before the full 07 qualification.
