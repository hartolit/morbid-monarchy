#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// ---------------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------------

struct WorldWindow {
    /// Bottom-left world coordinate of the active window (in cells).
    origin: vec2<f32>,
    /// Width and height of the active window (in cells).
    size: vec2<f32>,
    /// Toroidal buffer-head offset (in cells).
    head: vec2<f32>,
    /// Maximum terrain height in world units (when atmosphere.state == 0).
    h_max: f32,
    /// World units of height contributed per unit of atmosphere / fluid state.
    elevation_scale: f32,
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

// ---------------------------------------------------------------------------
// Vertex layout
// ---------------------------------------------------------------------------
//
// Draw topology (non-indexed TriangleList):
//
//   vertex_count = width × height × FACE_SLOTS × VERTS_PER_FACE
//                = width × height × 7           × 6
//                = width × height × 42
//
// Face-slot meanings:
//   0  = Terrain top cap   (XZ quad at y = my_terrain_height,   normal +Y)
//   1  = Front skirt  (-Z) bridges this cell and its -Z neighbour
//   2  = Back  skirt  (+Z) bridges this cell and its +Z neighbour
//   3  = Right skirt  (+X) bridges this cell and its +X neighbour
//   4  = Left  skirt  (-X) bridges this cell and its -X neighbour
//   5  = Fluid top cap     (XZ quad at y = terrain + fluid_depth, normal +Y)
//   6  = Surface top cap   (XZ quad at y = terrain + fluid_depth + 1, normal +Y)
//
// Skirt (bridging) rule — for each of the 4 cardinal sides:
//   neighbour_height = terrain height of the adjacent cell (toroidal lookup)
//   y_lo = min(my_height, neighbour_height)
//   y_hi = max(my_height, neighbour_height)
//   The skirt quad spans y_lo → y_hi and is owned by whichever cell is TALLER.
//   If my_height <= neighbour_height  → I am not taller → degenerate (hidden).
//   If my_height >  neighbour_height  → I own this cliff face → draw it.
//   When heights are equal → zero-height quad → zero area → culled by rasteriser.
//
// This guarantees every cliff face is drawn exactly once by the taller cell,
// with that cell's terrain material, and there are never any gaps or z-fights.
//
// Decomposition:
//   cell_index = vertex_index / VERTS_PER_CELL
//   face_slot  = (vertex_index % VERTS_PER_CELL) / VERTS_PER_FACE
//   vert       = vertex_index % VERTS_PER_FACE

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 42u;   // 7 slots × 6 verts

// ---------------------------------------------------------------------------
// Helper: toroidal buffer index for an arbitrary logical grid cell (cx, cy).
// Clamps cx/cy to the grid — edge cells treat out-of-bounds as height 0.
// ---------------------------------------------------------------------------
fn terrain_height_at(cx: i32, cy: i32, grid_w: i32, grid_h: i32, scale: f32, h_max: f32) -> f32 {
    // Edge cells: no neighbour exists outside the active window → treat as h=0
    // so the border skirts always draw (the edge cliff is always visible).
    if cx < 0 || cx >= grid_w || cy < 0 || cy >= grid_h {
        return 0.0;
    }
    let wx = ((cx + i32(window.head.x)) % grid_w + grid_w) % grid_w;
    let wy = ((cy + i32(window.head.y)) % grid_h + grid_h) % grid_h;
    let idx = u32(wy * grid_w + wx);
    let atmos_state = f32((world_buffer[idx * 4u + 2u] >> 8u) & 0xFFu);
    let fluid_state = f32((world_buffer[idx * 4u + 1u] >> 8u) & 0xFFu);
    return max(0.0, h_max - atmos_state * scale - fluid_state * scale);
}

// ---------------------------------------------------------------------------
// I/O structs
// ---------------------------------------------------------------------------

struct VertexInput {
    @location(0) _position: vec3<f32>,          // dummy — never read
    @builtin(vertex_index) vertex_index: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
};

// ---------------------------------------------------------------------------
// Vertex shader
// ---------------------------------------------------------------------------

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let grid_w = i32(window.size.x);
    let grid_h = i32(window.size.y);
    let scale = window.elevation_scale;

