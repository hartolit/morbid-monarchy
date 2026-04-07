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
    // Calculate Grid offset via world coordinates
    let local_pos = in.world_position.xy - window.origin;

    let grid_w = i32(window.size.x);
    let grid_h = i32(window.size.y);

    let cell_x = i32(floor(in.world_position.x));
    let cell_y = i32(floor(in.world_position.y));

    // Handle Toroidal Wrapping
    // Using positive modulo logic since WGSL modulo follows C semantics (allows negative)
    let wrap_x = ((cell_x % grid_w) + grid_w) % grid_w;
    let wrap_y = ((cell_y % grid_h) + grid_h) % grid_h;

    let index = wrap_y * grid_w + wrap_x;

    // Read the 16 bytes (4 x 32-bit uints) making up the WorldCell
    let cell_terrain = world_buffer[index * 4 + 0];
    let cell_fluid   = world_buffer[index * 4 + 1];
    let cell_atmos   = world_buffer[index * 4 + 2];
    let cell_surface = world_buffer[index * 4 + 3];

    // Extract the MaterialId (which is the lowest 8 bits of our packed u32)
    let mat_surface = cell_surface & 0xFFu;
    let mat_fluid   = cell_fluid & 0xFFu;
    let mat_terrain = cell_terrain & 0xFFu;

    // Resolve the Z-Stack
    if (mat_surface != 0u) {
        return palette[mat_surface];
    } else if (mat_fluid != 0u) {
        return palette[mat_fluid];
    } else if (mat_terrain != 0u) {
        return palette[mat_terrain];
    }

    // Fallback emptiness
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
