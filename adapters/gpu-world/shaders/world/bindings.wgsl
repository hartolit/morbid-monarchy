// The bindings every command kernel shares (one bind group, one pipeline layout). Cell and tile
// addressing is in shared/window.wgsl.

struct EventRing {
    header: EventHeader,
    records: array<EventRecord>,
}

@group(0) @binding(0) var<uniform> tick: TickParams;
@group(0) @binding(1) var<uniform> dispatch_params: DispatchParams;
@group(0) @binding(2) var<storage, read> commands: array<CommandRecord>;
@group(0) @binding(3) var<storage, read_write> cells: array<vec2<u32>>;
@group(0) @binding(4) var<storage, read> window_slots: array<u32>;
@group(0) @binding(5) var<storage, read> slot_records: array<ChunkSlotRecord>;
@group(0) @binding(6) var<storage, read_write> results: array<ResultRecord>;
@group(0) @binding(7) var<storage, read_write> events: EventRing;
@group(0) @binding(8) var<storage, read_write> activity: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read_write> diagnostics: array<atomic<u32>>;

// Appends a cosmetic event, or counts it as dropped when the tick's buffer is full. `count`
// grows with every attempt, so the consumer can check that `dropped` accounts for the excess.
fn push_event(kind: u32, a: u32, b: u32, c: u32) {
    let index = atomicAdd(&events.header.count, 1u);
    if (index < events.header.capacity) {
        events.records[index] = EventRecord(kind, a, b, c);
    } else {
        atomicAdd(&events.header.dropped, 1u);
    }
}
