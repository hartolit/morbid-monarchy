//! Probes of the body solver's pure functions, executed on the device against expectations
//! computed independently on the CPU: the baseline's closest-point and cap-volume tests, and
//! its apportionment test against the GPU's exact split.

use gpu_world::shader::{
    Assembly, SourceModule, body_geometry, rule_modules, shared_modules, window_helpers,
};
use world_model::glam::DVec3;

use super::super::{
    Check,
    driver::{Driver, config},
    probe::{self, Binding},
};

/// The globals the body geometry and the window helpers name, at bindings no probe kernel
/// uses, so the probe's automatic layout leaves them out.
const BODY_CONTEXT: &str = r#"
@group(0) @binding(8) var<uniform> tick: TickParams;
@group(0) @binding(9) var<uniform> body_params: BodyParams;
@group(0) @binding(10) var<storage, read> cells: array<vec2<u32>>;
@group(0) @binding(11) var<storage, read> window_slots: array<u32>;
@group(0) @binding(12) var<storage, read_write> activity: array<atomic<u32>>;
@group(0) @binding(13) var<storage, read_write> diagnostics: array<atomic<u32>>;
"#;

const GEOMETRY_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> probe_in: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> probe_out: array<u32>;
@group(0) @binding(2) var<uniform> probe_count: vec4<u32>;

// probe_in holds (p, a, b, c) per triangle query; three output words each.
@compute @workgroup_size(64, 1, 1)
fn probe_closest(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= probe_count.x) {
        return;
    }
    let base = gid.x * 4u;
    let q = closest_point_on_triangle(probe_in[base].xyz, probe_in[base + 1u].xyz, probe_in[base + 2u].xyz, probe_in[base + 3u].xyz);
    probe_out[gid.x * 3u] = bitcast<u32>(q.x);
    probe_out[gid.x * 3u + 1u] = bitcast<u32>(q.y);
    probe_out[gid.x * 3u + 2u] = bitcast<u32>(q.z);
}

// probe_in holds (depth, radius) per query; one output word each.
@compute @workgroup_size(64, 1, 1)
fn probe_cap(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= probe_count.x) {
        return;
    }
    let q = probe_in[gid.x];
    probe_out[gid.x] = bitcast<u32>(submerged_volume_fraction(q.x, q.y));
}
"#;

const APPORTION_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> probe_words: array<u32>;
@group(0) @binding(1) var<storage, read_write> probe_out: array<u32>;
@group(0) @binding(2) var<uniform> probe_count: vec4<u32>;

// probe_words: (a, b, c) triples; one output word each: mul_div_floor(a, b, c).
@compute @workgroup_size(64, 1, 1)
fn probe_mul_div(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= probe_count.x) {
        return;
    }
    let base = gid.x * 3u;
    probe_out[gid.x] = mul_div_floor(probe_words[base], probe_words[base + 1u], probe_words[base + 2u]);
}

// probe_words: `count` capacities. Invocation m splits m units over them as the deformation
// and displacement rounds do (shove, displace_apply) and writes `count` shares.
@compute @workgroup_size(64, 1, 1)
fn probe_shares(@builtin(global_invocation_id) gid: vec3<u32>) {
    let count = probe_count.x;
    let total = probe_count.y;
    let moved = gid.x;
    if (moved > total) {
        return;
    }
    var before = 0u;
    var taken = 0u;
    for (var i = 0u; i < count; i++) {
        before += probe_words[i];
        let through = mul_div_floor(moved, before, total);
        probe_out[moved * count + i] = through - taken;
        taken = through;
    }
}
"#;

fn assembly(name: &'static str, text: &'static str) -> Assembly {
    let mut modules = shared_modules();
    modules.extend(rule_modules());
    modules.push(SourceModule::file("probe/body_context.wgsl", BODY_CONTEXT));
    modules.push(window_helpers());
    modules.push(body_geometry());
    modules.push(SourceModule::file("probe/body.wgsl", text));
    Assembly::new(name, &modules)
}

fn words(values: &[[f32; 4]]) -> Vec<u32> {
    values
        .iter()
        .flatten()
        .map(|value| value.to_bits())
        .collect()
}

