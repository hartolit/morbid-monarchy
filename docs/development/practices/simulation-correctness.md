# Simulation correctness practice

The reference is the intended physical rule, not whichever output the current code happens to
produce. Preserve useful existing behaviors and tests, but challenge untested limits and accidental
ordering. The world is a stylized column simulation; do not label every conserved column unit a
physically calibrated mass or claim a general 3D solver.

## Fix units, coordinates and time

Document horizontal cell width, elevation unit, body length/mass convention, velocity, acceleration,
tick duration and each numerical tolerance. The baseline uses a 0.5 world-unit elevation scale and
1/60-second terrain ticks, with sphere substeps up to 1/120 second. These are current defaults, not
universal physical constants.

Cells and bodies must advance under one logical fixed clock after migration. State which terrain
version each body substep reads and when its deformation becomes available. Limit catch-up work and
report discarded simulation time rather than increasing a step silently. Test equivalent tick
commands under 30/60/144 Hz presentation and injected long frames.

Keep discrete absolute world identity separate from local GPU coordinates. Test negative chunk
coordinates, cell corners/centres, axis sign changes, window shifts and rebase events. A toroidal
array index is not permission to access a logically absent neighbour.

## Account for material changes

Use integer volume/elevation accounting for the packed-column rules. Closed flow and displacement
must preserve the relevant total exactly. A donor cannot lose more than its receiver actually
accepts. A full receiver, mixed material, frozen column or elevation ceiling must not cause silent
loss. Rejected work stays at the source or is explicitly not admitted.

Define reaction accounting separately. Freezing/thawing preserves stored volume. Quenching changes
material/column representation. Boiling and combustion can consume represented material; terrain
compaction can change represented volume. Tool injection/removal is an external source/sink. Give
each such change an explicit classified quantity so a diagnostic does not mistake an intended
sink for a race—or conceal a race inside an unrestricted “reaction loss” counter.

The existing one-column mixed-liquid rule may change which material label wins without preserving
per-species volumes. Do not add an impossible per-species conservation assertion while claiming
the old representation is unchanged. Any move to a multi-species field is a separate design change.

Check arithmetic at zero, every encoded maximum, near-capacity mixed reactions and negative inputs
before conversion. The supplied `quench` path drains its volume before a capacity-limited bedrock
raise; this ceiling case needs an explicit conservative replacement or declared reaction rule.
Do not preserve it merely because ordinary tests use low elevations.

## Make parallel updates well-defined

Use immutable phase inputs and a single writer/reducer for each output. Specify transfer priorities
and tie-breaking so results do not depend on workgroup scheduling. Evaluate contested body-induced
edits collectively against source availability and destination capacity. Two individually valid
body deformations can be invalid when both spend the same cell's material.

Contact generation and response are separate concerns. Broadphase must find all potentially
colliding supported bodies; narrowphase must reject false candidates. Define maximum radius/speed,
substeps or swept tests needed to prevent tunnelling within the supported contract. A small default
body is not evidence that a large or fast accepted body is safe.

A Jacobi/per-body gather solver or another parallel method must have documented convergence and
iteration bounds. Test stacked/clustered bodies, simultaneous terrain changes and degenerate
coincident centres. Do not silently fall back to CPU pair resolution or ignore pairs beyond a
fixed-size bin. Include friction, restitution, buoyancy, displacement and grounded-state stability,
not only final position.

## Distinguish reproducibility claims

Integer packing and selected deterministic rule cases can have exact comparisons. Body floating
point needs physical tolerances and invariants, not a promise of cross-vendor bitwise identity.
Fix iteration order where practical and test repeated runs on the qualified backend. Fast math,
reassociation and precision changes need evidence for the accepted error. Do not derive a tolerance
from the candidate's failure magnitude.

Stochastic simulation must take explicit seed, discrete world position, logical tick and rule salt
as inputs. GPU scheduling, wall time and tile-list order must not seed physical behavior. The
baseline biology/fire paths seed RNGs from thread-local randomness; their current outcomes are not
an exact deterministic oracle. Record the transition to keyed randomness as an intentional behavior
change. Test reproducibility, locality and that rule/cadence changes do not correlate every cell.

Pause/unload semantics must be explicit. This sprint freezes unloaded simulation state; it does
not implement arbitrary catch-up for unobserved chunks. Reactivation wakes the needed regions and
continues under the recorded rule/configuration version.

## Test independently and adversarially

Preserve the 28 supplied tests' behavior coverage. Extract fixtures without making the new GPU
implementation generate its own expected output. A shared layout declaration is useful, but include
hand-reviewed bit patterns and analytical microcases to detect an error shared by both encoders.

Use exact small cases, conservation/property tests, dense-versus-sparse comparisons, body trajectory
bounds, long settlement and failure sequences. Test active donors beside sleeping receivers, tile
and chunk seams, missing neighbours, counter rollover/reuse, simultaneous brushes/bodies, capacity
exhaustion, climate changes and reactivation. Test isolated and coupled operations; passing flow
alone does not establish body-displacement conservation.

Keep small CPU reference solvers in the evidence leaf when useful. They are not a production
fallback, a permanent full-world mirror or an unquestionable source of truth. A changed rule needs
an independently justified fixture/tolerance update and a reviewable explanation.

## Observe the right thing

Render inspection must display the real body pose and physical surface; a simplified debug model
that hides penetration is misleading. Add selected overlays for contacts, activity, bounds or
physical heights when they answer a concrete question. Overlays are derived presentation, not a
second simulation.

GPU diagnostics may reduce integer quantities per chunk and combine those small results on the
CPU; they need not read every cell back. Account for diagnostic traffic/overhead and distinguish
qualification mode from ordinary gameplay. Important errors stay observable in both modes.
