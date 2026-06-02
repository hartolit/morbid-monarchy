#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct WorldWindow {
    origin_size: vec4<f32>, // x: origin.x, y: origin.y, z: size.x, w: size.y
    head_cursor: vec4<f32>, // x: head.x, y: head.y, z: cursor.x, w: cursor.y
    config: vec4<f32>,      // x: elev_scale, y: cursor_radius, z: visual_roughness, w: corner_warp
}

struct LayerSample {
    heights: vec4<f32>, // x=Terrain, y=Granular, z=Fluid, w=Surface
    present: vec4<u32>, // x=Terrain, y=Granular, z=Fluid, w=Surface
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

const VERTS_PER_FACE: u32 = 6u;
const VERTS_PER_CELL: u32 = 120u; // 20 faces * 6 verts

const LAYER_TERRAIN: u32 = 0u;
const LAYER_GRANULAR: u32 = 1u;
const LAYER_FLUID: u32 = 2u;
const LAYER_SURFACE: u32 = 3u;

// Stable procedural noise. The constants are intentionally centralized here so
// terrain liveliness remains deterministic under window scrolling.
fn hash11(position: vec2<f32>) -> f32 {
    return fract(sin(dot(position, vec2<f32>(127.1, 311.7))) * 43758.5453123);
}

fn hash22(position: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        hash11(position + vec2<f32>(19.19, 47.77)),
        hash11(position + vec2<f32>(83.13, 11.71))
    );
}

