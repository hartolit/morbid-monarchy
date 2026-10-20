// What one presented frame draws with, written by the CPU once per frame (`Frame`), and where
// the camera stands, computed on the device from the live body state (`View`, camera.wgsl).
//
// Coordinates are window-local render space: x is the cell x, z is minus the cell y and y is
// up, with the origin at the first cell of the window's minimum chunk, as the GPU owner's body
// kernels place bodies. Must match `FrameUniform` and `ViewRecord` in src/frame.rs.

struct Frame {
    // xy: the window's minimum chunk key, zw: the window's size in chunks.
    window: vec4<i32>,
    // xy: the window's size in cells, z: elevation scale (world units per unit), w: seconds.
    window_cells: vec4<f32>,
    // xy: brush cursor cell (window-local), z: cursor radius in cells (< 0 hides it).
    cursor: vec4<f32>,
    // xyz: unit vector towards the sun, w: sun illuminance (lux).
    sun_direction: vec4<f32>,
    // rgb: linear sun colour, w: ambient sky brightness.
    sun_color: vec4<f32>,
    // rgb: linear zenith colour, w: exposure.
    sky_color: vec4<f32>,
    // rgb: linear ground bounce colour, w: heightfield terrain shadow strength.
    ground_color: vec4<f32>,
    // rgb: linear horizon colour (fog and sky), w: ambient occlusion strength.
    horizon_color: vec4<f32>,
    // rgb: linear sun glow colour in the fog, w: its opacity.
    sun_glow: vec4<f32>,
    // x, y: fog start and end as fractions of the distance to the window edge, z: glow
    // exponent, w: half the window's shorter side in world units.
    fog: vec4<f32>,
    // xyz: free camera eye, w: 1 when the camera follows a body.
    eye: vec4<f32>,
    // x: yaw, y: pitch (radians), z: tether distance, w: vertical field of view (radians).
    angles: vec4<f32>,
    // x: aspect ratio, y: near plane, z: far plane.
    projection: vec4<f32>,
    // x: followed body slot, y: its generation, z: body slots in use (extent), w: overlay bits.
    follow: vec4<u32>,
    // x: word offset of the body configurations, y: word offset of the activity tile records,
    // z: 1 to draw every resident chunk (the unculled comparison path), 0 to draw the visible.
    offsets: vec4<u32>,
    // The sun's orthographic view-projection for the sphere shadow map.
    shadow_view_proj: mat4x4<f32>,
    // x: shadow map texel size, y: depth bias, z: 1 when sphere shadows are drawn.
    shadow: vec4<f32>,
}

struct View {
    view_proj: mat4x4<f32>,
    // xyz: eye, w: fog start distance.
    eye: vec4<f32>,
    // x: fog end distance, y: 1 when the camera follows a live body, zw: unused.
    fog: vec4<f32>,
    // xyz: the followed body's centre, w: its radius.
    focus: vec4<f32>,
}
