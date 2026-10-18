// The bindings of the body commands and ray queries, which run in the tick's command pass in
// command order with the brush strokes and column queries.

@group(0) @binding(0) var<uniform> tick: TickParams;
@group(0) @binding(1) var<uniform> dispatch_params: DispatchParams;
@group(0) @binding(2) var<uniform> body_params: BodyParams;
@group(0) @binding(3) var<storage, read> commands: array<CommandRecord>;
@group(0) @binding(4) var<storage, read_write> results: array<ResultRecord>;
@group(0) @binding(5) var<storage, read> cells: array<vec2<u32>>;
@group(0) @binding(6) var<storage, read> window_slots: array<u32>;
@group(0) @binding(7) var<storage, read_write> bodies: array<u32>;
@group(0) @binding(8) var<storage, read_write> activity: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read_write> diagnostics: array<atomic<u32>>;

fn state_base(b: u32) -> u32 {
    return body_params.state_base + b * BODY_STATE_WORDS;
}

fn config_base(b: u32) -> u32 {
    return body_params.config_base + b * BODY_CONFIG_WORDS;
}
