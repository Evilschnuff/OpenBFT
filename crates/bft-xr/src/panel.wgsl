// The VR UI panel, HUD tiles and reticle: a part of the texture as it is,
// no lighting, no fog.

#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var panel_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var panel_sampler: sampler;
// The part of the texture the quad shows: corner in xy, size in zw.
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> uv_rect: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(panel_texture, panel_sampler, uv_rect.xy + in.uv * uv_rect.zw);
}
