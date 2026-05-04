# [CURRENT] vision:

# World Physics Vision: The 64-Bit Packed Universe

## Overview

This document captures the architectural and creative intent behind the world physics system in Morbid Monarchy. The engine is ruthlessly optimized for parallel Cellular Automata (CA) execution, treating memory bandwidth and CPU cache-line efficiency as the supreme constraints. 

---

## The Core Idea: The 8-Byte Cache-Line Miracle

Cellular Automata is bottlenecked by RAM fetches, not ALU math. To maximize simulation speed, the entire universe is compressed into an aggressively bit-packed 64-bit integer (`u64`) for every `WorldCell`. 

At exactly **8 bytes per cell**, a standard modern CPU fetches **8 cells per 64-byte L1 cache line**. The CPU Rayon threads iterate across a flat 2D grid of these `u64` integers, manipulating them with zero-cost bitwise shifts.

The CPU never sees a 3D world. It sees a 2D topographical map. The WGSL GPU shader extrapolates the 3D heights entirely on-the-fly from the bit-packed integers.

### The Boundary-Aligned Layout

Because WGSL reads storage buffers as 32-bit arrays (`array<u32>`), our 64 bits are strictly designed so that no variable crosses a 32-bit boundary. This prevents ALU-stalling bit-straddle extractions on the GPU.

**Word 0 (Bits 0-31): The Geometry Word**
Read by the GPU to construct the physical vertices of the terrain slab, fluid slab, and surface slab.
- **Bits 0-4 (5 bits):** `terrain_mat` (Rock, Sand, Dirt, Foliage)
- **Bits 5-9 (5 bits):** `fluid_mat` (Water, Magma, Blood)
- **Bits 10-14 (5 bits):** `surface_mat` (Fire, Steam, Smoke)
- **Bits 15-31 (17 bits):** `elevation` (Absolute base terrain height, 0 to 131,071)

**Word 1 (Bits 32-63): The Physics/State Word**
Read by the CPU and GPU to simulate movement, depth, and biology.
- **Bits 32-41 (10 bits):** `fluid_vol` (Depth of the liquid slab, max 1,023)
- **Bits 42-49 (8 bits):** `terrain_state` (Plant growth cycles, biological decay, free variable)
- **Bits 50-55 (6 bits):** `variants` (Visual noise for the shader palette)
- **Bits 56-59 (4 bits):** `momentum` (Hydraulic flow vectors: N, S, E, W)
- **Bits 60-63 (4 bits):** `awake` (CA active-cell tracking flags)

---

## The Physics Rules

### 1. Gravity and Elevation
The world's verticality is absolute. `elevation` defines the solid floor of the cell. 
Fluids stack on top of the terrain. The total visual height of a cell is always `elevation + fluid_vol`. 
- **Liquid Flow:** Water evaluates its 8 neighbors and flows toward the lowest total elevation gradient (`neighbor.elevation + neighbor.fluid_vol`).

### 2. Biology is Decoupled from Shape
The `terrain_state` bits are strictly reserved for biological lifecycles (e.g., how old a flower is, how decayed a corpse is). It does not affect the physical shape of the cell.

### 3. Falling Sand
Because the GPU topologies are rigidly ordered (`Terrain -> Fluid -> Surface`), loose sand cannot graphically exist in the terrain slot while simultaneously sinking "through" a liquid.
Instead, gravity in a rigid CA system is handled via **Material Swapping**. If a cell has `terrain_mat == SAND` and the cell below it has `fluid_mat == WATER`, the cellular automata rules evaluate the density and physically swap their material IDs and volumes in a single lock-free tick.

---

## Why This Is Not "Slop"

The bit-packing uses manual `const` shifts and masks in `crates/monarch-engine/src/engine/world/cell.rs`.
We do **not** use macro crates like `modular-bitfield`. Macros arbitrarily reorder bits and rely on host endianness, which would cause immediate corruption when the GPU shader reads the raw memory buffer. 

