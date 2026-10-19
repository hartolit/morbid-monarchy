// Places the frame's camera on the device, before any pass draws: a free camera at the eye the
// CPU gave, or one that follows a body at the tether distance from the body's position as the
// last submitted tick left it. Following reads the GPU's own pose, so the followed sphere stays
// steady on screen with no readback and no CPU prediction; the CPU's own decisions (residency
// focus, the cursor ray) use the body's latest sample, at most two ticks old.
//
// The projection is an infinite reverse-Z perspective (depth 1 at the near plane, 0 at
// infinity). The fog band is fitted to the camera's height: the distance to the window edge
// grows with altitude, so a top-down view is not swallowed by haze meant for the horizon.

// The eye sits this far above the observer or tethered point (the baseline's lens offset).
const LENS_OFFSET_Y: f32 = 0.7;

fn rotate_y(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

fn rotate_x(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(v.x, c * v.y - s * v.z, s * v.y + c * v.z);
}

// The observer's rotation: yaw about +Y, then the lens's pitch about its own X.
fn orient(v: vec3<f32>) -> vec3<f32> {
    return rotate_y(rotate_x(v, frame.angles.y), frame.angles.x);
}

@compute @workgroup_size(1)
fn place_camera() {
    var eye = frame.eye.xyz;
    var following = 0.0;
    var focus = vec4<f32>(0.0);
    if (frame.eye.w > 0.5) {
        let body = placed_body(frame.follow.x);
        if (body.alive && body.state.generation == frame.follow.y) {
            eye = body.position + orient(vec3<f32>(0.0, 0.0, frame.angles.z));
            following = 1.0;
            focus = vec4<f32>(body.position, body.radius);
        }
    }
    eye.y += LENS_OFFSET_Y;
    let forward = orient(vec3<f32>(0.0, 0.0, -1.0));
    let right = normalize(cross(forward, vec3<f32>(0.0, 1.0, 0.0)));
    let up = cross(right, forward);
    let view_matrix = mat4x4<f32>(
        vec4<f32>(right.x, up.x, -forward.x, 0.0),
        vec4<f32>(right.y, up.y, -forward.y, 0.0),
        vec4<f32>(right.z, up.z, -forward.z, 0.0),
        vec4<f32>(-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0),
    );
    let f = 1.0 / tan(frame.angles.w * 0.5);
    let near = frame.projection.y;
    let projection = mat4x4<f32>(
        vec4<f32>(f / frame.projection.x, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, f, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, -1.0),
        vec4<f32>(0.0, 0.0, near, 0.0),
    );
    let edge_distance = length(vec2<f32>(frame.fog.w, max(eye.y, 0.0)));
    view.view_proj = projection * view_matrix;
    view.eye = vec4<f32>(eye, edge_distance * frame.fog.x);
    view.fog = vec4<f32>(edge_distance * frame.fog.y, following, 0.0, 0.0);
    view.focus = focus;
}