fn sample_layers_at(
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32
) -> LayerSample {
    var sample: LayerSample;

    if cell_x < 0 || cell_x >= grid_width || cell_y < 0 || cell_y >= grid_height {
        sample.heights = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        sample.present = vec4<u32>(0u, 0u, 0u, 0u);
        return sample;
    }

    let wrapped_x = ((cell_x + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;

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

    return sample;
}

// Returns absolute heights: x=Terrain, y=Granular, z=Fluid, w=Surface
fn calculate_heights_at(
    cell_x: i32, cell_y: i32,
    grid_width: i32, grid_height: i32,
    elevation_scale: f32
) -> vec4<f32> {
    let sample = sample_layers_at(cell_x, cell_y, grid_width, grid_height, elevation_scale);
    return sample.heights;
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

fn layer_roughness(layer: u32) -> f32 {
    let roughness = max(window.config.z, 0.0);
    if layer == LAYER_FLUID { return roughness * 0.08; }
    if layer == LAYER_GRANULAR { return roughness * 0.35; }
    if layer == LAYER_SURFACE { return roughness * 0.55; }
    return roughness;
}

fn corner_world_coordinate(cell_x: i32, cell_y: i32, corner_x: f32, corner_z: f32) -> vec2<f32> {
    var world_y = window.origin_size.y + f32(cell_y);
    if corner_z < 0.5 {
        world_y = world_y + 1.0;
    }

    return vec2<f32>(window.origin_size.x + f32(cell_x) + corner_x, world_y);
}

fn corner_planar_offset(corner_world: vec2<f32>) -> vec2<f32> {
    let warp_strength = max(window.config.w, 0.0);
    let jitter = hash22(corner_world) - vec2<f32>(0.5, 0.5);
    return jitter * (warp_strength * 2.0);
}

fn corner_layer_height_pair(
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32
) -> vec2<f32> {
    let sample = sample_layers_at(cell_x, cell_y, grid_width, grid_height, elevation_scale);
    if layer_is_present(sample, layer) {
        return vec2<f32>(layer_height(sample, layer), 1.0);
    }

    return vec2<f32>(0.0, 0.0);
}

fn layer_corner_height(
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32,
    corner_x: f32,
    corner_z: f32,
    fallback_height: f32
) -> f32 {
    var edge_dx: i32 = -1;
    if corner_x > 0.5 {
        edge_dx = 1;
    }

    var edge_dy: i32 = 1;
    if corner_z > 0.5 {
        edge_dy = -1;
    }

    var total = vec2<f32>(0.0, 0.0);
    total = total + corner_layer_height_pair(cell_x, cell_y, grid_width, grid_height, elevation_scale, layer);
    total = total + corner_layer_height_pair(cell_x + edge_dx, cell_y, grid_width, grid_height, elevation_scale, layer);
    total = total + corner_layer_height_pair(cell_x, cell_y + edge_dy, grid_width, grid_height, elevation_scale, layer);
    total = total + corner_layer_height_pair(cell_x + edge_dx, cell_y + edge_dy, grid_width, grid_height, elevation_scale, layer);

    if total.y < 0.5 {
        return fallback_height;
    }

    var height = total.x / total.y;
    let corner_world = corner_world_coordinate(cell_x, cell_y, corner_x, corner_z);
    let vertical_noise = (hash11(corner_world + vec2<f32>(f32(layer) * 23.17, 91.7)) - 0.5) * 2.0 * layer_roughness(layer);
    height = max(0.0, height + vertical_noise);

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

// Returns x, z, is_top for a side-wall vertex while preserving the original winding.
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
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32
) -> vec3<f32> {
    if norm.y > 0.5 {
        let corner = cap_corner_for_vertex(vertex_index);
        let corner_world = corner_world_coordinate(cell_x, cell_y, corner.x, corner.y);
        let planar_offset = corner_planar_offset(corner_world);
        let height = layer_corner_height(
            cell_x,
            cell_y,
            grid_width,
            grid_height,
            elevation_scale,
            layer,
            corner.x,
            corner.y,
            y_top
        );

        return vec3<f32>(corner.x + planar_offset.x, height, corner.y + planar_offset.y);
    }

    let corner = wall_corner_for_vertex(vertex_index, norm);
    let corner_world = corner_world_coordinate(cell_x, cell_y, corner.x, corner.y);
    let planar_offset = corner_planar_offset(corner_world);

    var height = y_bottom;
    if corner.z > 0.5 {
        height = max(
            y_bottom,
            layer_corner_height(
                cell_x,
                cell_y,
                grid_width,
                grid_height,
                elevation_scale,
                layer,
                corner.x,
                corner.y,
                y_top
            )
        );
    }

    return vec3<f32>(corner.x + planar_offset.x, height, corner.y + planar_offset.y);
}

fn lively_quad_normal(
    vertex_index: u32,
    y_bottom: f32,
    y_top: f32,
    norm: vec3<f32>,
    cell_x: i32,
    cell_y: i32,
    grid_width: i32,
    grid_height: i32,
    elevation_scale: f32,
    layer: u32
) -> vec3<f32> {
    let triangle_start = select(0u, 3u, vertex_index >= 3u);
    let a = lively_quad_vertex(triangle_start, y_bottom, y_top, norm, cell_x, cell_y, grid_width, grid_height, elevation_scale, layer);
    let b = lively_quad_vertex(triangle_start + 1u, y_bottom, y_top, norm, cell_x, cell_y, grid_width, grid_height, elevation_scale, layer);
    let c = lively_quad_vertex(triangle_start + 2u, y_bottom, y_top, norm, cell_x, cell_y, grid_width, grid_height, elevation_scale, layer);

    let face_normal = cross(b - a, c - a);
    if dot(face_normal, face_normal) < 0.000001 {
        return norm;
    }

    return normalize(face_normal);
}

struct VertexInput {
    @location(0) _position: vec3<f32>,
    @builtin(vertex_index) vertex_index: u32,
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

    let cell_index = in.vertex_index / VERTS_PER_CELL;
    let index_within_cell = in.vertex_index % VERTS_PER_CELL;
    let face_slot = index_within_cell / VERTS_PER_FACE;
    let vertex_within_face = index_within_cell % VERTS_PER_FACE;

    let cell_x = i32(cell_index) % grid_width;
    let cell_y = i32(cell_index) / grid_width;

    let wrapped_x = ((cell_x + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cell_y + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;
    let buffer_index = u32(wrapped_y * grid_width + wrapped_x) * 2u;

    let word_0 = world_buffer[buffer_index];
    let word_1 = world_buffer[buffer_index + 1u];

    // Decode Word 0 (Geometry)
    let mat_terrain = word_0 & 0xFu;
    let mat_surface = (word_0 >> 4u) & 0xFu;
    let mat_granular = (word_0 >> 8u) & 0x7u;
    let mat_fluid = (word_0 >> 11u) & 0xFu;
    let variants = (word_0 >> 15u) & 0x1Fu;
    let elevation = f32((word_0 >> 20u) & 0xFFFu);

    // Decode Word 1 (Physics)
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0x1FFu);
    let surface_state = f32((word_1 >> 18u) & 0x3Fu);

    // Absolute Stacked Heights
    let t_height = elevation * scale;
    let g_height = t_height + (granular_vol * scale);
    let f_height = g_height + (fluid_vol * scale);
    let s_height = f_height + max(1.0, surface_state) * scale;

    let world_offset_x = f32(cell_x);
    let world_offset_z = f32(grid_height - 1 - cell_y);

    var local_pos = vec3<f32>(0.0, 0.0, 0.0);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
    var mat_lookup = mat_terrain;
    var is_rendered = false;
    var face_bottom = 0.0;
    var face_top = 0.0;
    var face_layer = LAYER_TERRAIN;

    // Offsets to neighbor cells for boundary occlusion
    var n_dx: i32 = 0;
    var n_dy: i32 = 0;

    // Setup generic wall faces mapped to all 4 physical layers
    if face_slot == 1u || face_slot == 6u || face_slot == 11u || face_slot == 16u { normal = vec3<f32>(0.0, 0.0, -1.0); n_dy = 1; }
    if face_slot == 2u || face_slot == 7u || face_slot == 12u || face_slot == 17u { normal = vec3<f32>(0.0, 0.0, 1.0); n_dy = -1; }
    if face_slot == 3u || face_slot == 8u || face_slot == 13u || face_slot == 18u { normal = vec3<f32>(1.0, 0.0, 0.0); n_dx = 1; }
    if face_slot == 4u || face_slot == 9u || face_slot == 14u || face_slot == 19u { normal = vec3<f32>(-1.0, 0.0, 0.0); n_dx = -1; }

    let neighbor_sample = sample_layers_at(cell_x + n_dx, cell_y + n_dy, grid_width, grid_height, scale);
    let neighbor_h = neighbor_sample.heights;

    switch face_slot {
        // --- TERRAIN FACES (0-4) ---
        case 0u: { // Cap
            mat_lookup = mat_terrain;
            face_top = t_height;
            face_bottom = t_height;
            face_layer = LAYER_TERRAIN;
            normal = vec3<f32>(0.0, 1.0, 0.0);
            if mat_terrain != 0u && t_height > 0.0 {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }
        case 1u, 2u, 3u, 4u: { // Boundary skirts
            mat_lookup = mat_terrain;
            face_bottom = neighbor_h.x;
            face_top = t_height;
            face_layer = LAYER_TERRAIN;
            if mat_terrain != 0u && t_height > neighbor_h.x && !layer_is_present(neighbor_sample, LAYER_TERRAIN) {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }

        // --- GRANULAR FACES (5-9) ---
        case 5u: { // Cap
            mat_lookup = mat_granular + 32u;
            face_top = g_height;
            face_bottom = g_height;
            face_layer = LAYER_GRANULAR;
            normal = vec3<f32>(0.0, 1.0, 0.0);
            if mat_granular != 0u && granular_vol > 0.0 {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }
        case 6u, 7u, 8u, 9u: { // Boundary skirts
            mat_lookup = mat_granular + 32u;
            let n_floor = max(neighbor_h.y, t_height);
            face_bottom = n_floor;
            face_top = g_height;
            face_layer = LAYER_GRANULAR;
            if mat_granular != 0u && g_height > n_floor && !layer_is_present(neighbor_sample, LAYER_GRANULAR) {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }

        // --- FLUID FACES (10-14) ---
        case 10u: { // Cap
            mat_lookup = mat_fluid + 64u;
            face_top = f_height;
            face_bottom = f_height;
            face_layer = LAYER_FLUID;
            normal = vec3<f32>(0.0, 1.0, 0.0);
            if mat_fluid != 0u && fluid_vol > 0.0 {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }
        case 11u, 12u, 13u, 14u: { // Boundary skirts
            mat_lookup = mat_fluid + 64u;
            let n_floor = max(neighbor_h.z, g_height);
            face_bottom = n_floor;
            face_top = f_height;
            face_layer = LAYER_FLUID;
            if mat_fluid != 0u && f_height > n_floor && !layer_is_present(neighbor_sample, LAYER_FLUID) {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }

        // --- SURFACE FACES (15-19) ---
        case 15u: { // Cap
            mat_lookup = mat_surface + 96u;
            face_top = s_height;
            face_bottom = s_height;
            face_layer = LAYER_SURFACE;
            normal = vec3<f32>(0.0, 1.0, 0.0);
            if mat_surface != 0u {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }
        case 16u, 17u, 18u, 19u: { // Boundary skirts
            mat_lookup = mat_surface + 96u;
            let n_floor = max(neighbor_h.w, f_height);
            face_bottom = n_floor;
            face_top = s_height;
            face_layer = LAYER_SURFACE;
            if mat_surface != 0u && s_height > n_floor && !layer_is_present(neighbor_sample, LAYER_SURFACE) {
                local_pos = lively_quad_vertex(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
                is_rendered = true;
            }
        }
        default: {}
    }

    // Discards the triangle without triggering float-clipping explosions.
    if !is_rendered {
        out.clip_position = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        out.color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        out.normal = vec3<f32>(0.0, 1.0, 0.0);
        return out;
    }

    normal = lively_quad_normal(vertex_within_face, face_bottom, face_top, normal, cell_x, cell_y, grid_width, grid_height, scale, face_layer);
    local_pos = local_pos + vec3<f32>(world_offset_x, 0.0, world_offset_z);

    out.normal = normal;
    out.clip_position = mesh_position_local_to_clip(get_world_from_local(0u), vec4<f32>(local_pos, 1.0));

    var base_color = palette[mat_lookup];
    let visual_shift = (f32(variants) - 16.0) / 16.0 * 0.10;

    base_color.r = saturate(base_color.r + visual_shift);
    base_color.g = saturate(base_color.g + visual_shift);
    base_color.b = saturate(base_color.b + visual_shift);

    // Reconstruct the absolute spatial coordinate of the rendered vertex to test bounds
    let true_world_x = f32(cell_x) + window.origin_size.x;
    let true_world_y = f32(cell_y) + window.origin_size.y;

    let material_mottle = (hash11(vec2<f32>(true_world_x, true_world_y) * 3.17 + vec2<f32>(f32(face_slot), f32(vertex_within_face) * 5.0)) - 0.5) * 0.08;
    base_color.r = saturate(base_color.r + material_mottle);
    base_color.g = saturate(base_color.g + material_mottle);
    base_color.b = saturate(base_color.b + material_mottle);

    let dx = true_world_x - window.head_cursor.z;
    let dy = true_world_y - window.head_cursor.w;
    let dist_sq = dx * dx + dy * dy;

    // Radius boundary test. Sub-zero radii cleanly bypass the mutation.
    if window.config.y >= 0.0 && dist_sq <= (window.config.y * window.config.y) + 0.1 {
        // Clinically apply a localized color mix against the base thermodynamic rendering
        base_color = mix(base_color, vec4<f32>(1.0, 0.4, 0.4, 1.0), 0.35);
    }

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let ambient = select(0.3, 0.15, normal.y < 0.5);
    let diffuse = max(dot(normal, light_dir), ambient);

    out.color = vec4<f32>(base_color.rgb * diffuse, base_color.a);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
