#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}
#import bevy_pbr::mesh_view_bindings::{view, globals, lights}
#import bevy_pbr::mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT
#import bevy_pbr::shadows::fetch_directional_shadow
#import bevy_pbr::view_transformations::position_world_to_view

#ifdef DISTANCE_FOG
#import bevy_pbr::mesh_view_bindings::fog
#import bevy_pbr::mesh_view_types::{FOG_MODE_LINEAR, FOG_MODE_EXPONENTIAL, FOG_MODE_EXPONENTIAL_SQUARED, FOG_MODE_ATMOSPHERIC}
#import bevy_pbr::fog::{linear_fog, exponential_fog, exponential_squared_fog, atmospheric_fog}
#endif

// Must match `WorldWindowUniform` in render.rs.
struct WorldWindow {
    origin_size: vec4<f32>,   // xy: window origin (cells), zw: window size (cells)
    head_cursor: vec4<f32>,   // xy: toroidal buffer head, zw: brush cursor cell
    config: vec4<f32>,        // x: elevation scale, y: cursor radius (<0 hides), z: terrain shadow strength, w: occlusion strength
    sun_direction: vec4<f32>, // xyz: unit vector towards the sun, w: sun illuminance (lux)
    sun_color: vec4<f32>,     // rgb: linear sun colour, w: ambient sky brightness
    sky_color: vec4<f32>,     // rgb: linear zenith colour
    ground_color: vec4<f32>,  // rgb: linear ground bounce colour
}

@group(3) @binding(10) var<storage, read> world_buffer: array<u32>;
@group(3) @binding(11) var<storage, read> palette: array<vec4<f32>>;
@group(3) @binding(12) var<uniform> window: WorldWindow;

const LAYER_TERRAIN: u32 = 0u;
const LAYER_GRANULAR: u32 = 1u;
const LAYER_FLUID: u32 = 2u;
const LAYER_SURFACE: u32 = 3u;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;
const INV_PI: f32 = 0.31830989;

// Fragments whose interpolated layer coverage falls below this lie outside the layer.
const COVERAGE_DISCARD_THRESHOLD: f32 = 0.05;
// An empty layer is tucked this far below its column's floor so the floor hides it.
const HIDDEN_LAYER_DEPTH: f32 = 0.5;
// Settled liquid can sit up to one transfer unit above a dry neighbour without flowing into
// it. Extending the surface flat over such cells (in elevation units) keeps shorelines level
// instead of dipping into the bank.
const FLUID_EDGE_TOLERANCE_UNITS: f32 = 2.0;
// Per-cell brightness jitter: variants are centred on 16 and step by this much.
const VARIANT_NEUTRAL: f32 = 16.0;
const VARIANT_BRIGHTNESS_STEP: f32 = 0.00625;
const VARIANT_MAX: f32 = 31.0;

// Heightfield sun shadows are marched in cell space with steps that lengthen as they go,
// covering roughly 120 cells in SHADOW_STEPS fetches.
const SHADOW_STEPS: u32 = 24u;
const SHADOW_FIRST_STEP: f32 = 1.0;
const SHADOW_STEP_GROWTH: f32 = 0.35;
// Lifts the ray start so a flat surface does not shadow itself through rounding.
const SHADOW_BIAS: f32 = 0.25;
// Higher values give harder penumbrae (clearance divided by distance travelled).
const SHADOW_PENUMBRA_SHARPNESS: f32 = 6.0;
const MIN_SUN_HORIZONTAL: f32 = 0.0001;

// Ambient occlusion compares the surface against terrain on two rings of eight samples.
const AO_RING_INNER: f32 = 2.0;
const AO_RING_OUTER: f32 = 6.0;
const AO_SAMPLE_COUNT: f32 = 16.0;

// Young foliage is paler and yellower than mature growth.
const FOLIAGE_YOUNG_TINT: vec3<f32> = vec3<f32>(1.45, 1.3, 0.55);

// Flames sway and flicker; both are driven by time and a per-cell phase.
const FLAME_SWAY_UNITS: f32 = 1.5;
const FLAME_SWAY_FREQUENCY: f32 = 7.0;
const FLICKER_FREQUENCY: f32 = 11.0;
const FLICKER_SECONDARY_FREQUENCY: f32 = 23.0;
const FLICKER_DEPTH: f32 = 0.35;

