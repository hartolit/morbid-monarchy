#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin: vec2<f32>,
    size: vec2<f32>,
    head: vec2<f32>,
    h_max: f32,
    elevation_scale: f32,
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(10) cell_index: u32,
    @location(11) layer: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let grid_w = i32(window.size.x);
    let grid_h = i32(window.size.y);

    let x = i32(in.cell_index) % grid_w;
    let y = i32(in.cell_index) / grid_w;

    // Toroidal wrapping alignment
    let wrap_x = ((x + i32(window.head.x)) % grid_w + grid_w) % grid_w;
    let wrap_y = ((y + i32(window.head.y)) % grid_h + grid_h) % grid_h;
    let buffer_idx = wrap_y * grid_w + wrap_x;

    let cell_terrain = world_buffer[buffer_idx * 4 + 0];
    let cell_fluid = world_buffer[buffer_idx * 4 + 1];
    let cell_atmos = world_buffer[buffer_idx * 4 + 2];
    let cell_surface = world_buffer[buffer_idx * 4 + 3];

    let atmos_state = f32((cell_atmos >> 8u) & 0xFFu);
    let fluid_state = f32((cell_fluid >> 8u) & 0xFFu);

    // --- ALGEBRAIC MEMBRANE PHYSICS ---
    // Extract constants from uniform to avoid slop
    let z_terrain = max(0.0, window.h_max - (atmos_state * window.elevation_scale) - (fluid_state * window.elevation_scale));
    let fluid_depth = fluid_state * window.elevation_scale;

    var final_y_bottom = 0.0;
    var final_y_top = 0.0;
    var active_mat = 0u;
    var is_visible = true;

    // Evaluate Structural Bounds based on custom Layer Attribute
    if in.layer == 0u {
        // LAYER 0: Terrain Block
        active_mat = cell_terrain & 0xFFu;
        final_y_bottom = 0.0;
        final_y_top = z_terrain;
        if active_mat == 0u || active_mat == 255u { is_visible = false; }
    } else if in.layer == 1u {
        // LAYER 1: Fluid Block (Stacks on terrain)
        active_mat = cell_fluid & 0xFFu;
        final_y_bottom = z_terrain;
        final_y_top = z_terrain + fluid_depth;
        if active_mat == 0u || fluid_depth <= 0.0 { is_visible = false; }
    } else {
        // LAYER 2: Surface Block (Boats, Foliage, Items)
        active_mat = cell_surface & 0xFFu;
        final_y_bottom = z_terrain + fluid_depth;
        final_y_top = final_y_bottom + 1.0;
        if active_mat == 0u { is_visible = false; }
    }

    var local_pos = in.position;

    // Zero-cost geometric culling: plunge non-existent blocks underground
    if !is_visible {
        local_pos.y = -100.0;
        local_pos.x = 0.0;
        local_pos.z = 0.0;
    } else {
        // Stretch the standard 1x1x1 cube physically along the Y axis
        if local_pos.y > 0.5 {
            local_pos.y = final_y_top;
        } else {
            local_pos.y = final_y_bottom;
        }
    }

    out.clip_position = mesh_position_local_to_clip(get_world_from_local(0u), vec4<f32>(local_pos, 1.0));

    // Resolve color variance
    let active_variant = (world_buffer[buffer_idx * 4 + i32(in.layer)] >> 16u) & 0xFFu;
    var color = palette[active_mat];
    let visual_shift = (f32(active_variant) - 128.0) / 128.0;

    color.r = saturate(color.r + visual_shift * 0.15);
    color.g = saturate(color.g + visual_shift * 0.15);
    color.b = saturate(color.b + visual_shift * 0.15);

    // Apply strict geometric Lambertian lighting
    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let diffuse = max(dot(in.normal, light_dir), 0.3);
    out.color = vec4<f32>(color.rgb * diffuse, color.a);

    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