    // --- Decompose vertex_index ---
    let cell_index = in.vertex_index / VERTS_PER_CELL;
    let rem = in.vertex_index % VERTS_PER_CELL;
    let face_slot = rem / VERTS_PER_FACE;
    let vert = rem % VERTS_PER_FACE;

    // --- Logical cell position ---
    let cell_x = i32(cell_index) % grid_w;
    let cell_y = i32(cell_index) / grid_w;

    // --- Toroidal buffer index for THIS cell ---
    let wrap_x = ((cell_x + i32(window.head.x)) % grid_w + grid_w) % grid_w;
    let wrap_y = ((cell_y + i32(window.head.y)) % grid_h + grid_h) % grid_h;
    let buffer_idx = u32(wrap_y * grid_w + wrap_x);

    // --- Read this cell's pixels ---
    // WorldCell layout (each Pixel = u32 little-endian):
    //   +0 terrain  +1 fluid  +2 atmosphere  +3 surface
    //   bits 7:0 = material_id   bits 15:8 = state
    let cell_terrain = world_buffer[buffer_idx * 4u + 0u];
    let cell_fluid = world_buffer[buffer_idx * 4u + 1u];
    let cell_atmos = world_buffer[buffer_idx * 4u + 2u];
    let cell_surface = world_buffer[buffer_idx * 4u + 3u];

    let terrain_mat = cell_terrain & 0xFFu;
    let fluid_mat = cell_fluid & 0xFFu;
    let surface_mat = cell_surface & 0xFFu;

    let atmos_state = f32((cell_atmos >> 8u) & 0xFFu);
    let fluid_state = f32((cell_fluid >> 8u) & 0xFFu);

    // --- Algebraic Membrane Physics ---
    let my_height = max(0.0, window.h_max - atmos_state * scale - fluid_state * scale);
    let fluid_depth = fluid_state * scale;

    // --- World-space cell offset ---
    // Z is flipped: engine row 0 (cell_y=0) maps to largest Z (screen bottom).
    let offset_x = f32(cell_x);
    let offset_z = f32(grid_h - 1 - cell_y);

    // --- Geometry and visibility ---
    var local_pos: vec3<f32>;
    var normal: vec3<f32>;
    var active_pixel: u32;

    // Default: degenerate — overwritten when the face is actually visible.
    local_pos = vec3<f32>(0.0, -9999.0, 0.0);
    normal = vec3<f32>(0.0, 1.0, 0.0);
    active_pixel = cell_terrain;

