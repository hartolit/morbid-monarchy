#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin_size: vec4<f32>, // x: origin.x, y: origin.y, z: size.x, w: size.y
    head_cursor: vec4<f32>, // x: head.x, y: head.y, z: cursor.x, w: cursor.y
    config: vec4<f32>,      // x: elev_scale, y: cursor_radius, z: visual_roughness, w: corner_warp
    time_data: vec4<f32>,   // x: elapsed_seconds
}

struct LayerSample {
    heights: vec4<f32>, // x=Terrain, y=Granular, z=Fluid, w=Surface
    present: vec4<u32>, // x=Terrain, y=Granular, z=Fluid, w=Surface
    mats: vec4<u32>,    // x=Terrain, y=Granular, z=Fluid, w=Surface
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 120u;

const LAYER_TERRAIN: u32 = 0u;
const LAYER_GRANULAR: u32 = 1u;
const LAYER_FLUID: u32 = 2u;
const LAYER_SURFACE: u32 = 3u;

// Procedural hashing for continuous geometry variance
fn hash11(position: vec2<f32>) -> f32 {
    return fract(sin(dot(position, vec2<f32>(127.1, 311.7))) * 43758.5453123);
}

fn hash22(position: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        hash11(position + vec2<f32>(19.19, 47.77)),
        hash11(position + vec2<f32>(83.13, 11.71))
    );
}