// Brush cursor overlay, in post-lighting output units.
const CURSOR_COLOR: vec3<f32> = vec3<f32>(1.0, 0.4, 0.4);
const CURSOR_FILL_OPACITY: f32 = 0.15;
const CURSOR_RING_OPACITY: f32 = 0.9;
const CURSOR_RING_HALF_WIDTH: f32 = 0.18;
const CURSOR_RING_OFFSET: f32 = 0.5;

const HASH_DOT: vec2<f32> = vec2<f32>(127.1, 311.7);
const HASH_SCALE: f32 = 43758.5453;

const N_OFFSETS: array<vec2<i32>, 8> = array<vec2<i32>, 8>(
    vec2<i32>(1, 0),  vec2<i32>(-1, 0), vec2<i32>(0, 1),  vec2<i32>(0, -1),
    vec2<i32>(1, 1),  vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1)
);

struct CellData {
    terrain_mat: u32,
    granular_mat: u32,
    fluid_mat: u32,
    surface_mat: u32,
    variants: u32,
    surface_state: f32,
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
    @location(0) albedo: vec4<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    // x: specular, y: shininess, z: emission, w: optical density (see `MaterialLook`)
    @location(3) material_props: vec4<f32>,
    // x: ambient occlusion, y: heightfield sun visibility, z: liquid depth, w: layer coverage
    @location(4) lighting_terms: vec4<f32>,
    // rgb: albedo of the liquid covering this surface, a: optical depth of that liquid
    @location(5) submersion: vec4<f32>,
    // x: per-cell random phase in [0, 1]
    @location(6) animation: vec4<f32>,
};

struct LayerSample {
    elevation: f32,
    mat_lookup: u32,
    has_mat: bool,
    is_hidden: bool,
}

fn buffer_index(cell_x: i32, cell_y: i32, grid_width: i32, grid_height: i32) -> u32 {
    let cx = clamp(cell_x, 0, grid_width - 1);
    let cy = clamp(cell_y, 0, grid_height - 1);

    let wrapped_x = ((cx + i32(window.head_cursor.x)) % grid_width + grid_width) % grid_width;
    let wrapped_y = ((cy + i32(window.head_cursor.y)) % grid_height + grid_height) % grid_height;

    return u32(wrapped_y * grid_width + wrapped_x) * 2u;
}

fn get_cell_data(
    cell_x: i32, cell_y: i32, grid_width: i32, grid_height: i32, scale: f32
) -> CellData {
    var data: CellData;
    let index = buffer_index(cell_x, cell_y, grid_width, grid_height);
    let word_0 = world_buffer[index];
    let word_1 = world_buffer[index + 1u];

    data.terrain_mat = word_0 & 0xFu;
    data.surface_mat = (word_0 >> 4u) & 0xFu;
    data.granular_mat = (word_0 >> 8u) & 0xFu;
    data.fluid_mat = (word_0 >> 12u) & 0x7u;
    data.variants = (word_0 >> 15u) & 0x1Fu;

    let elevation = f32((word_0 >> 20u) & 0xFFFu);
    let fluid_vol = f32(word_1 & 0x1FFu);
    let granular_vol = f32((word_1 >> 9u) & 0xFu);
    data.surface_state = f32((word_1 >> 13u) & 0x1FFu);

    data.terrain_h = elevation * scale;
    data.granular_h = data.terrain_h + (granular_vol * scale);
    data.fluid_h = data.granular_h + (fluid_vol * scale);
    data.surface_h = data.granular_h + max(1.0, data.surface_state) * scale;

    return data;
}

