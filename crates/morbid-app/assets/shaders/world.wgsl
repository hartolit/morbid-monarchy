#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin_size: vec4<f32>, // x: origin.x, y: origin.y, z: size.x, w: size.y
    head_cursor: vec4<f32>, // x: head.x, y: head.y, z: cursor.x, w: cursor.y
    config: vec4<f32>,      // x: elev_scale, y: cursor_radius
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 120u; // 20 faces * 6 verts

// Returns absolute heights: x=Terrain, y=Granular, z=Fluid, w=Surface
fn calculate_heights_at(
    cell_x: i32, cell_y: i32,
    grid_width: i32, grid_height: i32,
    elevation_scale: f32
) -> vec4<f32> {
    let cx = clamp(cell_x, 0, grid_width - 1);
    let cy = clamp(cell_y, 0, grid_height - 1);

    let wrapped_x = ((cx + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cy + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;

    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;
    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    // Extract raw units
    let mat_surface = (word_0 >> 4u) & 0xFu;
    let elevation = f32((word_0 >> 20u) & 0xFFFu);
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0xFu);
    let surface_state = f32((word_1 >> 13u) & 0x1FFu);

    // Compute stacked absolute heights
    let t_height = elevation * elevation_scale;
    let g_height = t_height + (granular_vol * elevation_scale);
    let f_height = g_height + (fluid_vol * elevation_scale);

    // Anchor Surface directly to the Granular crust, bypassing Fluid displacement
    var s_height = g_height;
    if mat_surface != 0u {
        // Enforce a minimum thickness of 1.0 so empty states still render a slim base
        s_height = g_height + max(1.0, surface_state) * elevation_scale;
    }

    return vec4<f32>(t_height, g_height, f_height, s_height);
}

struct VertexInput {
    @location(0) _position: vec3<f32>,
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.origin_size.z);
    let grid_height = i32(window.origin_size.w);
    let scale = window.config.x;

    let chunk_matrix = get_world_from_local(in.instance_index);
    let chunk_origin_x = i32(round(chunk_matrix[3].x));
    let chunk_origin_y = i32(round(-chunk_matrix[3].z));

    // Convert the negative Z back to positive Y for grid lookups
    let local_x = i32(round(in._position.x));
    let local_y = i32(round(-in._position.z));

    let world_cell_x = chunk_origin_x + local_x;
    let world_cell_y = chunk_origin_y + local_y;

    let local_grid_x = world_cell_x - i32(window.origin_size.x);
    let local_grid_y = world_cell_y - i32(window.origin_size.y);

    // Get exact heights for this vertex and its 4 neighbors to calculate smooth normals
    let h_center = calculate_heights_at(local_grid_x, local_grid_y, grid_width, grid_height, scale);
    let h_left = calculate_heights_at(local_grid_x - 1, local_grid_y, grid_width, grid_height, scale);
    let h_right = calculate_heights_at(local_grid_x + 1, local_grid_y, grid_width, grid_height, scale);
    let h_up = calculate_heights_at(local_grid_x, local_grid_y - 1, grid_width, grid_height, scale);
    let h_down = calculate_heights_at(local_grid_x, local_grid_y + 1, grid_width, grid_height, scale);

    // Extract the topmost visual layer (Surface > Fluid > Granular > Terrain)
    let top_h = max(h_center.w, max(h_center.z, max(h_center.y, h_center.x)));
    let l_top = max(h_left.w, max(h_left.z, max(h_left.y, h_left.x)));
    let r_top = max(h_right.w, max(h_right.z, max(h_right.y, h_right.x)));
    let u_top = max(h_up.w, max(h_up.z, max(h_up.y, h_up.x)));
    let d_top = max(h_down.w, max(h_down.z, max(h_down.y, h_down.x)));

    // Calculate Central Difference Normal
    // Cross product of X-gradient and Z-gradient vectors
    let normal = normalize(vec3<f32>(l_top - r_top, 2.0, d_top - u_top));

    // Calculate the actual 3D vertex position
    let local_pos = vec3<f32>(in._position.x, top_h, in._position.z);

    out.normal = normal;
    out.clip_position = mesh_position_local_to_clip(chunk_matrix, vec4<f32>(local_pos, 1.0));

    // Color Lookup (Using the base cell to determine material)
    let wrapped_x = ((local_grid_x + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((local_grid_y + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;
    let word_0 = world_buffer[buffer_index];

    let mat_terrain = word_0 & 0xFu;
    let mat_surface = (word_0 >> 4u) & 0xFu;
    let mat_granular = (word_0 >> 8u) & 0x7u;
    let mat_fluid = (word_0 >> 11u) & 0xFu;
    let variants = (word_0 >> 15u) & 0x1Fu;

    // Determine the topmost material index by checking which layer is physically highest
    var mat_lookup = mat_terrain;
    var max_h = h_center.x;

    if mat_granular != 0u && h_center.y >= max_h {
        mat_lookup = mat_granular + 32u;
        max_h = h_center.y;
    }

    // Surface is anchored to the crust. It wins if it's taller than the terrain/granular layer.
    if mat_surface != 0u && h_center.w >= max_h {
        mat_lookup = mat_surface + 96u;
        max_h = h_center.w;
    }

    // Fluid is also anchored to the crust. If it is deeper than the surface is tall, it drowns the surface.
    if mat_fluid != 0u && h_center.z > max_h {
        mat_lookup = mat_fluid + 64u;
        max_h = h_center.z;
    }

    var base_color = palette[mat_lookup];
    let visual_shift = (f32(variants) - 16.0) / 16.0 * 0.10;
    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    // Apply lighting
    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let ambient = 0.3;
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
