// The bindings every world kernel shares (one bind group, one pipeline layout), and the
// helpers that address resident cells.
//
// Coordinates are window-local cells. The window map holds one slot index per window chunk,
// row-major, or SLOT_NONE for a chunk that is not resident; such a chunk is a closed boundary,
// never the contents of a recycled slot.

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

// The slot holding window-local cell `local`, or SLOT_NONE.
fn resident_slot(local: vec2<i32>) -> u32 {
    let extent = vec2<i32>(vec2<u32>(tick.window_width, tick.window_height) * CHUNK_SIZE);
    if (any(local < vec2<i32>(0)) || any(local >= extent)) {
        return SLOT_NONE;
    }
    let chunk = vec2<u32>(local) / CHUNK_SIZE;
    return window_slots[chunk.y * tick.window_width + chunk.x];
}

// Index of window-local cell `local` inside `slot`.
fn cell_index(slot: u32, local: vec2<i32>) -> u32 {
    let within = vec2<u32>(local) % CHUNK_SIZE;
    return slot * CHUNK_CELLS + within.y * CHUNK_SIZE + within.x;
}

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
