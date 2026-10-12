# monarch-engine

The CPU baseline of the physical world, packaged as Bevy plugins. It contains:

- the double-buffered cellular automaton (`core/simulation*`): liquid, granular, thermal, fire and
  biology rules over the active grid, on a fixed clock with a catch-up cap;
- sphere bodies (`core/entities/spherical.rs`): integration, heightfield contact, deformation,
  buoyancy and liquid displacement, writing the CPU grid directly;
- the observer and the systems that move the active window.

What a cell, a material, a chunk key, a unit or a supported setting means is not defined here:
[`world-model`](../world-model/README.md) owns it, and this crate imports it. Prompt 01 moved the
packed cell, the material properties and the generator there, along with the five column-rule
tests. The cell and body solvers become GPU kernels in prompts 03–04, and the CPU production path
goes away in 06. Until then, the crate is the behavior reference that the
[coverage registry](../../tools/evidence/coverage.toml) pins: its 23 remaining baseline tests,
each with a behavior ID and the stage that carries it forward.

`core/counters.rs` defines `EngineCounters`. It records ticks, copied bytes, body pair tests and
solver time for baseline measurement, and nothing reads it to decide anything. See
[status](../../docs/product/status.md) for what this code does and does not guarantee.
