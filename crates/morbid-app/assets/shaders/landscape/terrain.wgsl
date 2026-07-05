#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}
#import "shaders/landscape/common.wgsl"::{window, world_buffer, palette, get_cell_data, get_layer_elevation, VertexInput, VertexOutput}

@vertex
fn vertex(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.origin_size.z);
    let grid_height = i32(window.origin_size.w);
    let scale = window.config.x;
    let layer_idx = 0u;

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

    var base_color = palette[data.terrain_mat];
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

    out.color = base_color;
    out.layer_weight = 1.0;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dx = dpdx(in.world_pos);
    let dy = dpdy(in.world_pos);
    var normal = normalize(cross(dx, dy));

    if normal.y < 0.0 { normal = -normal; }

    if in.layer_weight <= 0.05 { discard; }

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let diffuse = max(dot(normal, light_dir), 0.3);

    return vec4<f32>(in.color.rgb * diffuse, in.color.a);
}
