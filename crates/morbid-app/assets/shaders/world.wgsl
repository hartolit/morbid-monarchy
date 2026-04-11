#import bevy_sprite::mesh2d_vertex_output::VertexOutput

struct WorldWindow {
    origin: vec2<f32>,
    size: vec2<f32>,
}

@group(2) @binding(0) var<storage, read> world_buffer: array<u32>;
@group(2) @binding(1) var<storage, read> palette: array<vec4<f32>>;
@group(2) @binding(2) var<uniform> window: WorldWindow;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let local_pos = in.world_position.xy - window.origin;

    // If the pixel is outside our active grid simulation, render void/black
    if local_pos.x < 0.0 || local_pos.y < 0.0 || local_pos.x >= window.size.x || local_pos.y >= window.size.y {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let grid_w = i32(window.size.x);
    let grid_h = i32(window.size.y);

    let cell_x = i32(floor(in.world_position.x));
    let cell_y = i32(floor(in.world_position.y));

    // Handle Toroidal Wrapping
    let wrap_x = ((cell_x % grid_w) + grid_w) % grid_w;
    let wrap_y = ((cell_y % grid_h) + grid_h) % grid_h;

    let index = wrap_y * grid_w + wrap_x;

    let cell_terrain = world_buffer[index * 4 + 0];
    let cell_fluid = world_buffer[index * 4 + 1];
    let cell_atmos = world_buffer[index * 4 + 2];
    let cell_surface = world_buffer[index * 4 + 3];

    // Unpack Material, State, and Variant (Little-Endian shifting)
    let mat_surface = cell_surface & 0xFFu;
    let state_surface = (cell_surface >> 8u) & 0xFFu;
    let variant_surface = (cell_surface >> 16u) & 0xFFu;

    let mat_fluid = cell_fluid & 0xFFu;
    let state_fluid = (cell_fluid >> 8u) & 0xFFu;
    let variant_fluid = (cell_fluid >> 16u) & 0xFFu;

    let mat_terrain = cell_terrain & 0xFFu;
    let state_terrain = (cell_terrain >> 8u) & 0xFFu;
    let variant_terrain = (cell_terrain >> 16u) & 0xFFu;

    var active_mat = 0u;
    var active_state = 0u;
    var active_variant = 0u;

    if mat_surface != 0u {
        active_mat = mat_surface;
        active_state = state_surface;
        active_variant = variant_surface;
    } else if mat_fluid != 0u {
        active_mat = mat_fluid;
        active_state = state_fluid;
        active_variant = variant_fluid;
    } else if mat_terrain != 0u {
        active_mat = mat_terrain;
        active_state = state_terrain;
        active_variant = variant_terrain;
    }

    if active_mat != 0u {
        var color = palette[active_mat];

        if active_mat == 3u {
            // GRASS: Bloom towards rich green based on strength
            let strength = f32(active_state) / 10.0;
            color.r = saturate(color.r - strength * 0.05);
            color.g = saturate(color.g + strength * 0.2);
            color.b = saturate(color.b - strength * 0.05);
        } else if active_mat == 4u {
            // SAND: Glow towards a brighter, saturated gold based on strength
            let strength = f32(active_state) / 10.0;
            color.r = saturate(color.r + strength * 0.1);
            color.g = saturate(color.g + strength * 0.05);
            color.b = saturate(color.b - strength * 0.05);
        } else if active_mat == 1u {
            // WATER: strictly apply the quantized depth mass
            let depth_factor = f32(active_state) / 255.0;
            let darken = depth_factor * 0.4;
            color.r = saturate(color.r - darken);
            color.g = saturate(color.g - darken);
            color.b = saturate(color.b - darken);
        }

        return color;
    }

    // Fallback emptiness
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
