#import bevy_pbr::mesh_view_bindings::{view, globals}
#import "shaders/landscape/common.wgsl"::{
    VertexInput, VertexOutput, LAYER_FLUID, COVERAGE_DISCARD_THRESHOLD,
    process_landscape_vertex, shade_lit_surface, specular_lobe, sky_radiance, sun_irradiance, sun_facing,
    entity_shadow, apply_world_fog, brush_cursor_mask, apply_brush_cursor, value_noise, flame_flicker,
    hash21,
}

// Surface ripples: a sum of directional sine waves, differentiated analytically into a slope.
const WAVE_COUNT: u32 = 4u;
const WAVE_DIRECTIONS: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2<f32>(0.80, 0.60), vec2<f32>(-0.39, 0.92), vec2<f32>(0.96, -0.28), vec2<f32>(-0.71, -0.71)
);
// Spatial frequency (radians per world unit), angular speed (radians per second), amplitude.
const WAVE_SHAPES: array<vec3<f32>, 4> = array<vec3<f32>, 4>(
    vec3<f32>(0.45, 1.3, 0.35), vec3<f32>(0.85, 1.9, 0.18), vec3<f32>(1.6, 2.7, 0.08), vec3<f32>(2.9, 3.6, 0.04)
);
const RIPPLE_STRENGTH: f32 = 0.6;
// Emissive liquids are viscous melts; their ripples are suppressed by this factor per unit
// of emission.
const EMISSIVE_RIPPLE_DAMPING: f32 = 3.0;

// Schlick Fresnel reflectance at normal incidence for water-like liquids.
const FRESNEL_F0: f32 = 0.02;
const FRESNEL_POWER: f32 = 5.0;
// Viewing at a grazing angle lengthens the path through the liquid.
const MIN_VIEW_COSINE: f32 = 0.2;

// Foam forms in a band of shallow liquid along shores.
const FOAM_DEPTH: f32 = 0.9;
const FOAM_NOISE_SCALE: f32 = 1.7;
const FOAM_DRIFT_SPEED: f32 = 0.35;
const FOAM_COVERAGE: f32 = 0.55;
const FOAM_ALBEDO: vec3<f32> = vec3<f32>(0.85, 0.88, 0.9);

// Liquids glowing at least this strongly are melts: crusted, barely reflective and hot.
const MOLTEN_EMISSION: f32 = 1.0;
// Colour of the hottest melt in the cracks, which runs towards orange-yellow.
const MOLTEN_GLOW: vec3<f32> = vec3<f32>(1.0, 0.38, 0.06);

// Molten crust: dark cooled basalt plates drifting over bright cracks.
const CRUST_ALBEDO: vec3<f32> = vec3<f32>(0.05, 0.04, 0.035);
const CRUST_NOISE_SCALE: f32 = 0.35;
const CRUST_DETAIL_SCALE: f32 = 1.1;
// Detail only wobbles the crack lines; blending two noises evenly would pile values up at
// the mid-level and turn most of the surface into crack.
const CRUST_DETAIL_WEIGHT: f32 = 0.15;
const CRUST_DRIFT_SPEED: f32 = 0.08;
// Height shears the pattern so steep flow fronts are not smeared into vertical stripes.
const CRUST_HEIGHT_SHEAR: f32 = 0.7;
// Cracks run along the noise field's mid-level; these bound the distance from it.
const CRUST_CRACK_LOW: f32 = 0.03;
const CRUST_CRACK_HIGH: f32 = 0.1;
// Residual glow of the crust itself, as a fraction of the crack glow.
const CRUST_GLOW_FLOOR: f32 = 0.03;

