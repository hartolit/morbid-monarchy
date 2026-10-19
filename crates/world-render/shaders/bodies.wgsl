// Spheres, drawn from the GPU owner's body state: one instance per body slot below the extent,
// a shared unit icosphere scaled by the body's radius and placed at its position as the last
// submitted tick left it. A free slot collapses to nothing. No CPU transform exists: the pose
// is read here, in the same queue order as the ticks that wrote it.
//
// The look follows the baseline's two presets, chosen by relative density: denser than water
// draws as metal (the metal preset), lighter as wood. Lighting is a small metallic/roughness
// model under the same sun, sky, heightfield shadows and sphere shadow map as the terrain.

struct SphereOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // x: metallic, y: roughness, z: overlay state (0 none, 1 grounded, 2 suspended, 3 in liquid)
    @location(2) material: vec3<f32>,
    @location(3) base_color: vec3<f32>,
    // x: heightfield sun visibility at the centre
    @location(4) terms: vec4<f32>,
}

const METAL_COLOR: vec3<f32> = vec3<f32>(0.3185, 0.3185, 0.3864);
const WOOD_COLOR: vec3<f32> = vec3<f32>(0.2623, 0.1195, 0.0399);

@vertex
fn sphere_vertex(@location(0) unit: vec3<f32>, @builtin(instance_index) slot: u32) -> SphereOut {
    var out: SphereOut;
    let body = placed_body(slot);
    if (!body.alive) {
        out.clip_position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return out;
    }
    let position = body.position + unit * body.radius;
    out.clip_position = view.view_proj * vec4<f32>(position, 1.0);
    out.world_pos = position;
    out.normal = unit;
    let metal = body.config.relative_density > 1.0;
    out.base_color = select(WOOD_COLOR, METAL_COLOR, metal);
    out.material = vec3<f32>(select(0.0, 0.8, metal), select(0.8, 0.2, metal), 0.0);
    if ((frame.follow.w & OVERLAY_BODIES) != 0u) {
        var overlay = 0.0;
        if ((body.state.flags & BODY_SUSPENDED) != 0u) {
            overlay = 2.0;
        } else if (body.state.submerged > 0.0) {
            overlay = 3.0;
        } else if ((body.state.flags & BODY_GROUNDED) != 0u) {
            overlay = 1.0;
        }
        out.material.z = overlay;
    }
    let cell = vec2<f32>(body.position.x, -body.position.z);
    out.terms = vec4<f32>(heightfield_sun_visibility_at_point(cell, body.position.y), 0.0, 0.0, 0.0);
    return out;
}

// Heightfield visibility of the sun from a point that is not on a column (a sphere's centre).
fn heightfield_sun_visibility_at_point(cell: vec2<f32>, height: f32) -> f32 {
    if (slot_of(vec2<i32>(round(cell))) == SLOT_NONE) {
        return 1.0;
    }
    return heightfield_sun_visibility(cell, height);
}

// GGX distribution and Smith visibility for the metallic/roughness spheres.
fn ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn smith_visibility(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let v = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a) + a);
    let l = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a) + a);
    return 0.5 / max(v + l, 0.0001);
}

@fragment
fn sphere_fragment(in: SphereOut) -> @location(0) vec4<f32> {
    let normal = normalize(in.normal);
    let view_dir = normalize(view.eye.xyz - in.world_pos);
    let light = frame.sun_direction.xyz;
    let half_vector = normalize(light + view_dir);
    let metallic = in.material.x;
    let roughness = in.material.y;
    let n_dot_l = max(dot(normal, light), 0.0);
    let n_dot_v = max(dot(normal, view_dir), 0.0001);
    let n_dot_h = max(dot(normal, half_vector), 0.0);
    let f0 = mix(vec3<f32>(0.04), in.base_color, metallic);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - max(dot(half_vector, view_dir), 0.0), 5.0);
    let specular = fresnel * ggx(n_dot_h, roughness) * smith_visibility(n_dot_v, n_dot_l, roughness);
    let diffuse = (1.0 - fresnel) * (1.0 - metallic) * in.base_color * INV_PI;
    let visibility = in.terms.x * sphere_shadow(in.world_pos, normal);
    let direct = (diffuse + specular) * sun_irradiance() * n_dot_l * visibility;
    let ambient = mix(in.base_color * (1.0 - metallic), f0, 0.5 * metallic) * sky_radiance(normal);
    var color = direct + ambient;
    if (in.material.z > 0.5) {
        let tint = select(select(vec3<f32>(0.2, 0.5, 1.0), vec3<f32>(0.5, 0.5, 0.5), in.material.z < 2.5),
            vec3<f32>(0.2, 1.0, 0.3), in.material.z < 1.5);
        color = mix(color, tint, 0.5);
    }
    color = apply_world_fog(color, in.world_pos);
    return vec4<f32>(color, 1.0);
}

// The shadow pass: spheres only, as depth from the sun.
@vertex
fn sphere_shadow_vertex(@location(0) unit: vec3<f32>, @builtin(instance_index) slot: u32) -> @builtin(position) vec4<f32> {
    let body = placed_body(slot);
    if (!body.alive) {
        return vec4<f32>(0.0, 0.0, 2.0, 1.0);
    }
    return frame.shadow_view_proj * vec4<f32>(body.position + unit * body.radius, 1.0);
}