/// The closest point on triangle `abc` to `p`, by a different method than the kernel's: the
/// plane projection when it lies inside, otherwise the nearest of the three edges' closest
/// points, in `f64`.
fn closest_reference(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let normal = (b - a).cross(c - a);
    let area = normal.length_squared();
    if area > 0.0 {
        let projected = p - normal * ((p - a).dot(normal) / area);
        let inside = [(a, b), (b, c), (c, a)]
            .iter()
            .all(|(from, to)| (*to - *from).cross(projected - *from).dot(normal) >= 0.0);
        if inside {
            return projected;
        }
    }
    let on_segment = |from: DVec3, to: DVec3| {
        let edge = to - from;
        let t =
            ((p - from).dot(edge) / edge.length_squared().max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
        from + edge * t
    };
    [on_segment(a, b), on_segment(b, c), on_segment(c, a)]
        .into_iter()
        .min_by(|x, y| p.distance_squared(*x).total_cmp(&p.distance_squared(*y)))
        .unwrap_or(a)
}

/// `closest_point_respects_triangle_regions`, and arbitrary triangles against the reference.
pub fn closest_point_regions(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let (a, b, c) = (
        [0.0, 0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, -2.0, 0.0],
    );
    let mut queries: Vec<[[f32; 4]; 4]> = vec![
        // The baseline's three: above the face, in the vertex region, beside an edge.
        [[0.5, 3.0, -0.5, 0.0], a, b, c],
        [[-1.0, 1.0, 1.0, 0.0], a, b, c],
        [[1.0, 0.0, 1.0, 0.0], a, b, c],
    ];
    // Arbitrary triangles and points, from a fixed sequence.
    let mut state = 0x5EED_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as f32 / (1u64 << 31) as f32) * 8.0 - 4.0
    };
    for _ in 0..509 {
        let mut point = || [next(), next(), next(), 0.0];
        queries.push([point(), point(), point(), point()]);
    }
    let input: Vec<[f32; 4]> = queries.iter().flatten().copied().collect();
    let outputs = probe::run(
        &mut driver.world,
        &assembly("closest point probe", GEOMETRY_PROBE),
        "probe_closest",
        &[
            Binding::Storage(&words(&input)),
            Binding::Output(queries.len() as u32 * 3),
            Binding::Uniform(&[queries.len() as u32, 0, 0, 0]),
        ],
        [(queries.len() as u32).div_ceil(64), 1, 1],
    )?;
    let got = |index: usize| {
        DVec3::new(
            f64::from(f32::from_bits(outputs[0][index * 3])),
            f64::from(f32::from_bits(outputs[0][index * 3 + 1])),
            f64::from(f32::from_bits(outputs[0][index * 3 + 2])),
        )
    };
    let expected_fixed = [
        DVec3::new(0.5, 0.0, -0.5),
        DVec3::ZERO,
        DVec3::new(1.0, 0.0, 0.0),
    ];
    for (index, expected) in expected_fixed.iter().enumerate() {
        check.that(got(index).distance(*expected) < 1e-6, || {
            format!("baseline query {index}: {} expected {expected}", got(index))
        });
    }
    let mut worst = 0.0f64;
    for (index, query) in queries.iter().enumerate().skip(3) {
        let v = |q: [f32; 4]| DVec3::new(f64::from(q[0]), f64::from(q[1]), f64::from(q[2]));
        let reference = closest_reference(v(query[0]), v(query[1]), v(query[2]), v(query[3]));
        // Both are closest points: compare the distances they achieve (a degenerate triangle
        // can have several), within f32 rounding of coordinates of magnitude 4.
        let p = v(query[0]);
        worst = worst.max((p.distance(got(index)) - p.distance(reference)).abs());
    }
    check.that(worst < 2e-5, || {
        format!("closest distance differs by up to {worst}")
    });
    check.metric("arbitrary_queries", queries.len() as u64 - 3);
    check.metric("worst_distance_error", worst);
    Ok(())
}

