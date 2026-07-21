#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}
#import "shaders/landscape/common.wgsl"::{VertexInput, VertexOutput, process_landscape_vertex, process_landscape_fragment}

@vertex
fn vertex(in: VertexInput, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    return process_landscape_vertex(in, instance_index, 1u);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return process_landscape_fragment(in);
}
