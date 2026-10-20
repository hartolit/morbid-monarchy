// The four landscape layers (terrain, granular, fluid, surface), drawn straight from the
// resident cells. One instance per window chunk and layer; each instance is a 65 x 65 vertex
// grid whose vertex (x, y) is the cell at that offset from the chunk's first cell (the last
// row and column are the neighbouring chunks' first cells, closing the seams). An instance
// whose chunk is not resident collapses to nothing. Nothing is uploaded when cells change:
// every frame reads the cells as the last submitted tick left them.
//
// Ported from the baseline's landscape material (crates/morbid-app/assets/shaders/landscape,
// prompt 01's visual reference), with the same layering, palette, lighting terms and liquid
// effects; addressing goes through the window map instead of a toroidal CPU copy.

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
// Young foliage is paler and yellower than mature growth.
const FOLIAGE_YOUNG_TINT: vec3<f32> = vec3<f32>(1.45, 1.3, 0.55);
const FLAME_SWAY_UNITS: f32 = 1.5;
const FLAME_SWAY_FREQUENCY: f32 = 7.0;

const VERTS_ACROSS: u32 = CHUNK_SIZE + 1u;

struct LayerOut {
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
}

struct LayerSample {
    elevation: f32,
    mat_lookup: u32,
    has_mat: bool,
    is_hidden: bool,
}

fn layer_mat(data: CellData, layer: u32) -> u32 {
    switch layer {
        case 0u: { return data.terrain_mat; }
        case 1u: { return data.granular_mat; }
        case 2u: { return data.fluid_mat; }
        case 3u: { return data.surface_mat; }
        default: { return 0u; }
    }
}

fn layer_height(data: CellData, layer: u32) -> f32 {
    switch layer {
        case 0u: { return data.terrain_h; }
        case 1u: { return data.granular_h; }
        case 2u: { return data.fluid_h; }
        case 3u: { return data.surface_h; }
        default: { return 0.0; }
    }
}

// Places a layer vertex. Columns without the layer's material either borrow a neighbour's
// height to close the layer's edge, or hide just under their own floor.
fn sample_layer(center: CellData, cell: vec2<i32>, layer: u32) -> LayerSample {
    var sample: LayerSample;
    sample.is_hidden = false;
    if (layer == LAYER_TERRAIN) {
        sample.elevation = center.terrain_h;
        sample.mat_lookup = center.terrain_mat;
        sample.has_mat = true;
        return sample;
    }
    let active_mat = layer_mat(center, layer);
    sample.has_mat = active_mat != 0u;
    if (sample.has_mat) {
        sample.elevation = layer_height(center, layer);
        sample.mat_lookup = active_mat;
        return sample;
    }
    let floor_h = max(center.terrain_h, select(0.0, center.granular_h, center.granular_mat != 0u));
    var max_n_h = 0.0;
    var fallback_mat = 0u;
    for (var i = 0; i < 8; i++) {
        let n_data = cell_data(cell + N_OFFSETS[i]);
        let n_mat = layer_mat(n_data, layer);
        if (n_mat != 0u) {
            if (fallback_mat == 0u) {
                fallback_mat = n_mat;
            }
            max_n_h = max(max_n_h, layer_height(n_data, layer));
        }
    }
    sample.mat_lookup = fallback_mat;
    let edge_tolerance = select(0.0, FLUID_EDGE_TOLERANCE_UNITS * frame.window_cells.z, layer == LAYER_FLUID);
    if (max_n_h > 0.0 && floor_h + edge_tolerance >= max_n_h) {
        sample.elevation = max_n_h;
    } else {
        sample.elevation = floor_h - HIDDEN_LAYER_DEPTH;
        sample.is_hidden = true;
    }
    return sample;
}

// Central differences of the layer's height field. Cell +y maps to render -z.
fn layer_normal(cell: vec2<i32>, layer: u32) -> vec3<f32> {
    let h_left = layer_height(cell_data(cell + vec2<i32>(-1, 0)), layer);
    let h_right = layer_height(cell_data(cell + vec2<i32>(1, 0)), layer);
    let h_down = layer_height(cell_data(cell + vec2<i32>(0, -1)), layer);
    let h_up = layer_height(cell_data(cell + vec2<i32>(0, 1)), layer);
    return normalize(vec3<f32>(h_left - h_right, 2.0, h_up - h_down));
}