By writing explicit deterministic bit-math, we guarantee the layout is identical on ARM CPUs, x86 CPUs, disk saves (via `bitcode`), and the GPU shader. The raw `u64` is encapsulated behind safe getter/setter methods so the simulation logic (`liquid_sim.rs`, `biology_sim.rs`) never touches a bitwise operator.

---

## Entities Live Outside the Grid

Entities (creatures, players, projectiles) are **not** stored in `WorldCell`. They live in a separate Bevy ECS layer and have their own position, state, and physics. The grid is purely the environmental substrate. This keeps `WorldCell` strictly at 8 bytes and the CA simulation completely deterministic.




---
# [OLD OBSOLETE] vision:

# World Physics Vision: The Algebraic Membrane Universe

## Overview

This document captures the architectural and creative intent behind the world physics system in Morbid Monarchy. It exists so future engineers and agents understand **why** the world is designed this way, not just **how** it works. Do not "simplify" or "fix" these mechanics — they are intentional design decisions, not bugs.

---

## The Core Idea: Geology Subjugated by Atmospheric Pressure

The world does **not** store terrain elevation as a variable. Instead, elevation is an **algebraic output** derived from two cell-level state bytes at render time. This means:

- `WorldCell` remains exactly **16 bytes** — zero memory bloat.
- The CPU cellular automata engine operates on a **flat 2D grid** of `WorldCell` structs.
- The GPU shader **derives the 3D heightmap** from those cells on-the-fly, every frame.

The equation is:

```
Z_terrain = H_MAX - (atmosphere.state × elevation_scale) - (fluid.state × elevation_scale)
Z_fluid_bottom = Z_terrain
Z_fluid_top    = Z_terrain + (fluid.state × elevation_scale)
Z_surface      = Z_terrain + (fluid.state × elevation_scale)
```

Where `H_MAX` is a fixed constant (currently `50.0` world units, driven by `h_max` in `WorldWindowUniform`), and `elevation_scale` controls the visual steepness per unit of pressure/mass.

---

## The WorldCell Layer Model

Each `WorldCell` holds four `Pixel` layers. Only two of them contribute to vertical height:

| Layer       | Height Contribution | Notes                                                                 |
|-------------|---------------------|-----------------------------------------------------------------------|
| `terrain`   | None (stack of 1)   | Solid rock, sand, dirt. Rendered as a solid block from `y=0` to `Z_terrain`. |
| `fluid`     | `fluid.state`       | Water, blood, magma. Stacks on top of terrain. Depth equals `fluid.state`. |
| `atmosphere`| `atmosphere.state`  | The invisible weight. High pressure = deep crater. Low = tall mountain.  |
| `surface`   | None (stack of 1)   | Boats, foliage, items. Floats at `Z_terrain + fluid_depth`.           |

The `terrain.state` byte is **intentionally free** for biology (plant growth cycles, decay stages, etc.) because height is NOT stored there.

---

## Why This Is Not a Bug

### "The Vacuum Spell"

If all atmosphere is removed from a cell (`atmosphere.state = 0`), the terrain rises to maximum height. This is **intended gameplay**. It means:

- Explosions inject high-atmosphere-pressure mass into the blast radius, creating craters.
- Magic spells can manipulate terrain elevation by altering atmospheric state.
- Earthquakes can be simulated by rippling atmosphere pressure changes across cells.

### "The Pressurized Bottle"

The metaphor: when you pour water out of a bottle in the real world, air fills the void to maintain the bottle's shape. Here, if you move fluid out of a cell without equalizing with atmosphere, the terrain "grows inward" (rises). This is a deliberate, unique law of physics baked into the universe — not a simulation flaw.

### Water Flow Direction

On the CPU, water flows toward **higher atmospheric pressure**. A cell with `atmosphere.state = 50` is a deep crater; a cell with `atmosphere.state = 10` is a mountain peak. Water flowing downhill is implemented as: move fluid toward the neighbor with the greatest atmospheric mass. This is a cheap integer comparison.

---

## What This Achieves

