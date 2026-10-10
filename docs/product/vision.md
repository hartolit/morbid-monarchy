# Product vision

Morbid Monarchy is an interactive world whose terrain, loose material, liquids, surface life and
physical bodies affect one another. Its current representation is intentionally 2.5D: a horizontal
cell identifies a material column, while bodies move in three dimensions above and through those
columns. Preserve this useful constraint rather than translating everything into generic 3D meshes
and colliders.

## What the transition must improve

The live physical world should remain on the GPU. Terrain simulation, body contacts, material
displacement and rendering must reuse that state rather than synchronize competing CPU and GPU
worlds every frame. The purpose is lower CPU work and transfer traffic, less duplicated memory,
and room for richer interaction at a predictable frame time—not maximum shader count.

The CPU remains responsible for application control, asynchronous loading and generation,
residency decisions, persistence, input intent, and gameplay that does not require continuous
inspection of the live physical world. It communicates through bounded commands, selected
observations and explicit snapshots. A system that repeatedly needs live terrain should normally
move beside that data rather than introduce a hidden readback loop.

Retain `bevy_ecs` for CPU orchestration. The destination is a native application with explicit GPU
resource and rendering ownership, not the Bevy engine wrapped behind new names. The migration can
use the current application as a temporary baseline; the final normal runtime cannot depend on it.

## Preserve meaningful behavior

Preserve editable terrain and granular matter; flow and settlement; oil/water and magma/coolant
column rules; freezing, thawing, fire and growth; spheres that roll, climb, float, sink, crater,
press trails and displace liquid; possession, observation and tools; and chunk loading/saving.
Existing tests help identify these obligations, but do not make every incidental algorithm a law.

Behavior may be improved when a defect or unstable rule is demonstrated. Record the old behavior,
new rule and independent acceptance test before replacing expectations. Changes to physical units,
reaction accounting, terrain/body time order, save compatibility or visible feature coverage must
not hide inside an optimization.

A useful development build must let a person see and manipulate the real simulation. A headless
benchmark, a rendered heightfield with CPU physics, or a pretty scene with disabled interactions is
not the completed transition. Rendering quality is part of the comparison: reducing resolution,
visibility, effects or geometry cannot be presented as a simulation optimization.

## Limits of this program

This transition does not establish general 3D rigid bodies, joints, overhangs/caves, networking,
lockstep multiplayer, a new scripting platform, a general render-graph framework, browser support,
or a full thermodynamic model. The current ambient-temperature/phase rules do not imply a
per-cell temperature field. No new gameplay lore or feature set is inferred from the project name.

Native Linux on the user's desktop is the first qualification target. Other adapters and platforms
need their own evidence; using a portable API is not proof of portability. No discrete-GPU-only
trick is required merely to make a demonstration pass.

## Success

A controlled sphere crosses deformable ground and liquid, a tool edits the world, selected
interaction results reach the CPU, and a dirty chunk survives eviction and reload. The renderer
reads the same resident state. The supported workload passes its correctness tests and declared
resource/frame-time gates without a full-world CPU mirror or a steady-state synchronous readback.
