// Bloom and tone mapping of the HDR frame. The bloom is a mip chain: the first downsample uses
// a 13-tap filter with a Karis average (so a single bright pixel does not flicker), later ones
// the plain 13-tap filter, and the upsamples a 3 x 3 tent, each blended into the next larger
// mip with a per-mip constant (Bevy's energy-conserving `Bloom::NATURAL` settings, which the
// baseline used). The last upsample blends into the HDR frame itself. Tone mapping is AgX (the
// baseline used Bevy's TonyMcMapface, which needs a lookup texture; the difference is recorded
// with the visual comparison), followed by sRGB encoding when the output format does not do it.

struct PostParams {
    // xy: texel size of the source, z: unused, w: 1 to encode sRGB in the shader.
    texel: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> post: PostParams;

struct FullscreenOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle covering the screen.
@vertex
fn fullscreen_vertex(@builtin(vertex_index) index: u32) -> FullscreenOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: FullscreenOut;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn karis_weight(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luminance(c));
}

fn tap(uv: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSampleLevel(source, source_sampler, uv + vec2<f32>(x, y) * post.texel.xy, 0.0).rgb;
}

// The 13-tap downsample (Jimenez, "Next Generation Post Processing in Call of Duty: Advanced
// Warfare"): five overlapping 2 x 2 boxes.
fn downsample13(uv: vec2<f32>, karis: bool) -> vec3<f32> {
    let a = tap(uv, -2.0, 2.0);
    let b = tap(uv, 0.0, 2.0);
    let c = tap(uv, 2.0, 2.0);
    let d = tap(uv, -2.0, 0.0);
    let e = tap(uv, 0.0, 0.0);
    let f = tap(uv, 2.0, 0.0);
    let g = tap(uv, -2.0, -2.0);
    let h = tap(uv, 0.0, -2.0);
    let i = tap(uv, 2.0, -2.0);
    let j = tap(uv, -1.0, 1.0);
    let k = tap(uv, 1.0, 1.0);
    let l = tap(uv, -1.0, -1.0);
    let m = tap(uv, 1.0, -1.0);
    if (karis) {
        let g0 = (a + b + d + e) * 0.25;
        let g1 = (b + c + e + f) * 0.25;
        let g2 = (d + e + g + h) * 0.25;
        let g3 = (e + f + h + i) * 0.25;
        let g4 = (j + k + l + m) * 0.25;
        let w0 = karis_weight(g0) * 0.125;
        let w1 = karis_weight(g1) * 0.125;
        let w2 = karis_weight(g2) * 0.125;
        let w3 = karis_weight(g3) * 0.125;
        let w4 = karis_weight(g4) * 0.5;
        return (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / max(w0 + w1 + w2 + w3 + w4, 0.0001);
    }
    var sample = (a + c + g + i) * 0.03125;
    sample += (b + d + f + h) * 0.0625;
    sample += (e + j + k + l + m) * 0.125;
    return sample;
}

@fragment
fn downsample_first(in: FullscreenOut) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample13(in.uv, true), 1.0);
}

@fragment
fn downsample(in: FullscreenOut) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample13(in.uv, false), 1.0);
}

// The 3 x 3 tent upsample; the pipeline blends it into the target with the mip's constant.
@fragment
fn upsample(in: FullscreenOut) -> @location(0) vec4<f32> {
    var sample = tap(in.uv, 0.0, 0.0) * 4.0;
    sample += (tap(in.uv, -1.0, 0.0) + tap(in.uv, 1.0, 0.0) + tap(in.uv, 0.0, -1.0) + tap(in.uv, 0.0, 1.0)) * 2.0;
    sample += tap(in.uv, -1.0, -1.0) + tap(in.uv, 1.0, -1.0) + tap(in.uv, -1.0, 1.0) + tap(in.uv, 1.0, 1.0);
    return vec4<f32>(sample / 16.0, 1.0);
}

// AgX: Troy Sobotka's transform with the polynomial contrast fit by Benjamin Wrensch (MIT).
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx_tonemap(color: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116),
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * max(color, vec3<f32>(1.0e-10));
    v = clamp(log2(v), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = agx_contrast(v);
    v = outset * v;
    // The fit's output is display-encoded; return linear light for an sRGB target.
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@fragment
fn tonemap(in: FullscreenOut) -> @location(0) vec4<f32> {
    let hdr = textureSampleLevel(source, source_sampler, in.uv, 0.0).rgb;
    var color = agx_tonemap(hdr);
    if (post.texel.w > 0.5) {
        color = srgb_encode(saturate(color));
    }
    return vec4<f32>(color, 1.0);
}