// Height of the solid column (bedrock, loose material and ice); the only thing that casts
// heightfield shadows and occlusion. Mirrors `WorldCell::solid_top`.
fn fetch_solid_height(cell_x: i32, cell_y: i32, grid_width: i32, grid_height: i32, scale: f32) -> f32 {
    let index = buffer_index(cell_x, cell_y, grid_width, grid_height);
    let word_0 = world_buffer[index];
    let word_1 = world_buffer[index + 1u];
    let elevation = f32((word_0 >> 20u) & 0xFFFu);
    let granular_vol = f32((word_1 >> 9u) & 0xFu);
    let is_ice = ((word_0 >> 12u) & 0x7u) == #{FLUID_MAT_ICE}u;
    let frozen_vol = select(0.0, f32(word_1 & 0x1FFu), is_ice);
    return (elevation + granular_vol + frozen_vol) * scale;
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

// Places a layer vertex. Columns without the layer's material either borrow a neighbour's
// height to close the layer's edge, or hide just under their own floor.
fn sample_layer(
    center: CellData, cell_x: i32, cell_y: i32, grid_w: i32, grid_h: i32, scale: f32, layer_idx: u32
) -> LayerSample {
    var sample: LayerSample;
    sample.is_hidden = false;

    if layer_idx == LAYER_TERRAIN {
        sample.elevation = center.terrain_h;
        sample.mat_lookup = center.terrain_mat;
        sample.has_mat = true;
        return sample;
    }

    let active_mat = extract_layer_mat(center, layer_idx);
    sample.has_mat = active_mat != 0u;

    if sample.has_mat {
        sample.elevation = extract_layer_height(center, layer_idx);
        sample.mat_lookup = active_mat;
        return sample;
    }

    let floor_h = max(center.terrain_h, select(0.0, center.granular_h, center.granular_mat != 0u));

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
            max_n_h = max(max_n_h, extract_layer_height(n_data, layer_idx));
        }
    }

    sample.mat_lookup = fallback_mat;
    let edge_tolerance = select(0.0, FLUID_EDGE_TOLERANCE_UNITS * scale, layer_idx == LAYER_FLUID);
    if max_n_h > 0.0 && floor_h + edge_tolerance >= max_n_h {
        sample.elevation = max_n_h;
    } else {
        sample.elevation = floor_h - HIDDEN_LAYER_DEPTH;
        sample.is_hidden = true;
    }

    return sample;
}

// Central differences of the layer's height field. Grid +y maps to world -Z.
fn layer_normal(cell_x: i32, cell_y: i32, grid_w: i32, grid_h: i32, scale: f32, layer_idx: u32) -> vec3<f32> {
    let h_left = extract_layer_height(get_cell_data(cell_x - 1, cell_y, grid_w, grid_h, scale), layer_idx);
    let h_right = extract_layer_height(get_cell_data(cell_x + 1, cell_y, grid_w, grid_h, scale), layer_idx);
    let h_down = extract_layer_height(get_cell_data(cell_x, cell_y - 1, grid_w, grid_h, scale), layer_idx);
    let h_up = extract_layer_height(get_cell_data(cell_x, cell_y + 1, grid_w, grid_h, scale), layer_idx);
    return normalize(vec3<f32>(h_left - h_right, 2.0, h_up - h_down));
}

// Fraction of the sky hemisphere left open, estimated from how far surrounding terrain
// rises above the surface relative to its horizontal distance.
fn heightfield_occlusion(cell_x: i32, cell_y: i32, grid_w: i32, grid_h: i32, scale: f32, surface_h: f32) -> f32 {
    let strength = window.config.w;
    if strength <= 0.0 {
        return 1.0;
    }

    var occlusion = 0.0;
    for (var i = 0; i < 8; i++) {
        let direction = N_OFFSETS[i];
        let step_length = length(vec2<f32>(direction));
        for (var ring = 0; ring < 2; ring++) {
            let radius = select(AO_RING_OUTER, AO_RING_INNER, ring == 0);
            let offset = vec2<i32>(vec2<f32>(direction) * radius);
            let sample_h = fetch_solid_height(cell_x + offset.x, cell_y + offset.y, grid_w, grid_h, scale);
            occlusion += clamp((sample_h - surface_h) / (radius * step_length), 0.0, 1.0);
        }
    }

    return clamp(1.0 - strength * occlusion / AO_SAMPLE_COUNT, 0.0, 1.0);
}

