#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin_size: vec4<f32>,
    head_cursor: vec4<f32>,
    config: vec4<f32>, // x: elev_scale, y: cursor_radius, z: LAYER_INDEX
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

struct CellData {
    terrain_mat: u32,
    granular_mat: u32,
    fluid_mat: u32,
    surface_mat: u32,
    variants: u32,
    terrain_h: f32,
    granular_h: f32,
    fluid_h: f32,
    surface_h: f32,
}

fn get_cell_data(
    cell_x: i32, cell_y: i32, grid_width: i32, grid_height: i32, scale: f32
) -> CellData {
    var data: CellData;
    let cx = clamp(cell_x, 0, grid_width - 1);
    let cy = clamp(cell_y, 0, grid_height - 1);

    let wrapped_x = ((cx + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cy + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;

    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;
    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    data.terrain_mat = word_0 & 0xFu;
    data.surface_mat = (word_0 >> 4u) & 0xFu;
    data.granular_mat = (word_0 >> 8u) & 0xFu;
    data.fluid_mat = (word_0 >> 12u) & 0x7u;
    data.variants = (word_0 >> 15u) & 0x1Fu;

    let elevation = f32((word_0 >> 20u) & 0xFFFu);
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0xFu);
    let surface_state = f32((word_1 >> 13u) & 0x1FFu);

    data.terrain_h = elevation * scale;
    data.granular_h = data.terrain_h + (granular_vol * scale);
    data.fluid_h = data.granular_h + (fluid_vol * scale);
    data.surface_h = data.granular_h + max(1.0, surface_state) * scale;

    return data;
}

fn get_layer_elevation(
    cell_x: i32, cell_y: i32, grid_w: i32, grid_h: i32, scale: f32, layer_idx: u32
) -> f32 {
    let center = get_cell_data(cell_x, cell_y, grid_w, grid_h, scale);
    if layer_idx == 0u { return center.terrain_h; }

    var has_mat = false;
    var active_h = 0.0;

    if layer_idx == 1u { has_mat = center.granular_mat != 0u; active_h = center.granular_h; }
    else if layer_idx == 2u { has_mat = center.fluid_mat != 0u; active_h = center.fluid_h; }
    else if layer_idx == 3u { has_mat = center.surface_mat != 0u; active_h = center.surface_h; }

    if has_mat { return active_h; }

    let floor_h = max(center.terrain_h, select(0.0, center.granular_h, center.granular_mat != 0u));
    let hidden_h = floor_h - 0.5;

    let n_offsets = array<vec2<i32>, 8>(
        vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
        vec2<i32>(1, 1), vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1)
    );

    // Check ALL 8 neighbors (cardinal + diagonal) to prevent diagonal interpolation spikes
    var max_n_h = 0.0;
    for (var i = 0; i < 8; i++) {
        let offset = n_offsets[i];
        let n_data = get_cell_data(cell_x + offset.x, cell_y + offset.y, grid_w, grid_h, scale);

        var n_h = 0.0;
        if layer_idx == 1u && n_data.granular_mat != 0u { n_h = n_data.granular_h; }
        else if layer_idx == 2u && n_data.fluid_mat != 0u { n_h = n_data.fluid_h; }
        else if layer_idx == 3u && n_data.surface_mat != 0u { n_h = n_data.surface_h; }

        max_n_h = max(max_n_h, n_h);
    }

    if max_n_h > 0.0 && floor_h >= max_n_h {
        return max_n_h;
    }

    return hidden_h;
}

struct VertexInput {
    @location(0) _position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) layer_weight: f32,
    @location(2) world_pos: vec3<f32>, // Passed to fragment for flat shading derivatives
};

@vertex
fn vertex(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.origin_size.z);
    let grid_height = i32(window.origin_size.w);
    let scale = window.config.x;
    let layer_idx = u32(window.config.z);

    let chunk_matrix = get_world_from_local(instance_index);
    let chunk_origin_x = i32(round(chunk_matrix[3].x));
    let chunk_origin_y = i32(round(-chunk_matrix[3].z));

    let local_x = i32(round(in._position.x));
    let local_y = i32(round(-in._position.z));

    let world_cell_x = chunk_origin_x + local_x;
    let world_cell_y = chunk_origin_y + local_y;

    let local_grid_x = world_cell_x - i32(window.origin_size.x);
    let local_grid_y = world_cell_y - i32(window.origin_size.y);

    let c_h = get_layer_elevation(local_grid_x, local_grid_y, grid_width, grid_height, scale, layer_idx);
    let local_pos = vec3<f32>(in._position.x, c_h, in._position.z);

    out.clip_position = mesh_position_local_to_clip(chunk_matrix, vec4<f32>(local_pos, 1.0));
    out.world_pos = (chunk_matrix * vec4<f32>(local_pos, 1.0)).xyz;

    let data = get_cell_data(local_grid_x, local_grid_y, grid_width, grid_height, scale);

    var mat_lookup = 0u;
    var has_mat = false;

    if layer_idx == 0u {
        mat_lookup = data.terrain_mat;
        has_mat = true;
    } else {
        if layer_idx == 1u { mat_lookup = data.granular_mat; }
        else if layer_idx == 2u { mat_lookup = data.fluid_mat; }
        else if layer_idx == 3u { mat_lookup = data.surface_mat; }

        has_mat = mat_lookup != 0u;

        if !has_mat {
            // Borrow material color from any of the 8 neighbors to prevent color bleeding on skirts
            let n_offsets = array<vec2<i32>, 8>(
                vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
                vec2<i32>(1, 1), vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1)
            );
            for (var i = 0; i < 8; i++) {
                let offset = n_offsets[i];
                let n = get_cell_data(local_grid_x + offset.x, local_grid_y + offset.y, grid_width, grid_height, scale);
                if layer_idx == 1u && n.granular_mat != 0u { mat_lookup = n.granular_mat; break; }
                if layer_idx == 2u && n.fluid_mat != 0u { mat_lookup = n.fluid_mat; break; }
                if layer_idx == 3u && n.surface_mat != 0u { mat_lookup = n.surface_mat; break; }
            }
        }

        if layer_idx == 1u { mat_lookup += 32u; }
        else if layer_idx == 2u { mat_lookup += 64u; }
        else if layer_idx == 3u { mat_lookup += 96u; }
    }

    out.layer_weight = select(0.0, 1.0, has_mat);

    var base_color = palette[mat_lookup];
    let visual_shift = (f32(data.variants) - 16.0) / 16.0 * 0.10;
    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    let true_world_x = f32(world_cell_x);
    let true_world_y = f32(world_cell_y);
    let dx = true_world_x - window.head_cursor.z;
    let dy = true_world_y - window.head_cursor.w;

    if window.config.y >= 0.0 && (dx * dx + dy * dy) <= (window.config.y * window.config.y) + 0.1 {
        base_color = mix(base_color, vec4<f32>(1.0, 0.4, 0.4, 1.0), 0.35);
    }

    // Pass unlit color; lighting is calculated per-face in the fragment shader
    out.color = base_color;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Calculate derivatives FIRST to prevent 2x2 Quad helper lane corruption!
    let dx = dpdx(in.world_pos);
    let dy = dpdy(in.world_pos);
    var normal = normalize(cross(dx, dy));

    // Ensure the normal faces outward regardless of screen-space drawing order
    if normal.y < 0.0 {
        normal = -normal;
    }

    if in.layer_weight <= 0.05 {
        discard;
    }

    // Apply lighting per-face based on the derivative normal
    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let diffuse = max(dot(normal, light_dir), 0.3);

    return vec4<f32>(in.color.rgb * diffuse, in.color.a);
}
