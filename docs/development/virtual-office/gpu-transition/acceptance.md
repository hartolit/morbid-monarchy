# GPU transition acceptance

**All items are not run.** 00 reviewed this matrix against checkout `a9c1539` and found no
existing test intention or tool missing from it. The feature inventory below adds the controls it
had left implicit, and one conflict is recorded under it. This is the shared outcome matrix, not a
second implementation prompt or a substitute for the canonical contracts. A stage records concrete
evidence in its handoff; 01 creates the machine-readable behavior registry in `monarchy-evidence`.
07 requalifies integration.

## Acceptance matrix

| ID / first owner | Required outcome | Minimum discriminating evidence |
| --- | --- | --- |
| A01 / 01 | Identifiable buildable baseline; all source test/tool intentions retained; adopted pure model and real evidence runner. | Original test discovery/results; independent cell and old-writer fixtures; actual hardware baseline or explicitly incomplete evidence gate; failing runner specimen; frozen workload/tolerances. |
| A02 / 02 | Real ABI, supported limits and one shared GPU context/resource owner. | Production shader layout roundtrip; golden words; partial dispatch; lower-limit and allocation refusal; successful/refused resize without destroying old state. |
| A03 / 02 | Bounded asynchronous commands/readbacks with safe reuse and observable outcomes. | Real CPU consumer; delayed/out-of-order callbacks; full pools; stale session/slot rejection; same-ID retry/conflict; no steady frame waits; actual GPU suite execution. |
| A04 / 03 | Every current material family on the GPU with explicit parallel and reaction semantics. | Preserved simulation/cell intentions; independent exact transfers; frozen/mixed/full receivers; ceiling quench; deterministic fire/growth; classified sinks/sources. |
| A05 / 03 | One fixed logical clock and correct sparse work without recurring full-cell idle traversal/copy. | Dense/sparse agreement; sleeping receiver and multi-hop halo; climate/edit/chunk reactivation; empty/full activity; negative/seam coordinates; presentation-rate/catch-up cases. |
| A06 / 04 | GPU spheres couple correctly to terrain, fluids and each other. | All twelve body intentions plus apportionment; complete candidates versus small exhaustive oracle; clustered/mixed-radius/fast supported bodies; simultaneous deformation; bounded solve and body-edge policy. |
| A07 / 04 | Useful interactions cross the GPU boundary without rebuilding CPU authority. | Real spawn/control/jump/attract/lift/edit/query commands; CPU-consumed contact/immersion/result; retention/ack and overflow tests; measured observed-tick age; stale body rejection. |
| A08 / 05 | Metadata-only normal residency and safe asynchronous world movement. | Capture-before-reuse; reordered loads; dirty reload before save; old/new resize overlap; bounded retries; body suspension/pinning; no wrong wrapped neighbours or lost changes. |
| A09 / 05 | Consistent complete checkpoints, preserved failed saves and recoverable/importable data. | Resident-only autosave; chunk/body/config capture at T; atomic publication crash cases; actual fresh reopen; corruption refusal; original-writer import fixtures; in-flight shutdown; loss boundary. |
| A10 / 06 | Actual native renderer consumes the physical GPU state on the same device. | Production offscreen/render tests plus real surface run; coherent poses; resize/minimize/loss; resource-sharing capture/counters; comparable scene/effects; outside-repository asset startup. |
| A11 / 06 | Source-derived interactive features work, not merely exist in UI menus. | Completed feature inventory below with actual native input/UI evidence and command outcomes; save/recovery/pressure feedback; no hidden CPU edit path. |
| A12 / 06 | Native composition replaces Bevy engine and CPU production authority. | Resolved normal dependency closure; source/feature/path checks; no fallback old app, full CPU mirror, duplicate defaults or abandoned adapters; small reference code development-only. |
| A13 / 07 | Measured performance meets frozen profiles and supports the CPU/transfer improvement claim. | Repeated comparable raw runs; frame tails, actual ticks, CPU/GPU work, bytes, allocations/waits, memory, queue age and contention; no silent quality/work reduction. |
| A14 / 07 | All owners work together under realistic and adversarial load. | Complete `resident-world` in native/headless consumers, reordering/pressure/failure trace, checkpoint and fresh reopen; bounded capacity and explicit fault behavior. |
| A15 / 07 | Full checks, truthful qualification and coherent documentation/removal. | All required lanes pass; original 28 intentions retained; deliberate failures caught; actual commands/config/versions documented; canonical measured status; reviewed cleanup and final link check. |

An item is not complete merely because its types compile. Required GPU execution on unavailable
hardware is `unavailable`, not pass. A capacity refusal is acceptable only when consistent with the
frozen support contract and visible before unsupported mutation; it cannot be used to shrink the
ordinary qualification profile. Known failures remain attached to their owner until resolved.

## Feature preservation inventory

00 reconciled this inventory with checkout `a9c1539`. The ranges shown are the current UI
limits, not frozen support limits. 01 captures the baseline settings and visual reference and
records any later corrections. 06 proves each feature through the native consumer. Intentional
visual changes are documented; removing a required behavior needs an explicit scope change, not a
new label.