// Soft sun visibility from marching the height field towards the sun. Tracks the smallest
// clearance-over-distance ratio along the ray, which widens the penumbra with distance.
fn heightfield_sun_visibility(cell_x: i32, cell_y: i32, grid_w: i32, grid_h: i32, scale: f32, start_h: f32) -> f32 {
    let strength = window.config.z;
    let sun = window.sun_direction.xyz;
    let grid_direction = vec2<f32>(sun.x, -sun.z);
    let horizontal = length(grid_direction);
    if strength <= 0.0 || sun.y <= 0.0 || horizontal < MIN_SUN_HORIZONTAL {
        return 1.0;
    }

    let step_direction = grid_direction / horizontal;
    let rise_per_cell = sun.y / horizontal;
    let origin = vec2<f32>(f32(cell_x), f32(cell_y));

    var visibility = 1.0;
    var travelled = 0.0;
    var step_length = SHADOW_FIRST_STEP;
    for (var i = 0u; i < SHADOW_STEPS; i++) {
        travelled += step_length;
        step_length += SHADOW_STEP_GROWTH;

        let probe = vec2<i32>(round(origin + step_direction * travelled));
        if probe.x < 0 || probe.y < 0 || probe.x >= grid_w || probe.y >= grid_h {
            break;
        }

        let ray_h = start_h + SHADOW_BIAS + rise_per_cell * travelled;
        let occluder_h = fetch_solid_height(probe.x, probe.y, grid_w, grid_h, scale);
        visibility = min(visibility, SHADOW_PENUMBRA_SHARPNESS * (ray_h - occluder_h) / travelled);
        if visibility <= 0.0 {
            break;
        }
    }

    return mix(1.0, smoothstep(0.0, 1.0, clamp(visibility, 0.0, 1.0)), strength);
}

fn process_landscape_vertex(
    in: VertexInput, instance_index: u32, layer_idx: u32
) -> VertexOutput {
    var out: VertexOutput;

    let grid_width = i32(window.origin_size.z);
    let grid_height = i32(window.origin_size.w);
    let scale = window.config.x;

    let chunk_matrix = get_world_from_local(instance_index);
    let chunk_origin_x = i32(round(chunk_matrix[3].x));
    let chunk_origin_y = i32(round(-chunk_matrix[3].z));

    let local_x = i32(round(in._position.x));
    let local_y = i32(round(-in._position.z));

    let world_cell_x = chunk_origin_x + local_x;
    let world_cell_y = chunk_origin_y + local_y;

    let grid_x = world_cell_x - i32(window.origin_size.x);
    let grid_y = world_cell_y - i32(window.origin_size.y);

    let center = get_cell_data(grid_x, grid_y, grid_width, grid_height, scale);
    let layer_sample = sample_layer(center, grid_x, grid_y, grid_width, grid_height, scale, layer_idx);
    let phase = f32(center.variants) / VARIANT_MAX;
    let is_flame = layer_idx == LAYER_SURFACE && layer_sample.mat_lookup == #{SURFACE_MAT_FIRE}u;

    var height = layer_sample.elevation;
    if is_flame && layer_sample.has_mat {
        height += sin(globals.time * FLAME_SWAY_FREQUENCY + phase * TAU) * FLAME_SWAY_UNITS * scale;
    }

    let local_pos = vec3<f32>(in._position.x, height, in._position.z);
    out.clip_position = mesh_position_local_to_clip(chunk_matrix, vec4<f32>(local_pos, 1.0));
    out.world_pos = (chunk_matrix * vec4<f32>(local_pos, 1.0)).xyz;
    out.world_normal = layer_normal(grid_x, grid_y, grid_width, grid_height, scale, layer_idx);

    // Hidden vertices sit under the floor and are never seen, so skip their expensive terms.
    var ambient_occlusion = 1.0;
    var sun_visibility = 1.0;
    if !layer_sample.is_hidden {
        ambient_occlusion = heightfield_occlusion(grid_x, grid_y, grid_width, grid_height, scale, height);
        sun_visibility = heightfield_sun_visibility(grid_x, grid_y, grid_width, grid_height, scale, height);
    }

    let palette_idx = layer_idx * #{PALETTE_LAYER_STRIDE}u + layer_sample.mat_lookup;
    var albedo = palette[palette_idx];
    out.material_props = palette[palette_idx + #{PALETTE_PROPERTY_OFFSET}u];

    let visual_shift = (f32(center.variants) - VARIANT_NEUTRAL) * VARIANT_BRIGHTNESS_STEP;
    albedo = vec4<f32>(saturate(albedo.rgb * (1.0 + visual_shift * 2.0)), albedo.a);

    if layer_idx == LAYER_SURFACE && layer_sample.mat_lookup == #{SURFACE_MAT_FOLIAGE}u {
        let growth = saturate(center.surface_state / f32(#{FOLIAGE_MATURE_STATE}u));
        albedo = vec4<f32>(mix(saturate(albedo.rgb * FOLIAGE_YOUNG_TINT), albedo.rgb, growth), albedo.a);
    }
    out.albedo = albedo;

    let coverage = select(select(0.0, 1.0, layer_sample.has_mat), 1.0, layer_idx == LAYER_TERRAIN);
    let liquid_depth = select(0.0, max(height - center.granular_h, 0.0), layer_idx == LAYER_FLUID && layer_sample.has_mat);
    out.lighting_terms = vec4<f32>(ambient_occlusion, sun_visibility, liquid_depth, coverage);

    // Solid layers beneath a liquid receive sunlight filtered through it.
    var submersion = vec4<f32>(0.0);
    if layer_idx != LAYER_FLUID && center.fluid_mat != 0u {
        let fluid_slot = LAYER_FLUID * #{PALETTE_LAYER_STRIDE}u + center.fluid_mat;
        let covering_depth = max(center.fluid_h - height, 0.0);
        submersion = vec4<f32>(palette[fluid_slot].rgb, covering_depth * palette[fluid_slot + #{PALETTE_PROPERTY_OFFSET}u].w);
    }
    out.submersion = submersion;
    out.animation = vec4<f32>(phase, 0.0, 0.0, 0.0);

    return out;
}

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, HASH_DOT)) * HASH_SCALE);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let local = fract(p);
    let blend = local * local * (3.0 - 2.0 * local);
    let bottom = mix(hash21(cell), hash21(cell + vec2<f32>(1.0, 0.0)), blend.x);
    let top = mix(hash21(cell + vec2<f32>(0.0, 1.0)), hash21(cell + vec2<f32>(1.0, 1.0)), blend.x);
    return mix(bottom, top, blend.y);
}

