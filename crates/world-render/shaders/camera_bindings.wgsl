// The camera pass's bindings: the frame, the view it writes, and the body state it reads.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read_write> view: View;
@group(0) @binding(2) var<storage, read> bodies: array<u32>;
