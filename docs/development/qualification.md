# GPU transition qualification

**Final acceptance of the GPU transition (sprint 00–07), qualified on 2026-10-21 at `eb9d8fb` on
the nominated desktop.** This page owns the transition's acceptance record after the sprint's
removal: the outcome of each acceptance item, the evidence and where it is kept, the feature
inventory the transition preserved, the baseline's dispositions and the intentional differences.
[Status](../product/status.md) owns what is implemented now; the
[performance budgets](../reference/performance-budgets.md) own the measurements and their
conditions; the behaviour registry in [`tools/evidence/coverage.toml`](../../tools/evidence/coverage.toml)
owns the test coverage, including the 28 tests that existed before the transition. The sprint's
prompts, handoff and source audit remain in Git history
(`git log -- docs/development/virtual-office/gpu-transition` finds them).

## Tested identity

| Field | Value |
| --- | --- |
| Source | `experimental` at `eb9d8fb` for every executed lane and benchmark (documentation commits followed); Rust 1.99.0 (`rust-toolchain.toml`), `Cargo.lock` tracked; release builds |
| Host | Ryzen 9 5950X (32 threads), 31 GiB, Linux 7.2.11, KDE Plasma (KWin Wayland), displays at 240 and 144 Hz |
| Adapter | `vulkan-10de-2c05`: RTX 5070 Ti, Vulkan, NVIDIA 615.78.08 |
| Evidence | Reports under `target/evidence/07*` on that machine (`report.json`, logs, raw samples); the commands are below and in [workflow](workflow.md#verification-commands-and-availability) |

## Acceptance outcomes

Each item's required outcome and minimum evidence were fixed in 00 (the sprint's acceptance
matrix). "Requalified in 07" means the item's evidence ran again on the final tree.