// Irregular 0..1 brightness modulation for burning or glowing materials.
fn flame_flicker(phase: f32) -> f32 {
    let t = globals.time;
    let wobble = 0.5 + 0.25 * sin(t * FLICKER_FREQUENCY + phase * TAU)
        + 0.25 * sin(t * FLICKER_SECONDARY_FREQUENCY + phase * TAU * 3.0);
    return 1.0 - FLICKER_DEPTH * wobble;
}

fn sun_facing(normal: vec3<f32>) -> f32 {
    return max(dot(normal, window.sun_direction.xyz), 0.0);
}

fn sun_irradiance() -> vec3<f32> {
    return window.sun_color.rgb * window.sun_direction.w * view.exposure;
}

// Hemispheric sky light arriving along `direction`, blended from ground bounce to zenith.
fn sky_radiance(direction: vec3<f32>) -> vec3<f32> {
    let zenith_weight = clamp(direction.y * 0.5 + 0.5, 0.0, 1.0);
    return mix(window.ground_color.rgb, window.sky_color.rgb, zenith_weight)
        * window.sun_color.w * view.exposure;
}

// Shadow cast by PBR entities (spheres, the observer) from Bevy's directional shadow maps.
// The landscape itself does not render into those maps; its self-shadowing is marched.
fn entity_shadow(world_pos: vec3<f32>, normal: vec3<f32>, frag_coord: vec4<f32>) -> f32 {
    if lights.n_directional_lights == 0u {
        return 1.0;
    }
    if (lights.directional_lights[0].flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u {
        return 1.0;
    }
    let view_z = position_world_to_view(world_pos).z;
    return fetch_directional_shadow(0u, vec4<f32>(world_pos, 1.0), normal, view_z, frag_coord.xy);
}

// Normalised Blinn-Phong highlight; the (n + 8) / 8pi factor keeps reflected energy roughly
// constant as the lobe tightens.
fn specular_lobe(normal: vec3<f32>, view_dir: vec3<f32>, specular: f32, shininess: f32) -> f32 {
    let half_vector = normalize(window.sun_direction.xyz + view_dir);
    let n_dot_h = max(dot(normal, half_vector), 0.0);
    return specular * (shininess + 8.0) / (8.0 * PI) * pow(n_dot_h, shininess);
}

// Lambert diffuse from the sun plus hemispheric sky light, matching the scale of Bevy's PBR
// lighting (illuminance times exposure) so entities and terrain sit in the same exposure.
fn shade_lit_surface(
    albedo: vec3<f32>,
    normal: vec3<f32>,
    view_dir: vec3<f32>,
    specular: f32,
    shininess: f32,
    sun_visibility: f32,
    ambient_occlusion: f32,
) -> vec3<f32> {
    let irradiance = sun_irradiance() * sun_facing(normal) * sun_visibility;
    let direct = (albedo * INV_PI + vec3<f32>(specular_lobe(normal, view_dir, specular, shininess))) * irradiance;
    let ambient = albedo * sky_radiance(normal) * ambient_occlusion;
    return direct + ambient;
}

fn apply_world_fog(color: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
#ifdef DISTANCE_FOG
    let view_to_world = world_pos - view.world_position;
    let distance = length(view_to_world);

    var scattering = vec3<f32>(0.0);
    if fog.directional_light_color.a > 0.0 {
        let alignment = max(dot(view_to_world / max(distance, MIN_SUN_HORIZONTAL), window.sun_direction.xyz), 0.0);
        scattering = pow(alignment, fog.directional_light_exponent) * window.sun_color.rgb * window.sun_direction.w * view.exposure;
    }

    let input = vec4<f32>(color, 1.0);
    if fog.mode == FOG_MODE_LINEAR {
        return linear_fog(fog, input, distance, scattering).rgb;
    } else if fog.mode == FOG_MODE_EXPONENTIAL {
        return exponential_fog(fog, input, distance, scattering).rgb;
    } else if fog.mode == FOG_MODE_EXPONENTIAL_SQUARED {
        return exponential_squared_fog(fog, input, distance, scattering).rgb;
    } else if fog.mode == FOG_MODE_ATMOSPHERIC {
        return atmospheric_fog(fog, input, distance, scattering).rgb;
    }
#endif
    return color;
}

// Opacity of the brush overlay at this fragment: a faint disc plus an anti-aliased ring at
// the brush edge. Uses screen-space derivatives, so call it before any `discard`.
fn brush_cursor_mask(world_pos: vec3<f32>) -> f32 {
    let radius = window.config.y;
    let cell = vec2<f32>(world_pos.x, -world_pos.z);
    let distance = length(cell - window.head_cursor.zw);
    let pixel = max(fwidth(distance), MIN_SUN_HORIZONTAL);

    let ring_radius = radius + CURSOR_RING_OFFSET;
    let fill = 1.0 - smoothstep(ring_radius - pixel, ring_radius + pixel, distance);
    let ring = 1.0 - smoothstep(CURSOR_RING_HALF_WIDTH, CURSOR_RING_HALF_WIDTH + pixel, abs(distance - ring_radius));
    let mask = max(fill * CURSOR_FILL_OPACITY, ring * CURSOR_RING_OPACITY);
    return select(mask, 0.0, radius < 0.0);
}

fn apply_brush_cursor(color: vec3<f32>, mask: f32) -> vec3<f32> {
    return mix(color, CURSOR_COLOR, mask);
}

// Light reaching a surface under a liquid is tinted and dimmed by the column above it.
fn submerged_light_filter(submersion: vec4<f32>) -> vec3<f32> {
    let transmittance = exp(-submersion.a);
    return mix(submersion.rgb, vec3<f32>(1.0), transmittance);
}

// Shared fragment path for the opaque layers (terrain, granular, surface).
fn shade_solid_layer(in: VertexOutput) -> vec4<f32> {
    let cursor_mask = brush_cursor_mask(in.world_pos);
    if in.lighting_terms.w <= COVERAGE_DISCARD_THRESHOLD {
        discard;
    }

    let normal = normalize(in.world_normal);
    let view_dir = normalize(view.world_position - in.world_pos);
    let props = in.material_props;
    let sun_visibility = in.lighting_terms.y * entity_shadow(in.world_pos, normal, in.clip_position);

    var color = shade_lit_surface(in.albedo.rgb, normal, view_dir, props.x, props.y, sun_visibility, in.lighting_terms.x);
    color *= submerged_light_filter(in.submersion);
    color += in.albedo.rgb * props.z * flame_flicker(in.animation.x);

    color = apply_world_fog(color, in.world_pos);
    return vec4<f32>(apply_brush_cursor(color, cursor_mask), 1.0);
}
