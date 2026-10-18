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
compressed).

**Alternatives.** (1) No change: the limit is declared and tested, and piles of frictionless
spheres (the baseline has no body friction) keep flowing anyway. (2) Raise the pass limit above
16 (the dispatch table holds 16 entries per substep): cheap to try, `dense-cluster` measures it.
(3) Graph-coloured Gauss–Seidel: deterministic if colours are assigned from the bins, converges
much faster per pass, costs a colouring pass and more dispatches. (4) Shock propagation (solve
bottom-up layers treating lower bodies as immovable): suits stacks, adds an ordering pass.

**What would change the decision.** Visible interpenetration in the native client's piles (06),
or a gameplay rule that depends on contact depth. The discriminating test is `dense-cluster`'s
settled relative overlap and deep-pair count at equal or lower body-pass time in
`body-scaling-bins`' dense cases.
