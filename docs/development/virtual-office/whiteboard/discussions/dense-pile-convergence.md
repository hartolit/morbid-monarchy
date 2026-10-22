# Convergence of the body solver in dense piles

**Supported use.** Bodies piled on each other: an attracted cluster, a heap at the bottom of a
slope, spheres dropped into a crater. The GPU owner solves body pairs with a per-body Jacobi
gather (the [GPU contract](../../../../reference/gpu-contract.md#bodies-04)): 16 position and 16
velocity passes per substep by default, at most 16 each.

**Observed (04, executed).** In the `bodies` suite's `dense-cluster` case (512 bodies of radius
2–8 crushed together by attraction on sand, then settling 240 ticks), at `e8ee688` on the RTX
5070 Ti: once settled 60 ticks, no pair overlaps by more than 0.31 of its smaller radius at the
default passes (0.57 at 8 + 8, 0.90 at 4 + 4); the mean count of pairs overlapping by more than
0.4 units over the settle phase is 13 (91 and 482). The deepest pairs are light bodies wedged
between heavy ones. The case declares a limit of 0.4 of the smaller radius. Bins and all pairs
agree bit for bit, so this is the solver, not the broadphase.

**Why the current design converges slowly here (inference).** Jacobi averaging moves a body by
the mean of its partners' corrections, so a correction propagates through a pile one partner per
pass; a heavy body with many partners moves little per pass. Equal position splits, the terrain
slide and the 16-pass default each reduced the residual measurably during 04; more passes cost
little in this case (the dense 4,096 pile is cheaper at 16 + 16 than at 4 + 4, because it is less
compressed). Since `7f9d267` the passes read each body's candidates from a list made once a
substep: at 4,096 dense bodies the 64 passes take about 0.93 ms of a 2.3 ms body pass, some 15 µs
each ([the dense body pass](../../../../reference/performance-budgets.md#the-dense-body-pass)).

**Alternatives.** (1) No change: the limit is declared and tested, and piles of frictionless
spheres (the baseline has no body friction) keep flowing anyway. (2) Raise the pass limit above
16 (the dispatch table holds 16 entries per substep): cheap to try, `dense-cluster` measures it.
(3) Graph-coloured Gauss–Seidel: deterministic if colours are assigned from the bins, converges
much faster per pass, costs a colouring pass and more dispatches. (4) Shock propagation (solve
bottom-up layers treating lower bodies as immovable): suits stacks, adds an ordering pass.
(5) Over-relax the position passes' mean for bodies with two or more corrections (measured
below).

**Measured: over-relaxing the position mean (2026-10-22, executed on a draft).** A body in a
position pass moves by the mean of its partners' corrections; the draft
(`target/drafts/dense2/pair-relaxation-9174bdf.patch` on `9174bdf`, not committed; a factor and a
minimum partner count read from the environment when the shader is assembled) multiplies that
mean by a factor. At 1.0 it reproduces the shipped `dense-cluster` bit for bit. Headless on the
RTX 5070 Ti, single runs (the cases are deterministic, but a pile's settled shape changes chaotically
with the factor, so neighbouring factors differ more than a trend):

| Factor, from two corrections | 16 + 16: settled relative overlap / deep pairs | 8 + 8 | 4 + 4 |
| --- | ---: | ---: | ---: |
| 1.0 (shipped) | 0.31 / 13.0 | 0.57 / 91.1 | 0.90 / 482 |
| 1.4 | 0.26 / 8.1 | 0.42 / 68.3 | 0.70 / 434 |
| 1.6 | 0.25 / 3.6 | 0.29 / 59.4 | 0.68 / 341 |
| 1.8 | 0.28 / 5.2 | 0.55 / 56.5 | 0.80 / 289 |
| 2.0 | 0.20 / 6.6 | 0.53 / 46.8 | 0.66 / 293 |

Applied to every body, the factor overshoots an isolated pair (two spheres, one correction each,
part by the factor times their depth): at 1.8 `pair-collision`'s head-on spheres never touch and
pass through each other; at 1.6 it still passes. Applied only to bodies with two or more
corrections this pass (an isolated pair stays exact), every factor up to 2.0 passes it, and at 1.6
all 33 `bodies` cases pass. At 1.6 the less compressed piles are also cheaper: `body-scaling-bins`'
dense 4,096 body pass 3.80 → 2.79 ms (candidates 4.60 → 4.27 million a tick, residual overlap
10.0 → 7.7), dense 1,024 1.39 → 1.32 ms (overlap 2.34 → 1.62), 8 + 8 at 4,096 6.75 → 5.53 ms;
`capacity-tick-phases` body-contact 2.28 → 2.23 ms (contacts 92,000 → 86,000); the legacy workloads
0.45 → 0.46 and 0.60 → 0.57 ms, their residual overlap 1.27 → 0.70 and 1.02 → 0.94, their worst
residual approach 32 → 42 and 61 → 48 units/s. It does not let fewer passes reach the shipped 16 +
16's depth: 8 + 8 at 1.6 keeps 59 deep pairs against 13.

Not measured: the connected scenario (its step 6 rests and rebases a dense pile), the window,
and whether resting piles creep or jitter more (no case measures creep; the 33 cases' rest and
pile checks pass). The terrain contact, the other half of the dense body pass (about 1.2 ms, most
of it the bodies that reach the ground probing it up to six times a substep), is untouched by it;
an exact early-out saved 0.065 ms and fewer probes would change the contact manifold.

**Assessment.** (5) is the smallest change that improves both measures the discriminating test
names: deeper convergence at the default passes and a cheaper dense pass, at one constant and a
condition in `pairs_position`. It changes what the solver computes, so it needs physics revision
3 (worlds saved under revision 2 are refused, which before release the user accepts), and before
adoption the connected scenario headless and native, the capacity profile in the window and a
creep measure for resting piles. (1) remains the no-change option: 0.4 of the smaller radius is
declared and met. (2)–(4) cost more passes or dispatches for the same goal and are unmeasured.

**What would change the decision.** Visible interpenetration in the native client's piles (06),
or a gameplay rule that depends on contact depth. The discriminating test is `dense-cluster`'s
settled relative overlap and deep-pair count at equal or lower body-pass time in
`body-scaling-bins`' dense cases.
