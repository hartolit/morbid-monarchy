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

struct VertexInput {
    @location(0) _position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) layer_weight: f32,
    @location(2) world_pos: vec3<f32>,
};

const N_OFFSETS: array<vec2<i32>, 8> = array<vec2<i32>, 8>(
    vec2<i32>(1, 0),  vec2<i32>(-1, 0), vec2<i32>(0, 1),  vec2<i32>(0, -1),
    vec2<i32>(1, 1),  vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1)
);

// Pre-normalized directional light vector vec3(0.5, 1.0, 0.3)
const LIGHT_DIR: vec3<f32> = vec3<f32>(0.4319342, 0.8638684, 0.2591605);

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

fn extract_layer_mat(data: CellData, layer_idx: u32) -> u32 {
    switch layer_idx {
        case 0u: { return data.terrain_mat; }
        case 1u: { return data.granular_mat; }
        case 2u: { return data.fluid_mat; }
        case 3u: { return data.surface_mat; }
        default: { return 0u; }
    }
}

fn extract_layer_height(data: CellData, layer_idx: u32) -> f32 {
    switch layer_idx {
        case 0u: { return data.terrain_h; }
        case 1u: { return data.granular_h; }
        case 2u: { return data.fluid_h; }
        case 3u: { return data.surface_h; }
        default: { return 0.0; }
    }
}

struct LayerSample {
    elevation: f32,
    mat_lookup: u32,
    has_mat: bool,
}

// Single-pass sampling: fetches elevation and fallback material simultaneously
fn get_layer_elevation_and_mat(
    cell_x: i32, cell_y: i32, grid_w: i32, grid_h: i32, scale: f32, layer_idx: u32
) -> LayerSample {
    var sample: LayerSample;
    let center = get_cell_data(cell_x, cell_y, grid_w, grid_h, scale);

    if layer_idx == 0u {
        sample.elevation = center.terrain_h;
        sample.mat_lookup = center.terrain_mat;
        sample.has_mat = true;
        return sample;
    }

    let active_mat = extract_layer_mat(center, layer_idx);
    let active_h = extract_layer_height(center, layer_idx);
    sample.has_mat = active_mat != 0u;

    if sample.has_mat {
        sample.elevation = active_h;
        sample.mat_lookup = active_mat;
        return sample;
    }

    let floor_h = max(center.terrain_h, select(0.0, center.granular_h, center.granular_mat != 0u));
    let hidden_h = floor_h - 0.5;

    var max_n_h = 0.0;
    var fallback_mat = 0u;

    for (var i = 0; i < 8; i++) {
        let offset = N_OFFSETS[i];
        let n_data = get_cell_data(cell_x + offset.x, cell_y + offset.y, grid_w, grid_h, scale);
        let n_mat = extract_layer_mat(n_data, layer_idx);

        if n_mat != 0u {
            if fallback_mat == 0u {
                fallback_mat = n_mat;
            }
            let n_h = extract_layer_height(n_data, layer_idx);
            max_n_h = max(max_n_h, n_h);
        }
    }

    sample.mat_lookup = fallback_mat;
    if max_n_h > 0.0 && floor_h >= max_n_h {
        sample.elevation = max_n_h;
    } else {
        sample.elevation = hidden_h;
    }

    return sample;
}

fn process_landscape_vertex(
    in: VertexInput, instance_index: u32, layer_idx: u32
) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.origin_size.z);
    let grid_height = i32(window.origin_size.w);
    let scale = window.config.x;

    let chunk_matrix = bevy_pbr::mesh_functions::get_world_from_local(instance_index);
    let chunk_origin_x = i32(round(chunk_matrix[3].x));
    let chunk_origin_y = i32(round(-chunk_matrix[3].z));

    let local_x = i32(round(in._position.x));
    let local_y = i32(round(-in._position.z));

    let world_cell_x = chunk_origin_x + local_x;
    let world_cell_y = chunk_origin_y + local_y;

    let local_grid_x = world_cell_x - i32(window.origin_size.x);
    let local_grid_y = world_cell_y - i32(window.origin_size.y);

    let layer_sample = get_layer_elevation_and_mat(local_grid_x, local_grid_y, grid_width, grid_height, scale, layer_idx);
    let local_pos = vec3<f32>(in._position.x, layer_sample.elevation, in._position.z);

    out.clip_position = bevy_pbr::mesh_functions::mesh_position_local_to_clip(chunk_matrix, vec4<f32>(local_pos, 1.0));
    out.world_pos = (chunk_matrix * vec4<f32>(local_pos, 1.0)).xyz;

    let data = get_cell_data(local_grid_x, local_grid_y, grid_width, grid_height, scale);

    if layer_idx == 0u {
        out.layer_weight = 1.0;
    } else {
        out.layer_weight = select(0.0, 1.0, layer_sample.has_mat);
    }

    let mat_offset = layer_idx * 32u;
    let palette_idx = layer_sample.mat_lookup + mat_offset;
    var base_color = palette[palette_idx];

    let visual_shift = (f32(data.variants) - 16.0) * 0.00625;
    base_color = vec4<f32>(saturate(base_color.rgb + vec3<f32>(visual_shift)), base_color.a);

    let dx = f32(world_cell_x) - window.head_cursor.z;
    let dy = f32(world_cell_y) - window.head_cursor.w;
    let cursor_r = window.config.y;
    if cursor_r >= 0.0 && (dx * dx + dy * dy) <= (cursor_r * cursor_r + 0.1) {
        base_color = mix(base_color, vec4<f32>(1.0, 0.4, 0.4, 1.0), 0.35);
    }

    out.color = base_color;
    return out;
}

fn process_landscape_fragment(in: VertexOutput) -> vec4<f32> {
    if in.layer_weight <= 0.05 { discard; }

    let dx = dpdx(in.world_pos);
    let dy = dpdy(in.world_pos);
    var normal = normalize(cross(dx, dy));
    if normal.y < 0.0 { normal = -normal; }

    let diffuse = max(dot(normal, LIGHT_DIR), 0.3);
    return vec4<f32>(in.color.rgb * diffuse, in.color.a);
}
