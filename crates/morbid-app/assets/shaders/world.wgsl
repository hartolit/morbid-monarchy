#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// ===========================================================================
// Bindings & Uniforms
// ===========================================================================

struct WorldWindow {
    /// Bottom-left world coordinate of the active simulation window.
    origin: vec2<f32>,
    /// Width and height of the active window in cells.
    size: vec2<f32>,
    /// Toroidal buffer head offset, used to wrap coordinates without moving memory.
    head: vec2<f32>,
    /// Maximum terrain height in world units (achieved when atmosphere is 0).
    h_max: f32,
    /// World units of vertical height contributed per unit of atmosphere/fluid.
    elevation_scale: f32,
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

// ===========================================================================
// Geometric Layout Constants
// ===========================================================================
//
// To enforce strict domain boundaries between solid terrain and translucent
// liquids, we use 11 distinct quad faces (slots) per cell.
//
//   0  = Terrain top cap      (XZ quad at y = terrain_height, normal +Y)
//   1  = Terrain Front skirt  (-Z neighbor bridging)
//   2  = Terrain Back  skirt  (+Z neighbor bridging)
//   3  = Terrain Right skirt  (+X neighbor bridging)
//   4  = Terrain Left  skirt  (-X neighbor bridging)
//   5  = Fluid top cap        (XZ quad at y = visual_height, normal +Y)
//   6  = Surface top cap      (XZ quad at y = visual_height + 1, normal +Y)
//   7  = Fluid Front skirt    (-Z fluid bridging)
//   8  = Fluid Back skirt     (+Z fluid bridging)
//   9  = Fluid Right skirt    (+X fluid bridging)
//  10  = Fluid Left skirt     (-X fluid bridging)

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 66u;   // 11 slots × 6 verts

// ===========================================================================
// Helper Functions
// ===========================================================================

/// Calculates the physical height boundaries for a given cell coordinate.
/// Returns a vec2 where:
///   x = terrain_height (the solid ground floor)
///   y = total_visual_height (terrain + fluid depth)
fn calculate_heights_at(
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    max_height: f32
) -> vec2<f32> {
    // Return zero height for out-of-bounds coordinates to ensure grid edges
    // always draw a boundary wall down to y=0.
    if cell_x < 0 || cell_x >= grid_width || cell_y < 0 || cell_y >= grid_height {
        return vec2<f32>(0.0, 0.0);
    }

    // Apply Toroidal (wrapping) offset to map logical cell coordinates to
    // the physical 1D storage buffer index.
    let wrapped_x = ((cell_x + i32(window.head.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x);

    // Each cell is 4 u32s (16 bytes).
    // Offset +1 is Fluid, Offset +2 is Atmosphere.
    let cell_fluid = world_buffer[buffer_index * 4u + 1u];
    let cell_atmosphere = world_buffer[buffer_index * 4u + 2u];

    // Extract the state byte (bits 8-15) which determines mass/pressure.
    let fluid_state = f32((cell_fluid >> 8u) & 0xFFu);
    let atmosphere_state = f32((cell_atmosphere >> 8u) & 0xFFu);

    // Algebraic Membrane Physics:
    // Terrain height is inversely proportional to atmospheric pressure.
    let terrain_height = max(0.0, max_height - atmosphere_state * elevation_scale - fluid_state * elevation_scale);
    let total_visual_height = terrain_height + (fluid_state * elevation_scale);

    return vec2<f32>(terrain_height, total_visual_height);
}

// ===========================================================================
// I/O Structs
// ===========================================================================

struct VertexInput {
    @location(0) _position: vec3<f32>, // Dummy attribute to satisfy pipeline layout
    @builtin(vertex_index) vertex_index: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
};

// ===========================================================================
// Vertex Shader
// ===========================================================================

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.size.x);
    let grid_height = i32(window.size.y);
    let elevation_scale = window.elevation_scale;

    // 1. Decompose the linear vertex_index into semantic topology coordinates
    let cell_index = in.vertex_index / VERTS_PER_CELL;
    let index_within_cell = in.vertex_index % VERTS_PER_CELL;
    let face_slot = index_within_cell / VERTS_PER_FACE;
    let vertex_within_face = index_within_cell % VERTS_PER_FACE;

    // 2. Map cell index to 2D logical grid coordinates
    let cell_x = i32(cell_index) % grid_width;
    let cell_y = i32(cell_index) / grid_width;

    // 3. Resolve the Toroidal buffer index for this specific cell
    let wrapped_x = ((cell_x + i32(window.head.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x);

    // 4. Fetch the 4 layers (Pixels) for this cell
    let pixel_terrain = world_buffer[buffer_index * 4u + 0u];
    let pixel_fluid = world_buffer[buffer_index * 4u + 1u];
    let pixel_atmosphere = world_buffer[buffer_index * 4u + 2u];
    let pixel_surface = world_buffer[buffer_index * 4u + 3u];

    // Extract material IDs (bottom 8 bits)
    let material_terrain = pixel_terrain & 0xFFu;
    let material_fluid = pixel_fluid & 0xFFu;
    let material_surface = pixel_surface & 0xFFu;

    // Extract simulation states
    let atmosphere_state = f32((pixel_atmosphere >> 8u) & 0xFFu);
    let fluid_state = f32((pixel_fluid >> 8u) & 0xFFu);

    // 5. Calculate local heights for this cell
    let terrain_height = max(0.0, window.h_max - atmosphere_state * elevation_scale - fluid_state * elevation_scale);
    let fluid_depth = fluid_state * elevation_scale;

    // 6. Calculate World-Space horizontal offsets
    // Note: The Z axis is flipped so Engine Row 0 renders at the bottom of the screen
    let world_offset_x = f32(cell_x);
    let world_offset_z = f32(grid_height - 1 - cell_y);

    // --- Geometry Generation Defaults ---
    // If a face isn't needed, we degenerate the quad by collapsing it to y = -9999.0
    var local_position = vec3<f32>(0.0, -9999.0, 0.0);
    var face_normal = vec3<f32>(0.0, 1.0, 0.0);
    var active_pixel = pixel_terrain;

    // =======================================================================
    // Face Slot Dispatching
    // =======================================================================
    switch face_slot {

        // -------------------------------------------------------------------
        // Slot 0: Terrain Top Cap (Horizontal Flat Surface)
        // -------------------------------------------------------------------
        case 0u: {
            active_pixel = pixel_terrain;
            face_normal = vec3<f32>(0.0, 1.0, 0.0);

            // Only draw if there is a valid terrain material
            if material_terrain != 0u && material_terrain != 255u && terrain_height > 0.0 {
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, terrain_height, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, terrain_height, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, terrain_height, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, terrain_height, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, terrain_height, 1.0); }
                    default: { vertex_position = vec3<f32>(1.0, terrain_height, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slots 1-4: Terrain Skirts (Vertical Cliffs)
        // Spans from neighbor's terrain height up to our terrain height.
        // -------------------------------------------------------------------
        case 1u: { // Terrain Front Wall (-Z neighbor)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y + 1, grid_width, grid_height, elevation_scale, window.h_max);
            let height_lower_bound = neighbor_heights.x;

            // Draw cliff only if we are taller than our neighbor's terrain floor
            if material_terrain != 0u && material_terrain != 255u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, -1.0);
                active_pixel = pixel_terrain;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, terrain_height, 0.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, terrain_height, 0.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, terrain_height, 0.0); }
                    default: { vertex_position = vec3<f32>(1.0, height_lower_bound, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
        case 2u: { // Terrain Back Wall (+Z neighbor)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y - 1, grid_width, grid_height, elevation_scale, window.h_max);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 255u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, 1.0);
                active_pixel = pixel_terrain;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 1.0); }
                    case 1u: { vertex_position = vec3<f32>(1.0, terrain_height, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(0.0, terrain_height, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 1.0); }
                    case 4u: { vertex_position = vec3<f32>(0.0, terrain_height, 1.0); }
                    default: { vertex_position = vec3<f32>(0.0, height_lower_bound, 1.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
        case 3u: { // Terrain Right Wall (+X neighbor)
            let neighbor_heights = calculate_heights_at(cell_x + 1, cell_y, grid_width, grid_height, elevation_scale, window.h_max);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 255u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(1.0, 0.0, 0.0);
                active_pixel = pixel_terrain;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(1.0, terrain_height, 0.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, terrain_height, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, terrain_height, 1.0); }
                    default: { vertex_position = vec3<f32>(1.0, height_lower_bound, 1.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
        case 4u: { // Terrain Left Wall (-X neighbor)
            let neighbor_heights = calculate_heights_at(cell_x - 1, cell_y, grid_width, grid_height, elevation_scale, window.h_max);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 255u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(-1.0, 0.0, 0.0);
                active_pixel = pixel_terrain;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 1.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, terrain_height, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(0.0, terrain_height, 0.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 1.0); }
                    case 4u: { vertex_position = vec3<f32>(0.0, terrain_height, 0.0); }
                    default: { vertex_position = vec3<f32>(0.0, height_lower_bound, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 5: Fluid Top Cap (Horizontal Flat Surface)
        // -------------------------------------------------------------------
        case 5u: {
            active_pixel = pixel_fluid;
            face_normal = vec3<f32>(0.0, 1.0, 0.0);

            // Only draw if there is actual fluid volume
            if material_fluid != 0u && fluid_depth > 0.0 {
                let total_visual_height = terrain_height + fluid_depth;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, total_visual_height, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, total_visual_height, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, total_visual_height, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, total_visual_height, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, total_visual_height, 1.0); }
                    default: { vertex_position = vec3<f32>(1.0, total_visual_height, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slot 6: Surface Top Cap (Floating Items / Vegetation)
        // -------------------------------------------------------------------
        case 6u: {
            active_pixel = pixel_surface;
            face_normal = vec3<f32>(0.0, 1.0, 0.0);

            if material_surface != 0u {
                let surface_height = terrain_height + fluid_depth + 1.0;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, surface_height, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, surface_height, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, surface_height, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, surface_height, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, surface_height, 1.0); }
                    default: { vertex_position = vec3<f32>(1.0, surface_height, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }

        // -------------------------------------------------------------------
        // Slots 7-10: Fluid Skirts (Vertical Translucent Cliffs)
        // Spans from the highest obstructing point up to our fluid top.
        // -------------------------------------------------------------------
        case 7u: { // Fluid Front Wall (-Z neighbor)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y + 1, grid_width, grid_height, elevation_scale, window.h_max);

            // The fluid skirt starts at whichever is highest: the neighbor's total visual height,
            // or our own solid terrain floor. We don't draw fluid inside rocks.
            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, -1.0);
                active_pixel = pixel_fluid;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, height_upper_bound, 0.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, height_upper_bound, 0.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, height_upper_bound, 0.0); }
                    default: { vertex_position = vec3<f32>(1.0, height_lower_bound, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
        case 8u: { // Fluid Back Wall (+Z neighbor)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y - 1, grid_width, grid_height, elevation_scale, window.h_max);

            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, 1.0);
                active_pixel = pixel_fluid;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 1.0); }
                    case 1u: { vertex_position = vec3<f32>(1.0, height_upper_bound, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(0.0, height_upper_bound, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 1.0); }
                    case 4u: { vertex_position = vec3<f32>(0.0, height_upper_bound, 1.0); }
                    default: { vertex_position = vec3<f32>(0.0, height_lower_bound, 1.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
        case 9u: { // Fluid Right Wall (+X neighbor)
            let neighbor_heights = calculate_heights_at(cell_x + 1, cell_y, grid_width, grid_height, elevation_scale, window.h_max);

            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(1.0, 0.0, 0.0);
                active_pixel = pixel_fluid;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 0.0); }
                    case 1u: { vertex_position = vec3<f32>(1.0, height_upper_bound, 0.0); }
                    case 2u: { vertex_position = vec3<f32>(1.0, height_upper_bound, 1.0); }
                    case 3u: { vertex_position = vec3<f32>(1.0, height_lower_bound, 0.0); }
                    case 4u: { vertex_position = vec3<f32>(1.0, height_upper_bound, 1.0); }
                    default: { vertex_position = vec3<f32>(1.0, height_lower_bound, 1.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
        default: { // Slot 10 - Fluid Left Wall (-X neighbor)
            let neighbor_heights = calculate_heights_at(cell_x - 1, cell_y, grid_width, grid_height, elevation_scale, window.h_max);

            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(-1.0, 0.0, 0.0);
                active_pixel = pixel_fluid;
                var vertex_position: vec3<f32>;
                switch vertex_within_face {
                    case 0u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 1.0); }
                    case 1u: { vertex_position = vec3<f32>(0.0, height_upper_bound, 1.0); }
                    case 2u: { vertex_position = vec3<f32>(0.0, height_upper_bound, 0.0); }
                    case 3u: { vertex_position = vec3<f32>(0.0, height_lower_bound, 1.0); }
                    case 4u: { vertex_position = vec3<f32>(0.0, height_upper_bound, 0.0); }
                    default: { vertex_position = vec3<f32>(0.0, height_lower_bound, 0.0); }
                }
                local_position = vertex_position + vec3<f32>(world_offset_x, 0.0, world_offset_z);
            }
        }
    }

    out.normal = face_normal;
    out.clip_position = mesh_position_local_to_clip(get_world_from_local(0u), vec4<f32>(local_position, 1.0));

    // =======================================================================
    // Color & Lighting
    // =======================================================================

    let active_material_id = active_pixel & 0xFFu;
    let active_variant = (active_pixel >> 16u) & 0xFFu;

    var base_color = palette[active_material_id];

    // Apply slight visual variation to the base color depending on the variant byte
    let visual_shift = (f32(active_variant) - 128.0) / 128.0 * 0.15;
    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    // Apply simple Lambertian directional lighting
    let light_direction = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let is_skirt_wall = (face_slot >= 1u && face_slot <= 4u) || (face_slot >= 7u && face_slot <= 10u);

    // Dim the ambient light on vertical skirts to emphasize depth
    let ambient_light = select(0.3, 0.15, is_skirt_wall);
    let diffuse_light = max(dot(out.normal, light_direction), ambient_light);

    out.color = vec4<f32>(base_color.rgb * diffuse_light, base_color.a);
    return out;
}

// ===========================================================================
// Fragment Shader
// ===========================================================================

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