1. **Zero extra memory**: No elevation field is added to `WorldCell`. The 16-byte alignment is sacred.
2. **CPU physics remain simple**: The CA engine does integer comparisons on `u8` states. No floating-point terrain math on the CPU.
3. **GPU constructs the 3D world**: The WGSL shader in `assets/shaders/world.wgsl` reads `atmosphere.state` and `fluid.state` per cell and computes all Y-axis positions algebraically. The CPU never knows the world is 3D.
4. **Explosion craters are free**: Increase `atmosphere.state` in the blast radius. The crater appears immediately at the next render tick without mutating a single terrain byte.
5. **Dynamic fluid at any elevation**: Fluid can sit at mountain tops because altitude is derived from atmosphere, not hardcoded as a sub-level.
6. **Biology is decoupled from geometry**: `terrain.state` tracks plant growth, decay, and other biological lifecycle stages without affecting the visual shape of the land.

---

## Implementation Locations

| Concern                        | File                                                                 |
|--------------------------------|----------------------------------------------------------------------|
| Heightmap shader equation      | `crates/morbid-app/assets/shaders/world.wgsl`                        |
| Per-cell data layout           | `crates/monarch-engine/src/engine/world/cell.rs`                    |
| World generation (noise → atmos pressure) | `crates/monarch-engine/src/engine/generation/world_gen.rs` |
| Render mesh + GPU buffer sync  | `crates/morbid-app/src/runtime/render.rs`                           |
| Active grid / toroidal buffer  | `crates/monarch-engine/src/engine/world/grid.rs`                    |

---

## World Generation Conventions

The world generator (`WorldGenerator`) maps a noise value to `atmosphere.state`:

- **High noise (mountain)** → **low gas pressure** (atmosphere ≈ 0) → terrain rises to `H_MAX`.
- **Low noise (valley/ocean)** → **high gas pressure** (atmosphere ≈ 255) → terrain is crushed down.
- Fluid (`LIQUID_WATER`) is placed in cells where `gas_pressure > 160`, with depth equal to `gas_pressure - 160`.
- Coastlines (`gas_pressure 140–160`) are `LOOSE_SAND` with no fluid.
- Highlands (`gas_pressure < 140`) are `ORGANIC_FOLIAGE` with biology tracking via `terrain.state`.

---

## Toroidal Grid & Rendering Architecture

The `ActiveWorldGrid` is a **toroidal (wrapping) buffer**. When the active window shifts, only `buffer_head` advances — the underlying `Vec<WorldCell>` does not move. The GPU shader compensates by applying toroidal index arithmetic using `buffer_head` and `window_origin` from the `WorldWindowUniform`.

The render mesh is a static geometry of `width × height` unit cubes (one per cell), each with 3 layer-slabs (terrain, fluid, surface). The vertex shader deforms them at runtime. The mesh is only rebuilt when grid dimensions change (e.g., on simulation resize), not on every frame or chunk load.

---

## Entities Live Outside the Grid

Entities (creatures, players, projectiles) are **not** stored in `WorldCell`. They live in a separate ECS layer and have their own position, state, and physics. The grid is purely the environmental substrate. This keeps `WorldCell` lean and the CA simulation deterministic.

---

## Future Considerations

- **Atmosphere simulation**: Currently atmosphere state is static after world generation. A future system could diffuse atmosphere pressure across neighbors each tick to simulate wind, pressure equalization, or gas expansion. This should be opt-in and GPU-accelerated where possible.
- **Explosion system**: An event-driven system that injects atmosphere mass into a radius of cells. The crater emerges from the renderer automatically.
- **Fluid physics**: Water flows toward higher `atmosphere.state` neighbors. Implement as a standard CA pass, checking the 4 or 8 cardinal neighbors.
- **Z-chunk stacking**: `ChunkKey` uses `IVec3`. If vertical multi-floor worlds are needed, expand the active simulation window to hold multiple Z-slices and handle vertical CA handoffs between them.
- **Tuning resource**: `h_max` and `elevation_scale` are currently set inline in `sync_grid_rendering`. They should be moved to a typed `TomlConfig` resource for runtime tuning without recompiling.
