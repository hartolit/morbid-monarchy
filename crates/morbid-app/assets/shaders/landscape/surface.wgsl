#import "shaders/landscape/common.wgsl"::{VertexInput, VertexOutput, LAYER_SURFACE, process_landscape_vertex, shade_solid_layer}

@vertex
fn vertex(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    return process_landscape_vertex(in, instance_index, LAYER_SURFACE);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return shade_solid_layer(in);
}
