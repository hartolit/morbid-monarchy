#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// ===========================================================================
// Bindings & Uniforms
// ===========================================================================

struct WorldWindow {
    origin: vec2<f32>,
    size: vec2<f32>,
    head: vec2<f32>,
    elevation_scale: f32,
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

// ===========================================================================
// Geometric Layout Constants
// ===========================================================================

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 66u;

// ===========================================================================
// Helper Functions
// ===========================================================================

fn calculate_heights_at(
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32
) -> vec2<f32> {
    if cell_x < 0 || cell_x >= grid_width || cell_y < 0 || cell_y >= grid_height {
        return vec2<f32>(0.0, 0.0);
    }

    let wrapped_x = ((cell_x + i32(window.head.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head.y)) % grid_height + grid_height) % grid_height;

    // Multiply by 2 because each cell is exactly two u32 words (64 bits)
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    let elevation = f32((word_0 >> 15u) & 0x1FFFFu);
    let fluid_vol = f32(word_1 & 0x3FFu);

    let terrain_height = elevation * elevation_scale;
    let total_visual_height = terrain_height + (fluid_vol * elevation_scale);

    return vec2<f32>(terrain_height, total_visual_height);
}

// ===========================================================================
// I/O Structs
// ===========================================================================

struct VertexInput {
    @location(0) _position: vec3<f32>,
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

    let cell_index = in.vertex_index / VERTS_PER_CELL;
    let index_within_cell = in.vertex_index % VERTS_PER_CELL;
    let face_slot = index_within_cell / VERTS_PER_FACE;
    let vertex_within_face = index_within_cell % VERTS_PER_FACE;

    let cell_x = i32(cell_index) % grid_width;
    let cell_y = i32(cell_index) / grid_width;

    let wrapped_x = ((cell_x + i32(window.head.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head.y)) % grid_height + grid_height) % grid_height;

    // Boundary-Aligned Array Lookup (2 u32s per cell)
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    // Decode Word 0 (Geometry)
    let material_terrain = word_0 & 0x1Fu;
    let material_fluid   = (word_0 >> 5u) & 0x1Fu;
    let material_surface = (word_0 >> 10u) & 0x1Fu;
    let elevation        = f32((word_0 >> 15u) & 0x1FFFFu);

    // Decode Word 1 (Physics & State)
    let fluid_vol      = f32(word_1 & 0x3FFu);
    let active_variant = (word_1 >> 18u) & 0x3Fu;

    let terrain_height = elevation * elevation_scale;
    let fluid_depth = fluid_vol * elevation_scale;

    let world_offset_x = f32(cell_x);
    let world_offset_z = f32(grid_height - 1 - cell_y);

    var local_position = vec3<f32>(0.0, -9999.0, 0.0);
    var face_normal = vec3<f32>(0.0, 1.0, 0.0);
    var active_mat_lookup = material_terrain;

    switch face_slot {
        case 0u: { // Terrain Top Cap
            active_mat_lookup = material_terrain;
            face_normal = vec3<f32>(0.0, 1.0, 0.0);

            if material_terrain != 0u && material_terrain != 31u && terrain_height > 0.0 {
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
        case 1u: { // Terrain Front Wall (-Z)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y + 1, grid_width, grid_height, elevation_scale);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 31u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, -1.0);
                active_mat_lookup = material_terrain;
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
        case 2u: { // Terrain Back Wall (+Z)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y - 1, grid_width, grid_height, elevation_scale);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 31u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, 1.0);
                active_mat_lookup = material_terrain;
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
        case 3u: { // Terrain Right Wall (+X)
            let neighbor_heights = calculate_heights_at(cell_x + 1, cell_y, grid_width, grid_height, elevation_scale);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 31u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(1.0, 0.0, 0.0);
                active_mat_lookup = material_terrain;
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
        case 4u: { // Terrain Left Wall (-X)
            let neighbor_heights = calculate_heights_at(cell_x - 1, cell_y, grid_width, grid_height, elevation_scale);
            let height_lower_bound = neighbor_heights.x;

            if material_terrain != 0u && material_terrain != 31u && terrain_height > height_lower_bound {
                face_normal = vec3<f32>(-1.0, 0.0, 0.0);
                active_mat_lookup = material_terrain;
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
        case 5u: { // Fluid Top Cap
            active_mat_lookup = material_fluid + 32u;
            face_normal = vec3<f32>(0.0, 1.0, 0.0);

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
        case 6u: { // Surface Top Cap
            active_mat_lookup = material_surface + 64u;
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
        case 7u: { // Fluid Front Wall (-Z)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y + 1, grid_width, grid_height, elevation_scale);
            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, -1.0);
                active_mat_lookup = material_fluid + 32u;
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
        case 8u: { // Fluid Back Wall (+Z)
            let neighbor_heights = calculate_heights_at(cell_x, cell_y - 1, grid_width, grid_height, elevation_scale);
            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(0.0, 0.0, 1.0);
                active_mat_lookup = material_fluid + 32u;
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
        case 9u: { // Fluid Right Wall (+X)
            let neighbor_heights = calculate_heights_at(cell_x + 1, cell_y, grid_width, grid_height, elevation_scale);
            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(1.0, 0.0, 0.0);
                active_mat_lookup = material_fluid + 32u;
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
        default: { // Slot 10 - Fluid Left Wall (-X)
            let neighbor_heights = calculate_heights_at(cell_x - 1, cell_y, grid_width, grid_height, elevation_scale);
            let height_lower_bound = max(neighbor_heights.y, terrain_height);
            let height_upper_bound = terrain_height + fluid_depth;

            if material_fluid != 0u && height_upper_bound > height_lower_bound {
                face_normal = vec3<f32>(-1.0, 0.0, 0.0);
                active_mat_lookup = material_fluid + 32u;
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

    var base_color = palette[active_mat_lookup];
    let visual_shift = (f32(active_variant) - 32.0) / 32.0 * 0.15;

    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    let light_direction = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let is_skirt_wall = (face_slot >= 1u && face_slot <= 4u) || (face_slot >= 7u && face_slot <= 10u);
    let ambient_light = select(0.3, 0.15, is_skirt_wall);
    let diffuse_light = max(dot(out.normal, light_direction), ambient_light);

    out.color = vec4<f32>(base_color.rgb * diffuse_light, base_color.a);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