fn landscape_vertex(vertex_index: u32, chunk_index: u32, layer: u32) -> LayerOut {
    var out: LayerOut;
    let slot = window_slots[chunk_index];
    if (slot == SLOT_NONE) {
        // Beyond the far plane: every vertex of the instance lands here, so it draws nothing.
        out.clip_position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return out;
    }
    let chunks_x = u32(frame.window.z);
    home_chunk = vec2<i32>(i32(chunk_index % chunks_x), i32(chunk_index / chunks_x));
    home_slot = slot;
    let offset = vec2<i32>(i32(vertex_index % VERTS_ACROSS), i32(vertex_index / VERTS_ACROSS));
    let cell = home_chunk * i32(CHUNK_SIZE) + offset;
    let scale = frame.window_cells.z;

    let center = cell_data(cell);
    let layer_sample = sample_layer(center, cell, layer);
    let phase = f32(center.variants) / VARIANT_MAX;
    let is_flame = layer == LAYER_SURFACE && layer_sample.mat_lookup == SURFACE_FIRE;

    var height = layer_sample.elevation;
    if (is_flame && layer_sample.has_mat) {
        height += sin(seconds() * FLAME_SWAY_FREQUENCY + phase * TAU) * FLAME_SWAY_UNITS * scale;
    }
    let position = vec3<f32>(f32(cell.x), height, -f32(cell.y));
    out.clip_position = view.view_proj * vec4<f32>(position, 1.0);
    out.world_pos = position;
    out.world_normal = layer_normal(cell, layer);

    // Hidden vertices sit under the floor and are never seen, so skip their expensive terms.
    var ambient_occlusion = 1.0;
    var sun_visibility = 1.0;
    if (!layer_sample.is_hidden) {
        ambient_occlusion = heightfield_occlusion(cell, height);
        sun_visibility = heightfield_sun_visibility(vec2<f32>(cell), height);
    }

    let palette_index = layer * PALETTE_LAYER_STRIDE + layer_sample.mat_lookup;
    var albedo = palette[palette_index];
    out.material_props = palette[palette_index + PALETTE_PROPERTY_OFFSET];
    let visual_shift = (f32(center.variants) - VARIANT_NEUTRAL) * VARIANT_BRIGHTNESS_STEP;
    albedo = vec4<f32>(saturate(albedo.rgb * (1.0 + visual_shift * 2.0)), albedo.a);
    if (layer == LAYER_SURFACE && layer_sample.mat_lookup == SURFACE_FOLIAGE) {
        let growth = saturate(center.surface_state / f32(FOLIAGE_MATURE_STATE));
        albedo = vec4<f32>(mix(saturate(albedo.rgb * FOLIAGE_YOUNG_TINT), albedo.rgb, growth), albedo.a);
    }
    out.albedo = albedo;

    let coverage = select(select(0.0, 1.0, layer_sample.has_mat), 1.0, layer == LAYER_TERRAIN);
    let liquid_depth = select(0.0, max(height - center.granular_h, 0.0), layer == LAYER_FLUID && layer_sample.has_mat);
    out.lighting_terms = vec4<f32>(ambient_occlusion, sun_visibility, liquid_depth, coverage);

    // Solid layers beneath a liquid receive sunlight filtered through it.
    var submersion = vec4<f32>(0.0);
    if (layer != LAYER_FLUID && center.fluid_mat != 0u) {
        let fluid_slot = LAYER_FLUID * PALETTE_LAYER_STRIDE + center.fluid_mat;
        let covering_depth = max(center.fluid_h - height, 0.0);
        submersion = vec4<f32>(palette[fluid_slot].rgb, covering_depth * palette[fluid_slot + PALETTE_PROPERTY_OFFSET].w);
    }
    out.submersion = submersion;
    out.animation = vec4<f32>(phase, 0.0, 0.0, 0.0);
    return out;
}

// The window chunk of a layer's instance: from the layer's visible list, or every chunk in
// order on the unculled comparison path.
fn layer_chunk(layer: u32, instance: u32) -> u32 {
    if (frame.offsets.z != 0u) {
        return instance;
    }
    let chunks = u32(frame.window.z * frame.window.w);
    return layer_lists[layer * chunks + instance];
}

