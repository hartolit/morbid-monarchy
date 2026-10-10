# monarch-engine

The CPU baseline of the physical world, packaged as Bevy plugins. It contains:

- the packed column cell (`core/world/cell.rs`) and its material rules;
- the double-buffered cellular automaton (`core/simulation*`): liquid, granular, thermal, fire and
  biology rules over the active grid, on a fixed clock with a catch-up cap;
- sphere bodies (`core/entities/spherical.rs`): integration, heightfield contact, deformation,
  buoyancy and liquid displacement, writing the CPU grid directly;
- the observer, the world generator and the systems that move the active window.

The GPU transition replaces this crate's role. Cell meanings and generation move to a
framework-free model in prompt 01. The cell and body solvers become GPU kernels in prompts 03–04,
and the CPU production path goes away in 06. Until then, the crate is the behavior reference that
the [coverage registry](../../tools/evidence/coverage.toml) pins: 28 baseline tests, each with a
behavior ID and the stage that carries it forward.

`core/counters.rs` defines `EngineCounters`. It records ticks, copied bytes, body pair tests and
solver time for baseline measurement, and nothing reads it to decide anything. See
[status](../../docs/product/status.md) for what this code does and does not guarantee.
