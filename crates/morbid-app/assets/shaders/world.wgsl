#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin: vec2<f32>,
    size: vec2<f32>,
    head: vec2<f32>,
    elevation_scale: f32,
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 96u; // 16 faces * 6 verts

// Returns absolute heights: x=Terrain, y=Granular(Total), z=Fluid(Total)
fn calculate_heights_at(
    cell_x: i32, cell_y: i32,
    grid_width: i32, grid_height: i32,
    elevation_scale: f32
) -> vec3<f32> {
    if cell_x < 0 || cell_x >= grid_width || cell_y < 0 || cell_y >= grid_height {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    let wrapped_x = ((cell_x + i32(window.head.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    // Extract raw units
    let elevation = f32((word_0 >> 20u) & 0xFFFu);
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0x1FFu);

    // Compute stacked absolute heights
    let t_height = elevation * elevation_scale;
    let g_height = t_height + (granular_vol * elevation_scale);
    let f_height = g_height + (fluid_vol * elevation_scale);

    return vec3<f32>(t_height, g_height, f_height);
}

struct VertexInput {
    @location(0) _position: vec3<f32>,
    @builtin(vertex_index) vertex_index: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.size.x);
    let grid_height = i32(window.size.y);
    let scale = window.elevation_scale;

    let cell_index = in.vertex_index / VERTS_PER_CELL;
    let index_within_cell = in.vertex_index % VERTS_PER_CELL;
    let face_slot = index_within_cell / VERTS_PER_FACE;
    let vertex_within_face = index_within_cell % VERTS_PER_FACE;

    let cell_x = i32(cell_index) % grid_width;
    let cell_y = i32(cell_index) / grid_width;

    let wrapped_x = ((cell_x + i32(window.head.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    // Decode Word 0 (Geometry)
    let mat_terrain = word_0 & 0xFu;
    let mat_surface = (word_0 >> 4u) & 0xFu;
    let mat_granular = (word_0 >> 8u) & 0x7u;
    let mat_fluid = (word_0 >> 11u) & 0xFu;
    let variants = (word_0 >> 15u) & 0x1Fu;
    let elevation = f32((word_0 >> 20u) & 0xFFFu);

    // Decode Word 1 (Physics)
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0x1FFu);

    // Absolute Stacked Heights
    let t_height = elevation * scale;
    let g_height = t_height + (granular_vol * scale);
    let f_height = g_height + (fluid_vol * scale);

    let world_offset_x = f32(cell_x);
    let world_offset_z = f32(grid_height - 1 - cell_y);

    var local_pos = vec3<f32>(0.0, -9999.0, 0.0);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
    var mat_lookup = mat_terrain; // Default to terrain offset 0

    // Offsets to neighbor cells for boundary occlusion
    var n_dx = 0;
    var n_dy = 0;

    // Setup generic wall faces
    if face_slot == 1u || face_slot == 6u || face_slot == 11u { normal = vec3<f32>(0.0, 0.0, -1.0); n_dy = 1; }
    if face_slot == 2u || face_slot == 7u || face_slot == 12u { normal = vec3<f32>(0.0, 0.0, 1.0); n_dy = -1; }
    if face_slot == 3u || face_slot == 8u || face_slot == 13u { normal = vec3<f32>(1.0, 0.0, 0.0); n_dx = 1; }
    if face_slot == 4u || face_slot == 9u || face_slot == 14u { normal = vec3<f32>(-1.0, 0.0, 0.0); n_dx = -1; }

    let neighbor_h = calculate_heights_at(cell_x + n_dx, cell_y + n_dy, grid_width, grid_height, scale);

    switch face_slot {
        // --- TERRAIN FACES (0-4) ---
        case 0u: { // Cap
            mat_lookup = mat_terrain;
            if mat_terrain != 0u && t_height > 0.0 {
                local_pos = get_quad_vertex(vertex_within_face, t_height, t_height, normal);
            }
        }
        case 1u, 2u, 3u, 4u: { // Walls
            mat_lookup = mat_terrain;
            if mat_terrain != 0u && t_height > neighbor_h.x {
                local_pos = get_quad_vertex(vertex_within_face, neighbor_h.x, t_height, normal);
            }
        }

        // --- GRANULAR FACES (5-9) ---
        case 5u: { // Cap
            mat_lookup = mat_granular + 32u;
            if mat_granular != 0u && granular_vol > 0.0 {
                local_pos = get_quad_vertex(vertex_within_face, g_height, g_height, vec3<f32>(0.0, 1.0, 0.0));
            }
        }
        case 6u, 7u, 8u, 9u: { // Walls
            mat_lookup = mat_granular + 32u;
            let n_floor = max(neighbor_h.y, t_height);
            if mat_granular != 0u && g_height > n_floor {
                local_pos = get_quad_vertex(vertex_within_face, n_floor, g_height, normal);
            }
        }

        // --- FLUID FACES (10-14) ---
        case 10u: { // Cap
            mat_lookup = mat_fluid + 64u;
            if mat_fluid != 0u && fluid_vol > 0.0 {
                local_pos = get_quad_vertex(vertex_within_face, f_height, f_height, vec3<f32>(0.0, 1.0, 0.0));
            }
        }
        case 11u, 12u, 13u, 14u: { // Walls
            mat_lookup = mat_fluid + 64u;
            let n_floor = max(neighbor_h.z, g_height);
            if mat_fluid != 0u && f_height > n_floor {
                local_pos = get_quad_vertex(vertex_within_face, n_floor, f_height, normal);
            }
        }

        // --- SURFACE FACES (15) ---
        default: { // Slot 15 - Cap
            mat_lookup = mat_surface + 96u;
            if mat_surface != 0u {
                let s_height = f_height + 1.0;
                local_pos = get_quad_vertex(vertex_within_face, s_height, s_height, vec3<f32>(0.0, 1.0, 0.0));
            }
        }
    }

    local_pos = local_pos + vec3<f32>(world_offset_x, 0.0, world_offset_z);

    out.normal = normal;
    out.clip_position = mesh_position_local_to_clip(get_world_from_local(0u), vec4<f32>(local_pos, 1.0));

    var base_color = palette[mat_lookup];
    let visual_shift = (f32(variants) - 16.0) / 16.0 * 0.10; // Subtler noise shift

    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let ambient = select(0.3, 0.15, normal.y < 0.5); // Walls get darker ambient
    let diffuse = max(dot(normal, light_dir), ambient);

    out.color = vec4<f32>(base_color.rgb * diffuse, base_color.a);
    return out;
}

// Helper to clean up vertex positioning based on face normal
fn get_quad_vertex(v_idx: u32, y_bottom: f32, y_top: f32, norm: vec3<f32>) -> vec3<f32> {
    // Top Caps
    if norm.y > 0.5 {
        switch v_idx {
            case 0u: { return vec3<f32>(0.0, y_top, 0.0); }
            case 1u: { return vec3<f32>(0.0, y_top, 1.0); }
            case 2u: { return vec3<f32>(1.0, y_top, 1.0); }
            case 3u: { return vec3<f32>(0.0, y_top, 0.0); }
            case 4u: { return vec3<f32>(1.0, y_top, 1.0); }
            default: { return vec3<f32>(1.0, y_top, 0.0); }
        }
    }
    // Front Wall (-Z)
    if norm.z < -0.5 {
        switch v_idx {
            case 0u: { return vec3<f32>(0.0, y_bottom, 0.0); }
            case 1u: { return vec3<f32>(0.0, y_top, 0.0); }
            case 2u: { return vec3<f32>(1.0, y_top, 0.0); }
            case 3u: { return vec3<f32>(0.0, y_bottom, 0.0); }
            case 4u: { return vec3<f32>(1.0, y_top, 0.0); }
            default: { return vec3<f32>(1.0, y_bottom, 0.0); }
        }
    }
    // Back Wall (+Z)
    if norm.z > 0.5 {
        switch v_idx {
            case 0u: { return vec3<f32>(1.0, y_bottom, 1.0); }
            case 1u: { return vec3<f32>(1.0, y_top, 1.0); }
            case 2u: { return vec3<f32>(0.0, y_top, 1.0); }
            case 3u: { return vec3<f32>(1.0, y_bottom, 1.0); }
            case 4u: { return vec3<f32>(0.0, y_top, 1.0); }
            default: { return vec3<f32>(0.0, y_bottom, 1.0); }
        }
    }
    // Right Wall (+X)
    if norm.x > 0.5 {
        switch v_idx {
            case 0u: { return vec3<f32>(1.0, y_bottom, 0.0); }
            case 1u: { return vec3<f32>(1.0, y_top, 0.0); }
            case 2u: { return vec3<f32>(1.0, y_top, 1.0); }
            case 3u: { return vec3<f32>(1.0, y_bottom, 0.0); }
            case 4u: { return vec3<f32>(1.0, y_top, 1.0); }
            default: { return vec3<f32>(1.0, y_bottom, 1.0); }
        }
    }
    // Left Wall (-X)
    switch v_idx {
        case 0u: { return vec3<f32>(0.0, y_bottom, 1.0); }
        case 1u: { return vec3<f32>(0.0, y_top, 1.0); }
        case 2u: { return vec3<f32>(0.0, y_top, 0.0); }
        case 3u: { return vec3<f32>(0.0, y_bottom, 1.0); }
        case 4u: { return vec3<f32>(0.0, y_top, 0.0); }
        default: { return vec3<f32>(0.0, y_bottom, 0.0); }
    }
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