| Feature group | Required usable behavior | Important boundary |
| --- | --- | --- |
| Physical terrain | Packed columns, granular movement/erosion, water/oil/magma flow, thermal freeze/thaw at ambient temperature, magma quench and boiling, combustion and biological growth. | The GPU rules are authoritative; display is not a substitute for physical accounting. |
| Simulation controls | Toggles for liquid, granular, thermal, fire and biology; ambient temperature (−30 to 40 °C); tick rate (10–240 Hz); catch-up limit (1–16 ticks/frame); elevation scale. | Toggling a rule/climate invalidates/wakes the required state; no hidden second clock. Body substeps divide the selected tick. Elevation scale is an open conflict (below). |
| Brushes | Water, lava, oil, ice, fire, sand, raise terrain, lower terrain; radius 0–64, strength 1–255; cursor preview. | Actual GPU ray/hit/edit result, capacity outcome and visible target; no full-grid CPU fetch. The preview may be provisional. |
| Bodies | Metal (relative density 7.8) and wood (0.6) sphere spawning at the picked surface; terrain contact/rolling/crater/rut, buoyancy and fluid displacement. | Shared-cell/body conflicts are resolved, not overwritten by independent writes. |
| Direct control | Observer flight and walking (noclip toggle, terrain-following walk, sprint); attaching/detaching the residency focus; possession/release of the nearest sphere, driven motion, grounded jump, attract and lift. | Normal commands and observed/GPU presentation state; no writable CPU physical transform. Walking and target picking use GPU queries or observations, not CPU terrain reads. |
| Camera/input | Existing zoom, tether, right-button look/cursor capture and tool input; sensible native focus/resize behavior. | Camera following cannot require a same-frame blocking body readback. |
| Presentation | Terrain/material layers and spheres; sun azimuth/elevation, sky brightness, heightfield terrain shadows and ambient occlusion with their controls; directional shadow maps, distance fog and bloom. | Comparable quality documented; no obligation for pixel-identical Bevy output. |
| Inspection | Statistics (FPS, total liquid volume, total elevation), a compass showing heading, pitch and sun, an overlay toggle, and selected physical debugging. | Totals come from exact GPU integer reductions, not a full-grid CPU fetch; add resource/tick/observation pressure; no fabricated telemetry. |
| World controls | Active-world resize (radius 1–32 per axis), normal streaming, safe save/exit/reopen and explicit import/recovery. | Failure preserves current valid state or reports the documented loss boundary. At this cell size, radius 32 exceeds a default 128 MiB storage binding, so it is supported or visibly refused. |

### Open conflicts with existing behavior

**Live elevation scale.** Source (`runtime/dev_tools/ui.rs`) shows the UI writing
`GlobalPhysicsConfig::elevation_scale` (0.01–5.0) at runtime. Body contact heights, sphere spawn
placement and rendering all read it, and the value is not saved with the world. A live physical
scale conflicts with fixed physical units and with the saved-world coordinate/scale convention.
In Checkpoint C, 01 chooses one meaning and tests it: a physical setting applied at a tick
boundary, a world-creation parameter, or a presentation-only vertical exaggeration. Until then the
control remains in this inventory. It is not removed silently.

New audio/damage systems, arbitrary rigid shapes/joints, multi-species fluid fields, networking and
unloaded-region catch-up are not implied by this inventory. Preserve existing behavior and deliver
the specified CPU observations without inventing unrelated gameplay scope.

## Connected execution trace

Use a fixed effective configuration and synthetic inputs through the same normal boundary as the
native client. Freeze concrete values in 01; do not allow each stage to choose an easier scene.

1. Load/generate known chunks with a sand bank, rock support and water basin. Confirm absolute key,
   local coordinate and packed-cell interpretation. Spawn a known controlled sphere.
2. Drive across the sand into the water. Demonstrate support/rolling, bounded deformation and fluid
   displacement. Account for the expected material units, including declared reaction/tool sources.
3. Apply a material edit through a GPU-resolved ray. Consume its important outcome and a selected
   body/contact/immersion observation on the CPU. Measure identity, tick age and admitted inputs.
4. Move the resident window until modified regions leave it. Capture before reuse, continue bounded
   work, then return before a deliberately delayed save finishes. Newest retained state must win.
5. Capture a complete checkpoint at one completed tick, including bodies and configuration. Destroy
   application/runtime/store objects, reopen independently and verify promised state and identities.
6. Repeat with delayed mapping/consumers, reordered loads, a failed write, overlapping resize and
   a controlled interruption/loss. Recover from the actual last complete checkpoint, not a fictional
   command replay.

The trace grows through 04–05. 06 runs this same trace visibly, through the native client, its
renderer and its tools. 07 repeats it in the integrated run. Interactive checks of individual
tools extend this trace; they do not replace it, and a headless run alone does not complete it.

The trace is an integration obligation, not the entire test suite. Fine-grained golden tests must
locate errors and adversarial scenarios must distinguish queue policy, numerical failure and data
loss. An overlay/video cannot establish conservation or checkpoint durability by itself.

## Final qualification report

Report each A01–A15 outcome, source/configuration identity, actual commands/counts, adapter/driver,
artifact location and relevant measurements. Separate source-derived calculations, reserved resource
bytes, observed memory and measured transfer traffic. Include changed rule/visual semantics and
which hardware/backends remain unqualified.

The final gate requires the frozen contract, not a marketing speedup. Reduce simulation-related
CPU work and eliminate recurring full-world transfer while meeting correctness, input/observation
latency, memory/queue and interactive frame-time requirements. Missing or failed mandatory evidence
keeps the sprint active. 07 owns canonical publication and cleanup after successful qualification.
