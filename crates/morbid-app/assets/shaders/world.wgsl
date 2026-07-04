#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin_size: vec4<f32>,
    head_cursor: vec4<f32>,
    config: vec4<f32>,
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
    data.granular_mat = (word_0 >> 8u) & 0x7u;
    data.fluid_mat = (word_0 >> 11u) & 0xFu;
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
    data: CellData, n1: CellData, n2: CellData, n3: CellData, n4: CellData, layer_idx: u32
) -> f32 {
    if layer_idx == 0u { return data.terrain_h; }

    var has_mat = false;
    var active_h = 0.0;

    if layer_idx == 1u { has_mat = data.granular_mat != 0u; active_h = data.granular_h; }
    else if layer_idx == 2u { has_mat = data.fluid_mat != 0u; active_h = data.fluid_h; }
    else if layer_idx == 3u { has_mat = data.surface_mat != 0u; active_h = data.surface_h; }

    if has_mat { return active_h; }

    let floor_h = max(data.terrain_h, select(0.0, data.granular_h, data.granular_mat != 0u));
    let hidden_h = floor_h - 0.5;

    var max_n_h = 0.0;
    if layer_idx == 1u {
        max_n_h = max(max(select(0.0, n1.granular_h, n1.granular_mat != 0u), select(0.0, n2.granular_h, n2.granular_mat != 0u)),
            max(select(0.0, n3.granular_h, n3.granular_mat != 0u), select(0.0, n4.granular_h, n4.granular_mat != 0u)));
    } else if layer_idx == 2u {
        max_n_h = max(max(select(0.0, n1.fluid_h, n1.fluid_mat != 0u), select(0.0, n2.fluid_h, n2.fluid_mat != 0u)),
            max(select(0.0, n3.fluid_h, n3.fluid_mat != 0u), select(0.0, n4.fluid_h, n4.fluid_mat != 0u)));
    } else if layer_idx == 3u {
        max_n_h = max(max(select(0.0, n1.surface_h, n1.surface_mat != 0u), select(0.0, n2.surface_h, n2.surface_mat != 0u)),
            max(select(0.0, n3.surface_h, n3.surface_mat != 0u), select(0.0, n4.surface_h, n4.surface_mat != 0u)));
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
    @location(1) normal: vec3<f32>,
    @location(2) layer_weight: f32,
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

    let data = get_cell_data(local_grid_x, local_grid_y, grid_width, grid_height, scale);
    let n1 = get_cell_data(local_grid_x - 1, local_grid_y, grid_width, grid_height, scale);
    let n2 = get_cell_data(local_grid_x + 1, local_grid_y, grid_width, grid_height, scale);
    let n3 = get_cell_data(local_grid_x, local_grid_y - 1, grid_width, grid_height, scale);
    let n4 = get_cell_data(local_grid_x, local_grid_y + 1, grid_width, grid_height, scale);

    let c_h = get_layer_elevation(data, n1, n2, n3, n4, layer_idx);
    let l_h = get_layer_elevation(n1, get_cell_data(local_grid_x - 2, local_grid_y, grid_width, grid_height, scale), data, get_cell_data(local_grid_x - 1, local_grid_y - 1, grid_width, grid_height, scale), get_cell_data(local_grid_x - 1, local_grid_y + 1, grid_width, grid_height, scale), layer_idx);
    let r_h = get_layer_elevation(n2, data, get_cell_data(local_grid_x + 2, local_grid_y, grid_width, grid_height, scale), get_cell_data(local_grid_x + 1, local_grid_y - 1, grid_width, grid_height, scale), get_cell_data(local_grid_x + 1, local_grid_y + 1, grid_width, grid_height, scale), layer_idx);
    let u_h = get_layer_elevation(n3, get_cell_data(local_grid_x - 1, local_grid_y - 1, grid_width, grid_height, scale), get_cell_data(local_grid_x + 1, local_grid_y - 1, grid_width, grid_height, scale), get_cell_data(local_grid_x, local_grid_y - 2, grid_width, grid_height, scale), data, layer_idx);
    let d_h = get_layer_elevation(n4, get_cell_data(local_grid_x - 1, local_grid_y + 1, grid_width, grid_height, scale), get_cell_data(local_grid_x + 1, local_grid_y + 1, grid_width, grid_height, scale), data, get_cell_data(local_grid_x, local_grid_y + 2, grid_width, grid_height, scale), layer_idx);

    let normal = normalize(vec3<f32>(l_h - r_h, 2.0, d_h - u_h));
    let local_pos = vec3<f32>(in._position.x, c_h, in._position.z);

    out.normal = normal;
    out.clip_position = mesh_position_local_to_clip(chunk_matrix, vec4<f32>(local_pos, 1.0));

    var mat_lookup = 0u;
    var has_mat = false;

    if layer_idx == 0u {
        mat_lookup = data.terrain_mat;
        has_mat = true;
    } else if layer_idx == 1u {
        mat_lookup = data.granular_mat;
        has_mat = mat_lookup != 0u;
        if mat_lookup == 0u { mat_lookup = max(max(n1.granular_mat, n2.granular_mat), max(n3.granular_mat, n4.granular_mat)); }
        mat_lookup += 32u;
    } else if layer_idx == 2u {
        mat_lookup = data.fluid_mat;
        has_mat = mat_lookup != 0u;
        if mat_lookup == 0u { mat_lookup = max(max(n1.fluid_mat, n2.fluid_mat), max(n3.fluid_mat, n4.fluid_mat)); }
        mat_lookup += 64u;
    } else if layer_idx == 3u {
        mat_lookup = data.surface_mat;
        has_mat = mat_lookup != 0u;
        if mat_lookup == 0u { mat_lookup = max(max(n1.surface_mat, n2.surface_mat), max(n3.surface_mat, n4.surface_mat)); }
        mat_lookup += 96u;
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

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let diffuse = max(dot(normal, light_dir), 0.3);

    out.color = vec4<f32>(base_color.rgb * diffuse, base_color.a);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // A threshold of 0.05 guarantees the mesh is allowed to dip smoothly
    // past the shoreline and into the ground. Once safely buried, it deletes
    // the rest of the phantom geometry to eradicate Z-fighting on slopes.
    if in.layer_weight <= 0.05 {
        discard;
    }
    return in.color;
}