/// `submerged_fraction_matches_cap_volume`, and the cap against a numerical integral.
pub fn cap_volume(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let radius = 4.0f32;
    let mut queries = vec![
        [0.0, radius, 0.0, 0.0],
        [radius, radius, 0.0, 0.0],
        [2.0 * radius, radius, 0.0, 0.0],
        [3.0 * radius, radius, 0.0, 0.0],
        [-1.0, radius, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
    ];
    for step in 1..64 {
        let r = 2.0 + (step % 7) as f32;
        queries.push([2.0 * r * step as f32 / 64.0, r, 0.0, 0.0]);
    }
    let outputs = probe::run(
        &mut driver.world,
        &assembly("cap probe", GEOMETRY_PROBE),
        "probe_cap",
        &[
            Binding::Storage(&words(&queries)),
            Binding::Output(queries.len() as u32),
            Binding::Uniform(&[queries.len() as u32, 0, 0, 0]),
        ],
        [(queries.len() as u32).div_ceil(64), 1, 1],
    )?;
    let got = |index: usize| f32::from_bits(outputs[0][index]);
    for (index, expected) in [0.0, 0.5, 1.0, 1.0, 0.0, 0.0].iter().enumerate() {
        check.that((got(index) - expected).abs() < 1e-6, || {
            format!("query {index}: {} expected {expected}", got(index))
        });
    }
    // The submerged share of a sphere is the integral of its slices' areas up to the depth,
    // over its volume (Simpson's rule, 2,000 slices, in f64).
    let integral = |depth: f64, r: f64| {
        let h = depth.clamp(0.0, 2.0 * r);
        let slices = 2000;
        let dy = h / f64::from(slices);
        let area = |y: f64| std::f64::consts::PI * (r * r - (r - y).powi(2)).max(0.0);
        let mut sum = area(0.0) + area(h);
        for k in 1..slices {
            sum += area(f64::from(k) * dy) * if k % 2 == 1 { 4.0 } else { 2.0 };
        }
        sum * dy / 3.0 / (4.0 / 3.0 * std::f64::consts::PI * r.powi(3))
    };
    let mut worst = 0.0f64;
    for (index, query) in queries.iter().enumerate().skip(6) {
        let expected = integral(f64::from(query[0]), f64::from(query[1]));
        worst = worst.max((f64::from(got(index)) - expected).abs());
    }
    check.that(worst < 1e-6, || {
        format!("cap differs from the integral by {worst}")
    });
    check.metric("worst_error", worst);
    Ok(())
}

/// `apportion_is_exact_and_respects_capacity`, for the GPU's cumulative split: every total over
/// the baseline's capacities and over full liquid columns sums exactly and fits every slot, and
/// equals world-model's split; the 32-bit multiply-divide is exact at its limits.
pub fn apportionment(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let probe_assembly = assembly("apportion probe", APPORTION_PROBE);
    let limit = (1u32 << 21) - 1;
    let triples: Vec<[u32; 3]> = vec![
        [limit, limit, limit],
        [limit - 1, limit, limit],
        [1_034_775, 517_000, 1_034_775],
        [12_345, 999_999, 1_000_003],
        [0, 7, 9],
        [1023, 1025, 2048],
        [2047, 2049, 2_000_000],
        [1, 1, 1],
    ];
    let flat: Vec<u32> = triples.iter().flatten().copied().collect();
    let outputs = probe::run(
        &mut driver.world,
        &probe_assembly,
        "probe_mul_div",
        &[
            Binding::Storage(&flat),
            Binding::Output(triples.len() as u32),
            Binding::Uniform(&[triples.len() as u32, 0, 0, 0]),
        ],
        [1, 1, 1],
    )?;
    for (index, [a, b, c]) in triples.iter().enumerate() {
        let exact = (u64::from(*a) * u64::from(*b) / u64::from(*c)) as u32;
        check.equal(&format!("{a} * {b} / {c}"), outputs[0][index], exact);
    }
    let cases: [(&str, Vec<u32>); 3] = [
        ("baseline capacities", vec![3, 1, 7, 0, 2, 5]),
        ("full columns", vec![511; 9]),
        ("one slot", vec![12]),
    ];
    for (label, capacities) in cases {
        let total: u32 = capacities.iter().sum();
        let count = capacities.len() as u32;
        let outputs = probe::run(
            &mut driver.world,
            &probe_assembly,
            "probe_shares",
            &[
                Binding::Storage(&capacities),
                Binding::Output((total + 1) * count),
                Binding::Uniform(&[count, total, 0, 0]),
            ],
            [(total + 1).div_ceil(64), 1, 1],
        )?;
        let mut wrong = 0;
        for moved in 0..=total {
            let shares = &outputs[0][(moved * count) as usize..((moved + 1) * count) as usize];
            let sum: u32 = shares.iter().sum();
            let fits = shares
                .iter()
                .zip(&capacities)
                .all(|(share, capacity)| share <= capacity);
            if sum != moved || !fits || shares != world_model::body::apportion(moved, &capacities) {
                wrong += 1;
            }
        }
        check.equal(&format!("{label}: totals split wrongly"), wrong, 0);
    }
    Ok(())
}