| Item | Outcome | Evidence |
| --- | --- | --- |
| A01 Baseline, intentions, model, runner (01) | Satisfied; requalified in 07 | The measured baseline (`6e108d6`, summaries in `tools/evidence/baselines/legacy-6e108d6/`), all 28 original tests registered in `coverage.toml` and run as CPU tests or GPU cases, `world-model` with hand-written goldens and the old writer's fixtures, the runner's failing specimens (`tests/runner.rs`, `tests_cpu`, `checks::*` tests). |
| A02 ABI, limits, one context (02) | Satisfied; requalified in 07 | `transfers` suite: layout roundtrips and specimens on the device, partial dispatch, limit and allocation refusal, resize success and refusal without loss. |
| A03 Bounded asynchronous commands and readbacks (02) | Satisfied; requalified in 07 | `transfers`: delayed and reordered deliveries, full pools, stale session and slot, retry and conflict, no frame waits; the ledger as consumer. |
| A04 Every material family on the GPU (03) | Satisfied; requalified in 07 | `cells` suite: the ten simulation and five cell intentions, exact transfers, receivers, ceiling quench, keyed fire and growth, classified sinks and sources, the unmatched-transfer specimen. |
| A05 One clock, correct sparse work (03) | Satisfied; requalified in 07 | `cells`: dense/sparse agreement, sleeping receivers, reactivation, negative and seam coordinates, rebasing, presentation rates and catch-up; an idle window visits no tile. |
| A06 Coupled GPU spheres (04) | Satisfied; requalified in 07 | `bodies` suite: the twelve sphere intentions and apportionment, the broadphase against an exhaustive oracle, clusters, fast bodies, shared deformation, the residency edge. |
| A07 Useful interactions across the boundary (04) | Satisfied; requalified in 07 | `bodies`: every body command through receipts, rays, observation delivery under pressure, observation age at most two ticks; `resident-world` steps 2–3. |
| A08 Metadata-only residency, safe movement (05) | Satisfied; requalified in 07 | `streaming` suite and `resident-world` steps 4 and 6. |
| A09 Consistent checkpoints, failed saves, import (05) | Satisfied; requalified in 07 | `streaming`, `world-store` tests, `resident-world` steps 5–6, the scripted start screen's import and refusal. |
| A10 Native renderer on the physical state (06) | Satisfied; requalified in 07 | `render` suite, the real surface in the `features` and `loss` scripts, `resident-world` in the window, the [packaged fresh start](#packaged-fresh-start) (no assets, any working directory). |
| A11 Interactive features (06) | Satisfied; requalified in 07 | `features` (41 checks, three runs), `loss` (6) and `worlds` (5, with the damaged-save refusal) in the real window on `eb9d8fb`'s tree. |
| A12 Native composition replaces the engine (06) | Satisfied; reviewed in 07 | `check static`'s per-package closures (only the standalone ECS crates), the deleted baseline, no reference code outside the evidence leaf, one owner per default, no unbounded production queue. |
| A13 Measured performance (07) | Satisfied on the nominated hardware | Five valid runs per case of the qualification, scale and capacity profiles on `eb9d8fb`, none contended: every qualification run within p95 ≤ 16.67 ms and p99 ≤ 25 ms at 60 ticks/s; scale at 60 ticks/s with no frame over 16.67 ms (the baseline: 17–30 ticks/s); capacity admitted with its limits reported; no full-grid transfer; the CPU's share of a tick 30 to over 100 times smaller; no frame wait; observations at most two ticks old. The p95 of idle and streaming above the baseline's is explained by uncapped presentation's acquisition waits. Tables and conditions: [performance budgets](../reference/performance-budgets.md#integrated-qualification-07). |
| A14 Integrated and adversarial load (07) | Satisfied | `resident-world`, all six steps, headless and in the native window ([connected scenario](verification-evidence.md#connected-scenario)); step 6 checks exact totals against the ledger's tools, reactions and compaction across dense bodies at a chunk and tile seam with edits and phase changes, every deferred event delivered, stale handles, a full body table refusing, a rebase across negative coordinates, held and reversed deliveries, failed writes and a failed and retried save, a resize under pending transfers, device loss, process interruption before and after publication, orderly close and a fresh reopen. The packaged fresh start and the damaged-save refusal: [below](#packaged-fresh-start). |
| A15 Full checks, truthful documentation, removal (07) | Satisfied | `cargo evidence check all` (152 of 152) and the native scenario on `eb9d8fb`'s tree; the 28 original intentions registered and executed; deliberate failures caught (the [specimens](verification-evidence.md#deliberate-failure-specimens), and in 07 enclosing reports that failed for one nested item: the gate on `93774e0` for step 6, the gate before `d9bffb6` for a rustdoc link and a runner test, the first capacity benchmark for one case); commands, configuration and versions recorded here and in the budgets; status rewritten from the measurements; the sprint removed in a reviewed commit with its inbound links fixed and `check docs` run after it. |


## Packaged fresh start

On 2026-10-21 the release binaries of `eb9d8fb`'s tree (`morbid`, `native-client`,
`world-store`) were copied into an empty directory outside the repository and run from an empty
working directory with an empty home directory and no XDG variables, as a first start on another
machine would (`target/evidence/07e-packaged/`):

- `morbid --help` printed its options and exited 0;
- the `worlds` script, with the client's default directories, created a world from the start
  screen, imported a copy of the baseline's save, refused a copy with one undecodable row
  ("Import refused: 1 legacy rows do not decode"; the copy unchanged, no world listed), opened the
  new world, closed it with its save and reopened it at the closed tick;
- `world-store import-legacy` refused the same damaged copy, wrote nothing and left it unchanged;
- `morbid --world Scripted` opened that world from the default directory and, asked to close
  through the compositor as a person's click would, saved ("closed and saved; checkpoint 4 holds
  tick 578") and exited 0; `world-store inspect` and `verify` then read the checkpoint back;
- the only files written were the two worlds under the new home directory; the working
  directory and the repository were untouched.

## Commands

Run on the nominated machine with `CARGO_BUILD_JOBS=4`; reports under `target/evidence/`:

```sh
cargo evidence check all --adapter vulkan-10de-2c05                      # 07f-check-all
cargo evidence run --scenario resident-world --adapter vulkan-10de-2c05 --consumer native   # 07f-native
target/release/native-client script features|loss|worlds ...              # 07e-scripts
cargo evidence bench --profile qualification --adapter vulkan-10de-2c05 --runs 5   # 07-bench-qualification
cargo evidence bench --profile scale --adapter vulkan-10de-2c05 --runs 5           # 07-bench-scale
cargo evidence bench --profile capacity --adapter vulkan-10de-2c05 --runs 5        # 07-bench-capacity
cargo evidence bench --profile capacity --case coupled --adapter vulkan-10de-2c05 --runs 5   # 07-bench-capacity-coupled-2
```

The packaged run's script and the presentation diagnostics are kept with their reports
(`07e-packaged`, `07g-presentation`). The measured baseline's summaries are tracked in
`tools/evidence/baselines/legacy-6e108d6/`; its raw samples, like these reports, exist only on
the nominated machine.

## Feature preservation inventory

The transition preserved this inventory, reconciled with the baseline checkout `a9c1539` in 00
and frozen with the supported ranges in 01 (`world_model::config`, the
[GPU contract](../reference/gpu-contract.md#supported-physical-envelope-fixed-in-01)). The native
client's `features` script exercises every group through the real input path and checks each
against its command outcome ([workflow](workflow.md#scripted-runs-of-the-native-window--06));
removing a behaviour needs an explicit scope change, not a new label.

| Feature group | Required usable behavior | Important boundary |
| --- | --- | --- |
| Physical terrain | Packed columns, granular movement/erosion, water/oil/magma flow, thermal freeze/thaw at ambient temperature, magma quench and boiling, combustion and biological growth. | The GPU rules are authoritative; display is not a substitute for physical accounting. |
| Simulation controls | Toggles for liquid, granular, thermal, fire and biology; ambient temperature (−30 to 40 °C); tick rate (10–240 Hz); catch-up limit (1–16 ticks/frame); elevation scale. | Toggling a rule/climate invalidates/wakes the required state; no hidden second clock. Body substeps divide the selected tick. Elevation scale is physical configuration (below). |
| Brushes | Water, lava, oil, ice, fire, sand, raise terrain, lower terrain; radius 0–64, strength 1–255; cursor preview. | Actual GPU ray/hit/edit result, capacity outcome and visible target; no full-grid CPU fetch. The preview may be provisional. |
| Bodies | Metal (relative density 7.8) and wood (0.6) sphere spawning at the picked surface; terrain contact/rolling/crater/rut, buoyancy and fluid displacement. | Shared-cell/body conflicts are resolved, not overwritten by independent writes. |
| Direct control | Observer flight and walking (noclip toggle, terrain-following walk, sprint); attaching/detaching the residency focus; possession/release of the nearest sphere, driven motion, grounded jump, attract and lift. | Normal commands and observed/GPU presentation state; no writable CPU physical transform. Walking and target picking use GPU queries or observations, not CPU terrain reads. |
| Camera/input | Existing zoom, tether, right-button look/cursor capture and tool input; sensible native focus/resize behavior. | Camera following cannot require a same-frame blocking body readback. |
| Presentation | Terrain/material layers and spheres; sun azimuth/elevation, sky brightness, heightfield terrain shadows and ambient occlusion with their controls; directional shadow maps, distance fog and bloom. | Comparable quality documented; no obligation for pixel-identical Bevy output. |
| Inspection | Statistics (FPS, total liquid volume, total elevation), a compass showing heading, pitch and sun, an overlay toggle, and selected physical debugging. | Totals come from exact GPU integer reductions, not a full-grid CPU fetch; add resource/tick/observation pressure; no fabricated telemetry. |
| World controls | Active-world resize (radius 1–32 per axis), normal streaming, safe save/exit/reopen and explicit import/recovery. | Failure preserves current valid state or reports the documented loss boundary. At this cell size, radius 32 exceeds a default 128 MiB storage binding, so it is supported or visibly refused. |

New audio or damage systems, arbitrary rigid shapes or joints, multi-species fluid fields,
networking and catch-up of unloaded regions are not implied by this inventory.

### Presentation reference

The baseline's presentation and tuning defaults at 01, which the native client keeps or changes
with a recorded reason. The developer UI exposes the first group; the rest are fixed in code
(`Owner` names the baseline's source).

| Setting | Baseline default | UI range | Owner |
| --- | --- | --- | --- |
| Sun azimuth / elevation | 125° / 35° | 0–360° / 2–90° | `runtime/render.rs` `WorldLighting` |
| Sky (ambient) brightness | 900 | 0–3000 | same |
| Heightfield terrain shadows | 0.85 | 0–1 | same; 0 skips the march |
| Ambient occlusion | 0.6 | 0–1 | same |
| Sun light | Directional, `AMBIENT_DAYLIGHT` illuminance, sRGB (1.0, 0.95, 0.86), shadow maps on | not exposed | `spawn_sun` |
| Sky, ground and horizon colours; sun glow | sRGB (0.55, 0.70, 0.95), (0.42, 0.36, 0.28), (0.68, 0.76, 0.86); glow (1.0, 0.85, 0.65, 0.5), exponent 24 | not exposed | `WorldLighting` |
| Camera | HDR target, `Bloom::NATURAL`, linear distance fog from 0.55 to 0.98 of the distance to the loaded window's edge, refitted to the camera altitude every frame | not exposed | `configure_world_cameras`, `fit_fog_to_view` |
| Terrain | Four material layers (terrain, granular, fluid, surface) per active chunk over one shared cell buffer | – | `runtime/render.rs` |
| Overlay | Tool bar, statistics, simulation and lighting panels, compass; one toggle | – | `runtime/dev_tools` |

The visual reference is a set of frames the baseline driver saved from the frozen qualification
scenarios with these defaults, at 1920×1080 with the overlay shown:
[`tools/evidence/baselines/legacy-visual/`](../../tools/evidence/baselines/legacy-visual/README.md).
06 compares the native client with them; pixel identity is not required.

**Native presentation (06).** The client keeps the defaults above (the lighting panel's ranges and
values, the sun, sky, ground and horizon colours, the glow, the fog fitted to the window edge and
the camera altitude, the four layers over one cell buffer, the overlay's panels and compass), with
these recorded differences:

- Tone mapping is AgX in the post pass, where the baseline used Bevy's TonyMcMapface, which
  samples a lookup texture the client does not ship.
- Lighting is the baseline's landscape material ported to `world-render` (sun, sky ambient,
  heightfield shadows and occlusion, a 2048² shadow map for the spheres), not Bevy's PBR
  pipeline; bloom follows `Bloom::NATURAL`'s chain (a 13-tap downsample with a Karis average, a
  tent upsample).
- Landscape chunks are culled on the device against the view frustum; culled frames are
  pixel-identical to the unculled path (`render` `culling-matches-unculled`).
- Without vsync the client presents in mailbox mode (immediate only when mailbox is missing): it
  does not tear, and on the nominated desktop it waited less in image acquisition (see
  [performance budgets](../reference/performance-budgets.md#native-client-06)).

Compared on 2026-10-20 at the idle reference's camera (seed 69, cell (200, 230), height 150, yaw
30°, pitch −40°, 1920×1080, overlay shown), captured through the client with `native-client
capture` about 140 ticks after a new world opened (the idle case has no actions): the terrain, its four layers, the lake and shoreline, the fog band and the compass match
the reference frame; the client's sand and grass are paler (not measured; the tone mapping is the
likely cause), and its overlay adds the World and Outcomes toggles with the World panel open.


## Baseline dispositions

What the baseline at `a9c1539` did, as the sprint's source audit recorded it from source and
from 01's measurements, and where the transition left it. Paths are the baseline's (`core/` under
`crates/monarch-engine/src/`, `runtime/` under `crates/morbid-app/src/`); `9a3230c` is the last
commit that builds them.

| Baseline behaviour | Disposition |
| --- | --- |
| `WorldCell(pub u64)` packed columns; 64 × 64-cell chunks; active radius 6 | Kept bit for bit in `world_model::cell` with hand-written goldens (01); executed on the device as two words (02). |
| `swap_buffers` copied the whole grid every tick; `run_simulation_tick` traversed every cell and marked the grid dirty | The material phases visit only tiles with an awake cell and commit only awake cells; an idle window visits none (03). |
| `sync_grid_rendering` uploaded the whole grid whenever it was dirty (every tick, and after edits) | Gone: the renderer draws the device cells; edits reach the device as commands (06). |
| Liquid and granular neighbours' preferences recomputed per donor | Each transfer proposed once, resolved by its receiver against its capacity, committed with the accepted amount (03). |
| `quench` drained magma before a capacity-limited raise and lost the remainder at the ceiling | Fixed at the owner with independent fixtures (03). |
| Thread-seeded `SmallRng` for fire and biology | Keyed draws from the rule seed, position, tick and salt: a recorded semantic change (03). |
| Streamed chunks were never woken | Activation wakes the chunk and its boundary dependants (03). |
| Sphere kinematics integrated frame time; `resolve_sphere_pairs` visited all pairs; terrain contacts mutated the CPU grid | Fixed substeps of the tick on the device, hashed bins checked against an exhaustive oracle, writes under per-cell claims with exact apportionment (04). |
| Unordered Bevy `Update` systems for terrain, bodies and observer | One ordered physical tick: commands, material phases, body substeps ([GPU contract](../reference/gpu-contract.md#ordered-physical-work)). |
| CPU readers of the live grid: brush ray march, preview, spawn height, attract/lift targets, walking floor samples, full-grid statistics sums | GPU-resolved rays, column queries and totals reductions (03–06); no CPU terrain read. |
| `spatial-lib` chunk manager: an LRU cache, saves only on eviction, no dirty flag, stale reloads while a save was pending | Metadata-only residency, device change flags, capture before reuse, newest retained revision wins (05); deleted in 06. |
| Persistence: read or decode errors fell through to generation; failed writes were logged and dropped; the exit flush omitted active and cached chunks; `world_data/save.redb` opened relative to the working directory, panicking on failure | Format version 1 in `world-store`: corrupt and absent kept apart, payloads retained until durable, atomic checkpoints at one completed tick, orderly close, explicit world selection and typed errors, a non-destructive legacy import (05–06). |
| An unbounded, unconsumed simulation event queue; `runtime/hooks.rs` holding only comments | Bounded outcomes with a real consumer (the ledger, 02; the body monitor, 04); `hooks.rs` removed (01). |
| The elevation scale written live by the UI and never saved | Physical configuration: applied at a tick boundary and saved with every checkpoint (decided in 01; [world format](../reference/world-format.md#existing-packed-cell)). |
| The brush cursor marked all four landscape materials modified every frame | Gone with the Bevy materials; the cursor is a uniform of the renderer (06). |

The audit's calculated scale (one packed array is `8 N` bytes: 5.28 MiB at radius 6, 34.0 MiB at
16, 132 MiB at 32) is kept in the [performance budgets](../reference/performance-budgets.md#baseline-arithmetic).

## Intentional differences

- Rule changes of the material phases: [GPU contract](../reference/gpu-contract.md#rule-changes-from-the-baseline-03-and-after-07).
- Changes of body behaviour, each with the test that would fail without it:
  [GPU contract](../reference/gpu-contract.md#bodies-04).
- Presentation: the native presentation differences above.
- Persistence: an unreadable row is left unloaded instead of generated over; a world whose
  layout, physics or generator identity differs is refused, not converted; there is no command
  journal, so changes after the last checkpoint are lost on a crash or device loss and the loss
  report says how many ticks ([world format](../reference/world-format.md#exit-interruption-and-device-loss)).
- Measurement: a native frame sample is one presented frame; the baseline sampled its main
  schedule ([verification evidence](verification-evidence.md#measurement-outputs)).

## Not qualified

- Any adapter, driver or platform other than the one above: no compatibility claim (wgpu's
  portability is not evidence). Hosted CI runs the CPU and static lanes on Linux only; no GPU
  lane runs in CI.
- Other window systems and compositors: KWin's Xwayland path was measured once as a
  diagnostic (the same acquisition waits), not qualified; X11 sessions, GNOME and others are not.
- Other GPUs and drivers (AMD, Intel, integrated adapters, other NVIDIA drivers), software
  adapters for performance, Windows, macOS and browsers.
- Frame rates for the capacity profile, which promises none.