@vertex
fn terrain_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> LayerOut {
    return landscape_vertex(vertex, layer_chunk(LAYER_TERRAIN, instance), LAYER_TERRAIN);
}

@vertex
fn granular_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> LayerOut {
    return landscape_vertex(vertex, layer_chunk(LAYER_GRANULAR, instance), LAYER_GRANULAR);
}

@vertex
fn fluid_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> LayerOut {
    return landscape_vertex(vertex, layer_chunk(LAYER_FLUID, instance), LAYER_FLUID);
}

@vertex
fn surface_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> LayerOut {
    return landscape_vertex(vertex, layer_chunk(LAYER_SURFACE, instance), LAYER_SURFACE);
}

// Light reaching a surface under a liquid is tinted and dimmed by the column above it.
fn submerged_light_filter(submersion: vec4<f32>) -> vec3<f32> {
    let transmittance = exp(-submersion.a);
    return mix(submersion.rgb, vec3<f32>(1.0), transmittance);
}

// The opaque layers (terrain, granular, surface).
@fragment
fn solid_fragment(in: LayerOut) -> @location(0) vec4<f32> {
    let cursor_mask = brush_cursor_mask(in.world_pos);
    if (in.lighting_terms.w <= COVERAGE_DISCARD_THRESHOLD) {
        discard;
    }
    let normal = normalize(in.world_normal);
    let view_dir = normalize(view.eye.xyz - in.world_pos);
    let props = in.material_props;
    let sun_visibility = in.lighting_terms.y * sphere_shadow(in.world_pos, normal);
    var color = shade_lit_surface(in.albedo.rgb, normal, view_dir, props.x, props.y, sun_visibility, in.lighting_terms.x);
    color *= submerged_light_filter(in.submersion);
    color += in.albedo.rgb * props.z * flame_flicker(in.animation.x);
    color = activity_tint(color, in.world_pos);
    color = apply_world_fog(color, in.world_pos);
    return vec4<f32>(apply_brush_cursor(color, cursor_mask), 1.0);
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
// Emissive liquids are viscous melts; their ripples are suppressed by this factor per unit of
// emission.
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
// Detail only wobbles the crack lines; blending two noises evenly would pile values up at the
// mid-level and turn most of the surface into crack.
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

fn ripple_normal(position: vec3<f32>, base_normal: vec3<f32>, strength: f32) -> vec3<f32> {
    // Ripples follow absolute cells, so the pattern does not jump when the window moves.
    let xz = absolute_xz(position);
    var slope = vec2<f32>(0.0);
    for (var i = 0u; i < WAVE_COUNT; i++) {
        let direction = WAVE_DIRECTIONS[i];
        let shape = WAVE_SHAPES[i];
        let phase = dot(xz, direction) * shape.x + seconds() * shape.y;
        slope += direction * (shape.x * shape.z * cos(phase));
    }
    return normalize(base_normal - vec3<f32>(slope.x, 0.0, slope.y) * strength);
}

// The absolute render-space xz of a window-local position, for patterns tied to the world.
fn absolute_xz(position: vec3<f32>) -> vec2<f32> {
    let origin = vec2<f32>(frame.window.xy * i32(CHUNK_SIZE));
    return vec2<f32>(position.x + origin.x, position.z - origin.y);
}

// Tilts the surface normal by a constant random amount per facet cell.
fn ice_facet_normal(position: vec3<f32>, base_normal: vec3<f32>, strength: f32) -> vec3<f32> {
    let facet = floor(ICE_FACET_LATTICE * absolute_xz(position) * ICE_FACET_SCALE);
    let tilt = vec2<f32>(hash21(facet), hash21(facet + ICE_FACET_HASH_OFFSET)) - 0.5;
    return normalize(base_normal + vec3<f32>(tilt.x, 0.0, tilt.y) * ICE_FACET_TILT * strength);
}

// 1 on a frost line, 0 elsewhere.
fn ice_cracks(position: vec3<f32>) -> f32 {
    let field = value_noise(absolute_xz(position) * ICE_CRACK_SCALE);
    return 1.0 - smoothstep(ICE_CRACK_LOW, ICE_CRACK_HIGH, abs(field - 0.5) * 2.0);
}

// 0 on cooled crust, 1 in the glowing cracks between plates.
fn crust_cracks(position: vec3<f32>) -> f32 {
    let drift = vec2<f32>(seconds() * CRUST_DRIFT_SPEED);
    let xz = absolute_xz(position) + vec2<f32>(position.y * CRUST_HEIGHT_SHEAR);
    let plates = value_noise(xz * CRUST_NOISE_SCALE + drift);
    let detail = value_noise(xz * CRUST_DETAIL_SCALE - drift);
    let field = plates + (detail - 0.5) * CRUST_DETAIL_WEIGHT;
    return 1.0 - smoothstep(CRUST_CRACK_LOW, CRUST_CRACK_HIGH, abs(field - 0.5) * 2.0);
}

@fragment
fn fluid_fragment(in: LayerOut) -> @location(0) vec4<f32> {
    let cursor_mask = brush_cursor_mask(in.world_pos);
    let coverage = in.lighting_terms.w;
    if (coverage <= COVERAGE_DISCARD_THRESHOLD) {
        discard;
    }
    let props = in.material_props;
    let emission = props.z;
    let optical_density = props.w;
    let depth = in.lighting_terms.z;
    // The palette stores each liquid's mobility in its albedo alpha: 1 flows, 0 is frozen.
    let mobility = in.albedo.a;
    let frozen = 1.0 - mobility;

    let view_dir = normalize(view.eye.xyz - in.world_pos);
    let ripple = RIPPLE_STRENGTH * mobility / (1.0 + emission * EMISSIVE_RIPPLE_DAMPING);
    var normal = ripple_normal(in.world_pos, normalize(in.world_normal), ripple);
    if (frozen > 0.0) {
        normal = ice_facet_normal(in.world_pos, normal, frozen);
    }
    let sun_visibility = in.lighting_terms.y * sphere_shadow(in.world_pos, normal);

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

    // The material's specular weight scales its mirror reflection, so glossy water reflects the
    // sky while a melt's crust barely does.
    let reflectance = fresnel * props.x;
    var color = mix(body, reflection, reflectance) + glint;
    var alpha = max(1.0 - transmittance, reflectance);

    let foam_band = 1.0 - smoothstep(0.0, FOAM_DEPTH, depth);
    let foam_pattern = value_noise(absolute_xz(in.world_pos) * FOAM_NOISE_SCALE + seconds() * FOAM_DRIFT_SPEED);
    let foam = foam_band * smoothstep(1.0 - FOAM_COVERAGE, 1.0, foam_pattern) * step(emission, 0.0) * mobility;
    let foam_color = shade_lit_surface(FOAM_ALBEDO, normal, view_dir, 0.0, 1.0, sun_visibility, in.lighting_terms.x);
    color = mix(color, foam_color, foam);
    alpha = max(alpha, foam);

    if (frozen > 0.0) {
        let frost = ice_cracks(in.world_pos) * ICE_CRACK_OPACITY * frozen;
        let frost_color = shade_lit_surface(ICE_FROST_ALBEDO, normal, view_dir, 0.0, 1.0, sun_visibility, in.lighting_terms.x);
        color = mix(color, frost_color, frost);
        alpha = max(max(alpha, frost), ICE_MIN_OPACITY * frozen);
    }

    if (emission >= MOLTEN_EMISSION) {
        let cracks = crust_cracks(in.world_pos);
        let crust = shade_lit_surface(CRUST_ALBEDO, normal, view_dir, props.x, props.y, sun_visibility, in.lighting_terms.x);
        let glow_color = mix(in.albedo.rgb, MOLTEN_GLOW, cracks);
        color = mix(crust, color, cracks);
        color += glow_color * emission * mix(CRUST_GLOW_FLOOR, 1.0, cracks) * flame_flicker(in.animation.x);
    } else {
        color += in.albedo.rgb * emission;
    }

    alpha = max(alpha * smoothstep(COVERAGE_DISCARD_THRESHOLD, COVERAGE_FADE_END, coverage), cursor_mask);
    if (alpha < MIN_VISIBLE_ALPHA) {
        discard;
    }
    color = apply_world_fog(color, in.world_pos);
    return vec4<f32>(apply_brush_cursor(color, cursor_mask), alpha);
}