// Harvests structural data relative to the shifting window ToroidalGrid bounds
fn sample_layers_at(
    local_grid_x: i32,
    local_grid_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32
) -> LayerSample {
    var sample: LayerSample;

    if local_grid_x < 0 || local_grid_x >= grid_width || local_grid_y < 0 || local_grid_y >= grid_height {
        sample.heights = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        sample.present = vec4<u32>(0u, 0u, 0u, 0u);
        sample.mats = vec4<u32>(0u, 0u, 0u, 0u);
        return sample;
    }

    let wrapped_x = ((local_grid_x + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((local_grid_y + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;

    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    let mat_terrain = word_0 & 0xFu;
    let mat_surface = (word_0 >> 4u) & 0xFu;
    let mat_granular = (word_0 >> 8u) & 0x7u;
    let mat_fluid = (word_0 >> 11u) & 0xFu;

    let elevation = f32((word_0 >> 20u) & 0xFFFu);
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0x1FFu);
    let surface_state = f32((word_1 >> 18u) & 0x3Fu);

    let t_height = elevation * elevation_scale;
    let g_height = t_height + (granular_vol * elevation_scale);
    let f_height = g_height + (fluid_vol * elevation_scale);
    let s_height = select(f_height, f_height + max(1.0, surface_state) * elevation_scale, mat_surface != 0u);

    sample.heights = vec4<f32>(t_height, g_height, f_height, s_height);
    sample.present = vec4<u32>(
        select(0u, 1u, (mat_terrain != 0u) && (elevation > 0.0)),
        select(0u, 1u, (mat_granular != 0u) && (granular_vol > 0.0)),
        select(0u, 1u, (mat_fluid != 0u) && (fluid_vol > 0.0)),
        select(0u, 1u, mat_surface != 0u)
    );
    sample.mats = vec4<u32>(mat_terrain, mat_granular, mat_fluid, mat_surface);

    return sample;
}

fn layer_height(sample: LayerSample, layer: u32) -> f32 {
    if layer == LAYER_TERRAIN { return sample.heights.x; }
    if layer == LAYER_GRANULAR { return sample.heights.y; }
    if layer == LAYER_FLUID { return sample.heights.z; }
    return sample.heights.w;
}

fn layer_is_present(sample: LayerSample, layer: u32) -> bool {
    if layer == LAYER_TERRAIN { return sample.present.x != 0u; }
    if layer == LAYER_GRANULAR { return sample.present.y != 0u; }
    if layer == LAYER_FLUID { return sample.present.z != 0u; }
    return sample.present.w != 0u;
}

fn get_layer_mat(sample: LayerSample, layer: u32) -> u32 {
    if layer == LAYER_TERRAIN { return sample.mats.x; }
    if layer == LAYER_GRANULAR { return sample.mats.y; }
    if layer == LAYER_FLUID { return sample.mats.z; }
    return sample.mats.w;
}

// Reconstructs the absolute mathematical coordinate to guarantee deterministic hashing
fn corner_world_coordinate(local_grid_x: i32, local_grid_y: i32, corner_x: f32, corner_z: f32) -> vec2<f32> {
    var true_world_y = window.origin_size.y + f32(local_grid_y);
    if corner_z < 0.5 { true_world_y = true_world_y + 1.0; }
    return vec2<f32>(window.origin_size.x + f32(local_grid_x) + corner_x, true_world_y);
}

fn corner_planar_offset(corner_world: vec2<f32>) -> vec2<f32> {
    let warp_strength = max(window.config.w, 0.0);
    let jitter = hash22(corner_world) - vec2<f32>(0.5, 0.5);
    return jitter * (warp_strength * 2.0);
}

// Strictly evaluates layer parity to prevent dissimilar materials from blurring their geometry
fn corner_layer_height_pair(
    local_grid_x: i32,
    local_grid_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32,
    target_mat: u32
) -> vec2<f32> {
    let sample = sample_layers_at(local_grid_x, local_grid_y, grid_width, grid_height, elevation_scale);
    if layer_is_present(sample, layer) && get_layer_mat(sample, layer) == target_mat {
        return vec2<f32>(layer_height(sample, layer), 1.0);
    }
    return vec2<f32>(0.0, 0.0);
}

// Performs a 4-tap evaluation to average surrounding cell heights for homogenous meshes
fn layer_corner_height(
    local_grid_x: i32,
    local_grid_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32,
    target_mat: u32,
    corner_x: f32,
    corner_z: f32,
    fallback_height: f32
) -> f32 {
    var edge_dx: i32 = -1;
    if corner_x > 0.5 { edge_dx = 1; }

    var edge_dy: i32 = 1;
    if corner_z > 0.5 { edge_dy = -1; }

    var total = vec2<f32>(0.0, 0.0);
    total = total + corner_layer_height_pair(local_grid_x, local_grid_y, grid_width, grid_height, elevation_scale, layer, target_mat);
    total = total + corner_layer_height_pair(local_grid_x + edge_dx, local_grid_y, grid_width, grid_height, elevation_scale, layer, target_mat);
    total = total + corner_layer_height_pair(local_grid_x, local_grid_y + edge_dy, grid_width, grid_height, elevation_scale, layer, target_mat);
    total = total + corner_layer_height_pair(local_grid_x + edge_dx, local_grid_y + edge_dy, grid_width, grid_height, elevation_scale, layer, target_mat);

    if total.y < 0.5 { return fallback_height; }

    var height = total.x / total.y;

    if layer == LAYER_FLUID {
        let corner_world = corner_world_coordinate(local_grid_x, local_grid_y, corner_x, corner_z);
        let t = window.time_data.x;
        let wave_1 = sin(corner_world.x * 2.5 + t * 3.5);
        let wave_2 = cos(corner_world.y * 2.5 + t * 2.8);

        height += (wave_1 * wave_2 * 0.15);
    }

    return height;
}

fn cap_corner_for_vertex(vertex_index: u32) -> vec2<f32> {
    switch vertex_index {
        case 0u: { return vec2<f32>(0.0, 0.0); }
        case 1u: { return vec2<f32>(0.0, 1.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        case 3u: { return vec2<f32>(0.0, 0.0); }
        case 4u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(1.0, 0.0); }
    }
}

fn wall_corner_for_vertex(vertex_index: u32, norm: vec3<f32>) -> vec3<f32> {
    if norm.z < -0.5 {
        switch vertex_index {
            case 0u: { return vec3<f32>(0.0, 0.0, 0.0); }
            case 1u: { return vec3<f32>(0.0, 0.0, 1.0); }
            case 2u: { return vec3<f32>(1.0, 0.0, 1.0); }
            case 3u: { return vec3<f32>(0.0, 0.0, 0.0); }
            case 4u: { return vec3<f32>(1.0, 0.0, 1.0); }
            default: { return vec3<f32>(1.0, 0.0, 0.0); }
        }
    }
    if norm.z > 0.5 {
        switch vertex_index {
            case 0u: { return vec3<f32>(1.0, 1.0, 0.0); }
            case 1u: { return vec3<f32>(1.0, 1.0, 1.0); }
            case 2u: { return vec3<f32>(0.0, 1.0, 1.0); }
            case 3u: { return vec3<f32>(1.0, 1.0, 0.0); }
            case 4u: { return vec3<f32>(0.0, 1.0, 1.0); }
            default: { return vec3<f32>(0.0, 1.0, 0.0); }
        }
    }
    if norm.x > 0.5 {
        switch vertex_index {
            case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }
            case 1u: { return vec3<f32>(1.0, 0.0, 1.0); }
            case 2u: { return vec3<f32>(1.0, 1.0, 1.0); }
            case 3u: { return vec3<f32>(1.0, 0.0, 0.0); }
            case 4u: { return vec3<f32>(1.0, 1.0, 1.0); }
            default: { return vec3<f32>(1.0, 1.0, 0.0); }
        }
    }
    switch vertex_index {
        case 0u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 1u: { return vec3<f32>(0.0, 1.0, 1.0); }
        case 2u: { return vec3<f32>(0.0, 0.0, 1.0); }
        case 3u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 4u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.0, 0.0, 0.0); }
    }
}

fn lively_quad_vertex(
    vertex_index: u32,
    y_bottom: f32,
    y_top: f32,
    norm: vec3<f32>,
    local_grid_x: i32,
    local_grid_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32,
    target_mat: u32
) -> vec3<f32> {
    if norm.y > 0.5 {
        let corner = cap_corner_for_vertex(vertex_index);
        let corner_world = corner_world_coordinate(local_grid_x, local_grid_y, corner.x, corner.y);
        let planar_offset = corner_planar_offset(corner_world);
        let height = layer_corner_height(local_grid_x, local_grid_y, grid_width, grid_height, elevation_scale, layer, target_mat, corner.x, corner.y, y_top);

        return vec3<f32>(corner.x + planar_offset.x, height, corner.y + planar_offset.y);
    }

    let corner = wall_corner_for_vertex(vertex_index, norm);
    let corner_world = corner_world_coordinate(local_grid_x, local_grid_y, corner.x, corner.y);
    let planar_offset = corner_planar_offset(corner_world);

    var height = y_bottom;
    if corner.z > 0.5 {
        height = max(y_bottom, layer_corner_height(local_grid_x, local_grid_y, grid_width, grid_height, elevation_scale, layer, target_mat, corner.x, corner.y, y_top));
    }

    return vec3<f32>(corner.x + planar_offset.x, height, corner.y + planar_offset.y);
}

struct VertexInput {
    @location(0) _position: vec3<f32>,
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) normal: vec3<f32>,
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

    let cell_index = in.vertex_index / VERTS_PER_CELL;
    let index_within_cell = in.vertex_index % VERTS_PER_CELL;
    let face_slot = index_within_cell / VERTS_PER_FACE;

    // Isolate the vertex exactly to its physical coordinate within the 64x64 chunk bounds
    let local_cell_x = i32(cell_index) % 64;
    let local_cell_y = i32(cell_index) / 64;

    let true_world_x = chunk_origin_x + local_cell_x;
    let true_world_y = chunk_origin_y + local_cell_y;

    // Translate global coordinates into relative SSBO window bounds
    let local_grid_x = true_world_x - i32(window.origin_size.x);
    let local_grid_y = true_world_y - i32(window.origin_size.y);

    let wrapped_x = ((local_grid_x + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((local_grid_y + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    let mat_terrain = word_0 & 0xFu;
    let mat_surface = (word_0 >> 4u) & 0xFu;
    let mat_granular = (word_0 >> 8u) & 0x7u;
    let mat_fluid = (word_0 >> 11u) & 0xFu;
    let variants = (word_0 >> 15u) & 0x1Fu;
    let elevation = f32((word_0 >> 20u) & 0xFFFu);

    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0x1FFu);
    let surface_state = f32((word_1 >> 18u) & 0x3Fu);

    let t_height = elevation * scale;
    let g_height = t_height + (granular_vol * scale);
    let f_height = g_height + (fluid_vol * scale);
    let s_height = select(f_height, f_height + max(1.0, surface_state) * scale, mat_surface != 0u);

    var local_pos = vec3<f32>(0.0, 0.0, 0.0);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
    var mat_lookup = mat_terrain;
    var target_mat = 0u;
    var is_rendered = false;

    var face_bottom = 0.0;
    var face_top = 0.0;
    var face_layer = LAYER_TERRAIN;
    var n_dx: i32 = 0;
    var n_dy: i32 = 0;

    let vertex_within_face = index_within_cell % VERTS_PER_FACE;

    if face_slot == 1u || face_slot == 6u || face_slot == 11u || face_slot == 16u { normal = vec3<f32>(0.0, 0.0, -1.0); n_dy = 1; }
    if face_slot == 2u || face_slot == 7u || face_slot == 12u || face_slot == 17u { normal = vec3<f32>(0.0, 0.0, 1.0); n_dy = -1; }
    if face_slot == 3u || face_slot == 8u || face_slot == 13u || face_slot == 18u { normal = vec3<f32>(1.0, 0.0, 0.0); n_dx = 1; }
    if face_slot == 4u || face_slot == 9u || face_slot == 14u || face_slot == 19u { normal = vec3<f32>(-1.0, 0.0, 0.0); n_dx = -1; }

    let neighbor_sample = sample_layers_at(local_grid_x + n_dx, local_grid_y + n_dy, grid_width, grid_height, scale);
    let neighbor_h = neighbor_sample.heights;

    switch face_slot {
        // --- TERRAIN ---
        case 0u: {
            mat_lookup = mat_terrain;
            target_mat = mat_terrain;
            face_top = t_height;
            face_bottom = t_height;
            face_layer = LAYER_TERRAIN;
            if mat_terrain != 0u && t_height > 0.0 {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }
        case 1u, 2u, 3u, 4u: {
            mat_lookup = mat_terrain;
            target_mat = mat_terrain;
            face_bottom = neighbor_h.x;
            face_top = t_height;
            face_layer = LAYER_TERRAIN;
            if mat_terrain != 0u && t_height > neighbor_h.x {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }

        // --- GRANULAR ---
        case 5u: {
            mat_lookup = mat_granular + 32u;
            target_mat = mat_granular;
            face_top = g_height;
            face_bottom = g_height;
            face_layer = LAYER_GRANULAR;
            if mat_granular != 0u && granular_vol > 0.0 {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }
        case 6u, 7u, 8u, 9u: {
            mat_lookup = mat_granular + 32u;
            target_mat = mat_granular;
            let n_floor = max(neighbor_h.y, t_height);
            face_bottom = n_floor;
            face_top = g_height;
            face_layer = LAYER_GRANULAR;
            if mat_granular != 0u && g_height > n_floor {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }

        // --- FLUID ---
        case 10u: {
            mat_lookup = mat_fluid + 64u;
            target_mat = mat_fluid;
            face_top = f_height;
            face_bottom = f_height;
            face_layer = LAYER_FLUID;
            if mat_fluid != 0u && fluid_vol > 0.0 {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }
        case 11u, 12u, 13u, 14u: {
            mat_lookup = mat_fluid + 64u;
            target_mat = mat_fluid;
            let n_floor = max(neighbor_h.z, g_height);
            face_bottom = n_floor;
            face_top = f_height;
            face_layer = LAYER_FLUID;
            if mat_fluid != 0u && f_height > n_floor {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }

        // --- SURFACE ---
        case 15u: {
            mat_lookup = mat_surface + 96u;
            target_mat = mat_surface;
            face_top = s_height;
            face_bottom = s_height;
            face_layer = LAYER_SURFACE;
            if mat_surface != 0u {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }
        case 16u, 17u, 18u, 19u: {
            mat_lookup = mat_surface + 96u;
            target_mat = mat_surface;
            let n_floor = max(neighbor_h.w, f_height);
            face_bottom = n_floor;
            face_top = s_height;
            face_layer = LAYER_SURFACE;
            if mat_surface != 0u && s_height > n_floor {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, local_grid_x, local_grid_y, grid_width, grid_height, scale, face_layer, target_mat);
                is_rendered = true;
            }
        }
        default: {}
    }

    if !is_rendered {
        out.clip_position = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        return out;
    }

    // Inject local mesh constraints relative to the chunk origin
    local_pos = local_pos + vec3<f32>(f32(local_cell_x), 0.0, f32(-local_cell_y));
    let world_coord = get_world_from_local(in.instance_index) * vec4<f32>(local_pos, 1.0);

    out.clip_position = mesh_position_local_to_clip(chunk_matrix, vec4<f32>(local_pos, 1.0));
    out.world_position = world_coord.xyz;
    // Pass the absolute geometric normal to circumvent camera-winding inversions
    out.normal = normal;

    var base_color = palette[mat_lookup];
    let visual_shift = (f32(variants) - 16.0) / 16.0 * 0.10;

    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    // Lock the noise to the structural face ID, preventing vertex interpolation static
    let material_mottle = (hash11(vec2<f32>(f32(true_world_x), f32(true_world_y)) * 3.17 + vec2<f32>(f32(face_slot), 1.0)) - 0.5) * 0.08;
    base_color.r = saturate(base_color.r + material_mottle);
    base_color.g = saturate(base_color.g + material_mottle);
    base_color.b = saturate(base_color.b + material_mottle);

    let dx = f32(true_world_x) - window.head_cursor.z;
    let dy = f32(true_world_y) - window.head_cursor.w;
    let dist_sq = dx * dx + dy * dy;

    if window.config.y >= 0.0 && dist_sq <= (window.config.y * window.config.y) + 0.1 {
        base_color = mix(base_color, vec4<f32>(1.0, 0.4, 0.4, 1.0), 0.35);
    }

    out.color = base_color;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var final_normal = in.normal;

    // Isolate procedural normal derivation strictly to horizontal caps.
    if in.normal.y > 0.5 {
        let dx = dpdx(in.world_position);
        let dy = dpdy(in.world_position);
        let p_norm = normalize(cross(dx, dy));

        // Prevent screen-space winding inversions from burying the cap in shadow
        final_normal = select(-p_norm, p_norm, p_norm.y > 0.0);
    }

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let ambient = select(0.3, 0.15, final_normal.y < 0.5);
    let diffuse = max(dot(final_normal, light_dir), ambient);

    let depth_ao = smoothstep(-10.0, 50.0, in.world_position.y);
    let occluded_color = in.color.rgb * mix(0.65, 1.0, depth_ao);

    let final_color = occluded_color * diffuse;
    return vec4<f32>(final_color, in.color.a);
}
