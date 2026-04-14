#import bevy_sprite::mesh2d_vertex_output::VertexOutput

struct WorldWindow {
    origin: vec2<f32>,
    size: vec2<f32>,
    head: vec2<f32>,
}

@group(2) @binding(0) var<storage, read> world_buffer: array<u32>;
@group(2) @binding(1) var<storage, read> palette: array<vec4<f32>>;
@group(2) @binding(2) var<uniform> window: WorldWindow;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let local_pos = mesh.world_position.xy - window.origin;

    // If the pixel is outside our active grid simulation, render void/black
    if local_pos.x < 0.0 || local_pos.y < 0.0 || local_pos.x >= window.size.x || local_pos.y >= window.size.y {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let grid_w = i32(window.size.x);
    let grid_h = i32(window.size.y);

    let cell_x = i32(floor(local_pos.x + window.head.x));
    let cell_y = i32(floor(local_pos.y + window.head.y));

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

    // Process valid materials (Ignore 0 = EMPTY and 255 = VOID)
    if active_mat != 0u && active_mat != 255u {
        var color = palette[active_mat];

        // The variant byte (0-255) becomes a visual multiplier ranging from -1.0 to 1.0.
        // If variant is 128, the shift is 0.0 (default palette color).
        let visual_shift = (f32(active_variant) - 128.0) / 128.0;

        // --- LIQUIDS (1 - 31) ---
        if active_mat >= 1u && active_mat <= 31u {
            // Dynamic State: represents fluid depth/mass
            let depth_factor = f32(active_state) / 255.0;
            let darken = depth_factor * 0.4;
            color.r = saturate(color.r - darken);
            color.g = saturate(color.g - darken);
            color.b = saturate(color.b - darken);

            // Static Variant: shifts the liquid's hue (e.g. murky swamp water vs pristine water)
            color.g = saturate(color.g + visual_shift * 0.15);
            color.b = saturate(color.b - visual_shift * 0.1);
        }
        
        // --- ORGANICS (64 - 127) ---
        else if active_mat >= 64u && active_mat <= 127u {
            // Dynamic State: represents health/age. 
            // As organics age (state increases), they brown out and lose vibrancy.
            let age_factor = f32(active_state) / 255.0;
            color.g = saturate(color.g - age_factor * 0.3);
            color.r = saturate(color.r + age_factor * 0.1); // Shift toward brown/rot
            color.b = saturate(color.b - age_factor * 0.1);

            // Static Variant: shifts the species hue (Human pink vs Goblin green)
            color.r = saturate(color.r + visual_shift * 0.3);
            color.g = saturate(color.g - visual_shift * 0.2);
        }
        
        // --- POWDERS & LOOSE SOLIDS (128 - 191) ---
        else if active_mat >= 128u && active_mat <= 191u {
            // Dynamic State: might represent moisture or compaction
            let moisture = f32(active_state) / 255.0;
            color.r = saturate(color.r - moisture * 0.25);
            color.g = saturate(color.g - moisture * 0.25);
            color.b = saturate(color.b - moisture * 0.25);

            // Static Variant: changes lightness (e.g., dark soil vs pale sand)
            color.r = saturate(color.r + visual_shift * 0.15);
            color.g = saturate(color.g + visual_shift * 0.15);
            color.b = saturate(color.b + visual_shift * 0.15);
        }
        
        // --- RIGID SOLIDS (192 - 254) ---
        else if active_mat >= 192u && active_mat <= 254u {
            // Static Variant: purely structural lightness (obsidian/basalt vs bright marble)
            color.r = saturate(color.r + visual_shift * 0.3);
            color.g = saturate(color.g + visual_shift * 0.3);
            color.b = saturate(color.b + visual_shift * 0.3);
        }

        return color;
    }

    // Fallback void/emptiness
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}