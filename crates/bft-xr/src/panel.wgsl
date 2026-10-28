// The VR UI panel and reticle: the texture as it is, no lighting, no fog.

#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var panel_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var panel_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(panel_texture, panel_sampler, in.uv);
}