    switch face_slot {

        // -------------------------------------------------------------------
        // Slot 0 — Terrain top cap
        //   XZ quad at y = my_height.  Normal +Y.
        //   Visible whenever there is real terrain material.
        // -------------------------------------------------------------------
        case 0u: {
            active_pixel = cell_terrain;
            normal = vec3<f32>(0.0, 1.0, 0.0);

            if terrain_mat != 0u && terrain_mat != 255u && my_height > 0.0 {
                var p: vec3<f32>;
                let y = my_height;
                switch vert {
                    case 0u: { p = vec3<f32>(0.0, y, 0.0); }
                    case 1u: { p = vec3<f32>(0.0, y, 1.0); }
                    case 2u: { p = vec3<f32>(1.0, y, 1.0); }
                    case 3u: { p = vec3<f32>(0.0, y, 0.0); }
                    case 4u: { p = vec3<f32>(1.0, y, 1.0); }
                    default: { p = vec3<f32>(1.0, y, 0.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 1 — Front skirt  (-Z neighbour, i.e. cell_y - 1)
        //
        // In engine space, decreasing cell_y means moving north (screen top).
        // After Z-flip: cell_y-1 renders at offset_z+1 (further from camera).
        // This cell's -Z face sits at local z=0.
        //
        // We own this cliff only when my_height > neighbour_height.
        // The quad spans from neighbour_height (bottom) to my_height (top).
        // Normal: -Z (outward from this cell's front face).
        //
        // CCW from -Z outside:
        //   tri0: (0, y_lo, 0), (0, y_hi, 0), (1, y_hi, 0)
        //   tri1: (0, y_lo, 0), (1, y_hi, 0), (1, y_lo, 0)
        // -------------------------------------------------------------------
        case 1u: {
            active_pixel = cell_terrain;
            normal = vec3<f32>(0.0, 0.0, -1.0);

            let nb_height = terrain_height_at(cell_x, cell_y + 1, grid_w, grid_h, scale, window.h_max);
            let y_lo = nb_height;
            let y_hi = my_height;

            if terrain_mat != 0u && terrain_mat != 255u && y_hi > y_lo {
                var p: vec3<f32>;
                switch vert {
                    case 0u: { p = vec3<f32>(0.0, y_lo, 0.0); }
                    case 1u: { p = vec3<f32>(0.0, y_hi, 0.0); }
                    case 2u: { p = vec3<f32>(1.0, y_hi, 0.0); }
                    case 3u: { p = vec3<f32>(0.0, y_lo, 0.0); }
                    case 4u: { p = vec3<f32>(1.0, y_hi, 0.0); }
                    default: { p = vec3<f32>(1.0, y_lo, 0.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 2 — Back skirt  (+Z neighbour, i.e. cell_y + 1)
        //
        // This cell's +Z face sits at local z=1.
        // We own this cliff only when my_height > neighbour_height.
        // Normal: +Z.
        //
        // CCW from +Z outside (reverse X):
        //   tri0: (1, y_lo, 1), (1, y_hi, 1), (0, y_hi, 1)
        //   tri1: (1, y_lo, 1), (0, y_hi, 1), (0, y_lo, 1)
        // -------------------------------------------------------------------
        case 2u: {
            active_pixel = cell_terrain;
            normal = vec3<f32>(0.0, 0.0, 1.0);

            let nb_height = terrain_height_at(cell_x, cell_y - 1, grid_w, grid_h, scale, window.h_max);
            let y_lo = nb_height;
            let y_hi = my_height;

            if terrain_mat != 0u && terrain_mat != 255u && y_hi > y_lo {
                var p: vec3<f32>;
                switch vert {
                    case 0u: { p = vec3<f32>(1.0, y_lo, 1.0); }
                    case 1u: { p = vec3<f32>(1.0, y_hi, 1.0); }
                    case 2u: { p = vec3<f32>(0.0, y_hi, 1.0); }
                    case 3u: { p = vec3<f32>(1.0, y_lo, 1.0); }
                    case 4u: { p = vec3<f32>(0.0, y_hi, 1.0); }
                    default: { p = vec3<f32>(0.0, y_lo, 1.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 3 — Right skirt  (+X neighbour, i.e. cell_x + 1)
        //
        // This cell's +X face sits at local x=1.
        // We own this cliff only when my_height > neighbour_height.
        // Normal: +X.
        //
        // CCW from +X outside:
        //   tri0: (1, y_lo, 0), (1, y_hi, 0), (1, y_hi, 1)
        //   tri1: (1, y_lo, 0), (1, y_hi, 1), (1, y_lo, 1)
        // -------------------------------------------------------------------
        case 3u: {
            active_pixel = cell_terrain;
            normal = vec3<f32>(1.0, 0.0, 0.0);

            let nb_height = terrain_height_at(cell_x + 1, cell_y, grid_w, grid_h, scale, window.h_max);
            let y_lo = nb_height;
            let y_hi = my_height;

            if terrain_mat != 0u && terrain_mat != 255u && y_hi > y_lo {
                var p: vec3<f32>;
                switch vert {
                    case 0u: { p = vec3<f32>(1.0, y_lo, 0.0); }
                    case 1u: { p = vec3<f32>(1.0, y_hi, 0.0); }
                    case 2u: { p = vec3<f32>(1.0, y_hi, 1.0); }
                    case 3u: { p = vec3<f32>(1.0, y_lo, 0.0); }
                    case 4u: { p = vec3<f32>(1.0, y_hi, 1.0); }
                    default: { p = vec3<f32>(1.0, y_lo, 1.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 4 — Left skirt  (-X neighbour, i.e. cell_x - 1)
        //
        // This cell's -X face sits at local x=0.
        // We own this cliff only when my_height > neighbour_height.
        // Normal: -X.
        //
        // CCW from -X outside (reverse Z):
        //   tri0: (0, y_lo, 1), (0, y_hi, 1), (0, y_hi, 0)
        //   tri1: (0, y_lo, 1), (0, y_hi, 0), (0, y_lo, 0)
        // -------------------------------------------------------------------
        case 4u: {
            active_pixel = cell_terrain;
            normal = vec3<f32>(-1.0, 0.0, 0.0);

            let nb_height = terrain_height_at(cell_x - 1, cell_y, grid_w, grid_h, scale, window.h_max);
            let y_lo = nb_height;
            let y_hi = my_height;

            if terrain_mat != 0u && terrain_mat != 255u && y_hi > y_lo {
                var p: vec3<f32>;
                switch vert {
                    case 0u: { p = vec3<f32>(0.0, y_lo, 1.0); }
                    case 1u: { p = vec3<f32>(0.0, y_hi, 1.0); }
                    case 2u: { p = vec3<f32>(0.0, y_hi, 0.0); }
                    case 3u: { p = vec3<f32>(0.0, y_lo, 1.0); }
                    case 4u: { p = vec3<f32>(0.0, y_hi, 0.0); }
                    default: { p = vec3<f32>(0.0, y_lo, 0.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 5 — Fluid top cap
        //   XZ quad at y = my_height + fluid_depth.  Normal +Y.
        //   Visible when the fluid layer has a real material and non-zero depth.
        // -------------------------------------------------------------------
        case 5u: {
            active_pixel = cell_fluid;
            normal = vec3<f32>(0.0, 1.0, 0.0);

            if fluid_mat != 0u && fluid_depth > 0.0 {
                let y = my_height + fluid_depth;
                var p: vec3<f32>;
                switch vert {
                    case 0u: { p = vec3<f32>(0.0, y, 0.0); }
                    case 1u: { p = vec3<f32>(0.0, y, 1.0); }
                    case 2u: { p = vec3<f32>(1.0, y, 1.0); }
                    case 3u: { p = vec3<f32>(0.0, y, 0.0); }
                    case 4u: { p = vec3<f32>(1.0, y, 1.0); }
                    default: { p = vec3<f32>(1.0, y, 0.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 6 — Surface top cap
        //   XZ quad at y = my_height + fluid_depth + 1.  Normal +Y.
        //   Visible when the surface layer has a real material.
        // -------------------------------------------------------------------
        default: {
            active_pixel = cell_surface;
            normal = vec3<f32>(0.0, 1.0, 0.0);

            if surface_mat != 0u {
                let y = my_height + fluid_depth + 1.0;
                var p: vec3<f32>;
                switch vert {
                    case 0u: { p = vec3<f32>(0.0, y, 0.0); }
                    case 1u: { p = vec3<f32>(0.0, y, 1.0); }
                    case 2u: { p = vec3<f32>(1.0, y, 1.0); }
                    case 3u: { p = vec3<f32>(0.0, y, 0.0); }
                    case 4u: { p = vec3<f32>(1.0, y, 1.0); }
                    default: { p = vec3<f32>(1.0, y, 0.0); }
                }
                local_pos = p + vec3<f32>(offset_x, 0.0, offset_z);
            }
        }
    }

    out.normal = normal;

    out.clip_position = mesh_position_local_to_clip(
        get_world_from_local(0u),
        vec4<f32>(local_pos, 1.0),
    );

    // --- Colour ---
    let active_mat = active_pixel & 0xFFu;
    let active_variant = (active_pixel >> 16u) & 0xFFu;

    var color = palette[active_mat];
    let visual_shift = (f32(active_variant) - 128.0) / 128.0 * 0.15;
    color.r = saturate(color.r + visual_shift);
    color.g = saturate(color.g + visual_shift);
    color.b = saturate(color.b + visual_shift);

    // Lambertian diffuse in vertex shader.
    // Side skirts receive a reduced ambient (0.15) so cliff faces appear
    // distinctly darker than the lit top cap, reinforcing the height difference.
    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let is_skirt = face_slot >= 1u && face_slot <= 4u;
    let ambient = select(0.3, 0.15, is_skirt);
    let diffuse = max(dot(out.normal, light_dir), ambient);
    out.color = vec4<f32>(color.rgb * diffuse, color.a);

    return out;
}

// ---------------------------------------------------------------------------
// Fragment shader
// ---------------------------------------------------------------------------

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
