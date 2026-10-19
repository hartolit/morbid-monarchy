// Light and atmosphere shared by the terrain layers and the spheres: the sun (Lambert diffuse
// and a normalised Blinn-Phong highlight), hemispheric sky light, heightfield sun shadows and
// ambient occlusion marched over the resident columns, the sun's sphere shadow map, linear
// distance fog with a sun glow, and the brush cursor. Light is in the baseline's units
// (illuminance times exposure), so terrain and spheres share one exposure and one tone map.

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;
const INV_PI: f32 = 0.31830989;
const MIN_SUN_HORIZONTAL: f32 = 0.0001;

// Heightfield sun shadows are marched in cell space with steps that lengthen as they go,
// covering roughly 120 cells in SHADOW_STEPS fetches.
const SHADOW_STEPS: u32 = 24u;
const SHADOW_FIRST_STEP: f32 = 1.0;
const SHADOW_STEP_GROWTH: f32 = 0.35;
// Lifts the ray start so a flat surface does not shadow itself through rounding.
const SHADOW_BIAS: f32 = 0.25;
// Higher values give harder penumbrae (clearance divided by distance travelled).
const SHADOW_PENUMBRA_SHARPNESS: f32 = 6.0;

// Ambient occlusion compares the surface against terrain on two rings of eight samples.
const AO_RING_INNER: f32 = 2.0;
const AO_RING_OUTER: f32 = 6.0;
const AO_SAMPLE_COUNT: f32 = 16.0;

const N_OFFSETS: array<vec2<i32>, 8> = array<vec2<i32>, 8>(
    vec2<i32>(1, 0),  vec2<i32>(-1, 0), vec2<i32>(0, 1),  vec2<i32>(0, -1),
    vec2<i32>(1, 1),  vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1)
);

// Brush cursor overlay, in post-lighting output units.
const CURSOR_COLOR: vec3<f32> = vec3<f32>(1.0, 0.4, 0.4);
const CURSOR_FILL_OPACITY: f32 = 0.15;
const CURSOR_RING_OPACITY: f32 = 0.9;
const CURSOR_RING_HALF_WIDTH: f32 = 0.18;
const CURSOR_RING_OFFSET: f32 = 0.5;

const HASH_DOT: vec2<f32> = vec2<f32>(127.1, 311.7);
const HASH_SCALE: f32 = 43758.5453;

// Flames sway and flicker; both are driven by time and a per-cell phase.
const FLICKER_FREQUENCY: f32 = 11.0;
const FLICKER_SECONDARY_FREQUENCY: f32 = 23.0;
const FLICKER_DEPTH: f32 = 0.35;

fn seconds() -> f32 {
    return frame.window_cells.w;
}

fn exposure() -> f32 {
    return frame.sky_color.w;
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
    let t = seconds();
    let wobble = 0.5 + 0.25 * sin(t * FLICKER_FREQUENCY + phase * TAU)
        + 0.25 * sin(t * FLICKER_SECONDARY_FREQUENCY + phase * TAU * 3.0);
    return 1.0 - FLICKER_DEPTH * wobble;
}

fn sun_facing(normal: vec3<f32>) -> f32 {
    return max(dot(normal, frame.sun_direction.xyz), 0.0);
}

fn sun_irradiance() -> vec3<f32> {
    return frame.sun_color.rgb * frame.sun_direction.w * exposure();
}

// Hemispheric sky light arriving along `direction`, blended from ground bounce to zenith.
fn sky_radiance(direction: vec3<f32>) -> vec3<f32> {
    let zenith_weight = clamp(direction.y * 0.5 + 0.5, 0.0, 1.0);
    return mix(frame.ground_color.rgb, frame.sky_color.rgb, zenith_weight)
        * frame.sun_color.w * exposure();
}

// Normalised Blinn-Phong highlight; the (n + 8) / 8pi factor keeps reflected energy roughly
// constant as the lobe tightens.
fn specular_lobe(normal: vec3<f32>, view_dir: vec3<f32>, specular: f32, shininess: f32) -> f32 {
    let half_vector = normalize(frame.sun_direction.xyz + view_dir);
    let n_dot_h = max(dot(normal, half_vector), 0.0);
    return specular * (shininess + 8.0) / (8.0 * PI) * pow(n_dot_h, shininess);
}

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

// Fraction of the sky hemisphere left open, estimated from how far surrounding terrain rises
// above `surface_h` relative to its horizontal distance.
fn heightfield_occlusion(cell: vec2<i32>, surface_h: f32) -> f32 {
    let strength = frame.horizon_color.w;
    if (strength <= 0.0) {
        return 1.0;
    }
    var occlusion = 0.0;
    for (var i = 0; i < 8; i++) {
        let direction = N_OFFSETS[i];
        let step_length = length(vec2<f32>(direction));
        for (var ring = 0; ring < 2; ring++) {
            let radius = select(AO_RING_OUTER, AO_RING_INNER, ring == 0);
            let offset = vec2<i32>(vec2<f32>(direction) * radius);
            let sample_h = solid_height(cell + offset);
            occlusion += clamp((sample_h - surface_h) / (radius * step_length), 0.0, 1.0);
        }
    }
    return clamp(1.0 - strength * occlusion / AO_SAMPLE_COUNT, 0.0, 1.0);
}