// Ice: per-cell tilted facets give a crystalline mosaic that sparkles under the sun, and thin
// frost lines run along the mid-level of a noise field.
const ICE_FACET_SCALE: f32 = 0.7;
const ICE_FACET_TILT: f32 = 0.1;
// The facet lattice is sheared off the cell grid so crystals do not line up with columns.
const ICE_FACET_LATTICE: mat2x2<f32> = mat2x2<f32>(0.87, 0.5, -0.5, 0.87);
const ICE_FACET_HASH_OFFSET: vec2<f32> = vec2<f32>(17.0, 31.0);
const ICE_CRACK_SCALE: f32 = 0.22;
const ICE_CRACK_LOW: f32 = 0.025;
const ICE_CRACK_HIGH: f32 = 0.07;
const ICE_CRACK_OPACITY: f32 = 0.6;
const ICE_FROST_ALBEDO: vec3<f32> = vec3<f32>(0.93, 0.96, 1.0);
// Even thin ice reads as a solid sheet.
const ICE_MIN_OPACITY: f32 = 0.45;

// Edge fade over the last stretch of a liquid's footprint.
const COVERAGE_FADE_END: f32 = 0.6;
// The layer writes depth, so fragments this transparent are dropped rather than allowed to
// hide whatever is drawn behind them later.
const MIN_VISIBLE_ALPHA: f32 = 0.02;

fn ripple_normal(world_pos: vec3<f32>, base_normal: vec3<f32>, strength: f32) -> vec3<f32> {
    let position = world_pos.xz;
    var slope = vec2<f32>(0.0);
    for (var i = 0u; i < WAVE_COUNT; i++) {
        let direction = WAVE_DIRECTIONS[i];
        let shape = WAVE_SHAPES[i];
        let phase = dot(position, direction) * shape.x + globals.time * shape.y;
        slope += direction * (shape.x * shape.z * cos(phase));
    }
    return normalize(base_normal - vec3<f32>(slope.x, 0.0, slope.y) * strength);
}

// Tilts the surface normal by a constant random amount per facet cell.
fn ice_facet_normal(world_pos: vec3<f32>, base_normal: vec3<f32>, strength: f32) -> vec3<f32> {
    let facet = floor(ICE_FACET_LATTICE * world_pos.xz * ICE_FACET_SCALE);
    let tilt = vec2<f32>(hash21(facet), hash21(facet + ICE_FACET_HASH_OFFSET)) - 0.5;
    return normalize(base_normal + vec3<f32>(tilt.x, 0.0, tilt.y) * ICE_FACET_TILT * strength);
}

// 1 on a frost line, 0 elsewhere.
fn ice_cracks(world_pos: vec3<f32>) -> f32 {
    let field = value_noise(world_pos.xz * ICE_CRACK_SCALE);
    return 1.0 - smoothstep(ICE_CRACK_LOW, ICE_CRACK_HIGH, abs(field - 0.5) * 2.0);
}

// 0 on cooled crust, 1 in the glowing cracks between plates.
fn crust_cracks(world_pos: vec3<f32>) -> f32 {
    let drift = vec2<f32>(globals.time * CRUST_DRIFT_SPEED);
    let position = world_pos.xz + vec2<f32>(world_pos.y * CRUST_HEIGHT_SHEAR);
    let plates = value_noise(position * CRUST_NOISE_SCALE + drift);
    let detail = value_noise(position * CRUST_DETAIL_SCALE - drift);
    let field = plates + (detail - 0.5) * CRUST_DETAIL_WEIGHT;
    return 1.0 - smoothstep(CRUST_CRACK_LOW, CRUST_CRACK_HIGH, abs(field - 0.5) * 2.0);
}

