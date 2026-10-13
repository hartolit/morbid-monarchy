// flood_events: pressure injection. Appends `tick.injected_events` cosmetic events through the
// same push_event the physical kernels use, to prove that a full event buffer reports its
// drops. Compiled and dispatched only in sessions opened with fault injection enabled.

@compute @workgroup_size(64, 1, 1)
fn flood_events(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= tick.injected_events) {
        return;
    }
    push_event(EVENT_INJECTED, id.x, tick.tick_lo, tick.tick_hi);
}