// Soft sun visibility from marching the height field towards the sun from `origin` (cells) at
// `start_h`. Tracks the smallest clearance-over-distance ratio along the ray, which widens the
// penumbra with distance. The march ends where the world is not resident.
fn heightfield_sun_visibility(origin: vec2<f32>, start_h: f32) -> f32 {
    let strength = frame.ground_color.w;
    let sun = frame.sun_direction.xyz;
    let grid_direction = vec2<f32>(sun.x, -sun.z);
    let horizontal = length(grid_direction);
    if (strength <= 0.0 || sun.y <= 0.0 || horizontal < MIN_SUN_HORIZONTAL) {
        return 1.0;
    }
    let step_direction = grid_direction / horizontal;
    let rise_per_cell = sun.y / horizontal;
    var visibility = 1.0;
    var travelled = 0.0;
    var step_length = SHADOW_FIRST_STEP;
    for (var i = 0u; i < SHADOW_STEPS; i++) {
        travelled += step_length;
        step_length += SHADOW_STEP_GROWTH;
        let probe = vec2<i32>(round(origin + step_direction * travelled));
        let slot = slot_of(probe);
        if (slot == SLOT_NONE) {
            break;
        }
        let ray_h = start_h + SHADOW_BIAS + rise_per_cell * travelled;
        let occluder_h = solid_height_of(cell_in(slot, probe));
        visibility = min(visibility, SHADOW_PENUMBRA_SHARPNESS * (ray_h - occluder_h) / travelled);
        if (visibility <= 0.0) {
            break;
        }
    }
    return mix(1.0, smoothstep(0.0, 1.0, clamp(visibility, 0.0, 1.0)), strength);
}

// Light the spheres let through to `position`: the sun's shadow map of the spheres, 3 x 3
// percentage-closer filtered. 1 outside the map or when sphere shadows are off.
fn sphere_shadow(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if (frame.shadow.z <= 0.0) {
        return 1.0;
    }
    // A normal offset keeps a lit surface from shadowing itself at grazing angles.
    let offset = position + normal * 0.15;
    let clip = frame.shadow_view_proj * vec4<f32>(offset, 1.0);
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let depth = ndc.z - frame.shadow.y;
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let at = uv + vec2<f32>(f32(x), f32(y)) * frame.shadow.x;
            lit += textureSampleCompareLevel(shadow_map, shadow_sampler, at, depth);
        }
    }
    return lit / 9.0;
}

// Linear distance fog towards the horizon colour, with the sun's glow scattered into it when
// looking towards the sun. The band is fitted to the camera's height (camera.wgsl).
fn apply_world_fog(color: vec3<f32>, position: vec3<f32>) -> vec3<f32> {
    let to_point = position - view.eye.xyz;
    let distance = length(to_point);
    var scattering = vec3<f32>(0.0);
    if (frame.sun_glow.a > 0.0) {
        let alignment = max(dot(to_point / max(distance, MIN_SUN_HORIZONTAL), frame.sun_direction.xyz), 0.0);
        scattering = pow(alignment, frame.fog.z) * frame.sun_color.rgb * frame.sun_direction.w * exposure();
    }
    let start = view.eye.w;
    let end = view.fog.x;
    let fog_color = frame.horizon_color.rgb + scattering * frame.sun_glow.rgb * frame.sun_glow.a;
    let amount = 1.0 - clamp((end - distance) / max(end - start, 0.001), 0.0, 1.0);
    return mix(color, fog_color, amount);
}

// Opacity of the brush overlay at window-local `position`: a faint disc plus an anti-aliased
// ring at the brush edge. Uses screen-space derivatives, so call it before any `discard`.
fn brush_cursor_mask(position: vec3<f32>) -> f32 {
    let radius = frame.cursor.z;
    let cell = vec2<f32>(position.x, -position.z);
    let distance = length(cell - frame.cursor.xy);
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

// Debug tint of the activity tile under a terrain fragment, from the last tick's scheduling:
// awake tiles yellow, tiles whose rules ran red. Drawn only with OVERLAY_ACTIVITY.
fn activity_tint(color: vec3<f32>, position: vec3<f32>) -> vec3<f32> {
    if ((frame.follow.w & OVERLAY_ACTIVITY) == 0u) {
        return color;
    }
    let cell = vec2<i32>(floor(vec2<f32>(position.x, -position.z) + 0.5));
    let state = tile_state(cell);
    if ((state & TILE_ACTIVE) != 0u) {
        return mix(color, vec3<f32>(1.0, 0.15, 0.1), 0.45);
    }
    if ((state & TILE_AWAKE) != 0u) {
        return mix(color, vec3<f32>(1.0, 0.85, 0.2), 0.3);
    }
    return color;
}