@vertex
fn vertex(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    return process_landscape_vertex(in, instance_index, LAYER_FLUID);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let cursor_mask = brush_cursor_mask(in.world_pos);
    let coverage = in.lighting_terms.w;
    if coverage <= COVERAGE_DISCARD_THRESHOLD {
        discard;
    }

    let props = in.material_props;
    let emission = props.z;
    let optical_density = props.w;
    let depth = in.lighting_terms.z;

    // The palette stores each liquid's mobility in its albedo alpha: 1 flows, 0 is frozen.
    let mobility = in.albedo.a;
    let frozen = 1.0 - mobility;

    let view_dir = normalize(view.world_position - in.world_pos);
    let ripple = RIPPLE_STRENGTH * mobility / (1.0 + emission * EMISSIVE_RIPPLE_DAMPING);
    var normal = ripple_normal(in.world_pos, normalize(in.world_normal), ripple);
    if frozen > 0.0 {
        normal = ice_facet_normal(in.world_pos, normal, frozen);
    }
    let sun_visibility = in.lighting_terms.y * entity_shadow(in.world_pos, normal, in.clip_position);

    // Beer-Lambert absorption along the view ray through the column decides how much of the
    // bed shows through; the blend's alpha carries the absorbed fraction.
    let path_length = depth / max(dot(view_dir, vec3<f32>(0.0, 1.0, 0.0)), MIN_VIEW_COSINE);
    let transmittance = exp(-optical_density * path_length);

    let n_dot_v = max(dot(normal, view_dir), 0.0);
    let fresnel = FRESNEL_F0 + (1.0 - FRESNEL_F0) * pow(1.0 - n_dot_v, FRESNEL_POWER);
    let reflection = sky_radiance(reflect(-view_dir, normal));

    let body = shade_lit_surface(in.albedo.rgb, normal, view_dir, 0.0, 1.0, sun_visibility, in.lighting_terms.x);
    let glint = specular_lobe(normal, view_dir, props.x, props.y) * sun_irradiance()
        * sun_facing(normal) * sun_visibility;

    // The material's specular weight scales its mirror reflection, so glossy water reflects
    // the sky while a melt's crust barely does.
    let reflectance = fresnel * props.x;
    var color = mix(body, reflection, reflectance) + glint;
    var alpha = max(1.0 - transmittance, reflectance);

    let foam_band = 1.0 - smoothstep(0.0, FOAM_DEPTH, depth);
    let foam_pattern = value_noise(in.world_pos.xz * FOAM_NOISE_SCALE + globals.time * FOAM_DRIFT_SPEED);
    let foam = foam_band * smoothstep(1.0 - FOAM_COVERAGE, 1.0, foam_pattern) * step(emission, 0.0)
        * mobility;
    let foam_color = shade_lit_surface(FOAM_ALBEDO, normal, view_dir, 0.0, 1.0, sun_visibility, in.lighting_terms.x);
    color = mix(color, foam_color, foam);
    alpha = max(alpha, foam);

    if frozen > 0.0 {
        let frost = ice_cracks(in.world_pos) * ICE_CRACK_OPACITY * frozen;
        let frost_color = shade_lit_surface(ICE_FROST_ALBEDO, normal, view_dir, 0.0, 1.0, sun_visibility, in.lighting_terms.x);
        color = mix(color, frost_color, frost);
        alpha = max(max(alpha, frost), ICE_MIN_OPACITY * frozen);
    }

    if emission >= MOLTEN_EMISSION {
        let cracks = crust_cracks(in.world_pos);
        let crust = shade_lit_surface(CRUST_ALBEDO, normal, view_dir, props.x, props.y, sun_visibility, in.lighting_terms.x);
        let glow_color = mix(in.albedo.rgb, MOLTEN_GLOW, cracks);
        color = mix(crust, color, cracks);
        color += glow_color * emission * mix(CRUST_GLOW_FLOOR, 1.0, cracks) * flame_flicker(in.animation.x);
    } else {
        color += in.albedo.rgb * emission;
    }

    alpha = max(alpha * smoothstep(COVERAGE_DISCARD_THRESHOLD, COVERAGE_FADE_END, coverage), cursor_mask);
    if alpha < MIN_VISIBLE_ALPHA {
        discard;
    }
    color = apply_world_fog(color, in.world_pos);
    return vec4<f32>(apply_brush_cursor(color, cursor_mask), alpha);
}
