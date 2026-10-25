struct VertexOut { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) tint:vec4<f32> };
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var image_sampler:sampler;
// Screen rectangle of the entire lighting domain, including any off-screen halo.
@group(1) @binding(0) var<uniform> domain:vec4<f32>;
@group(2) @binding(0) var illumination:texture_2d<f32>;
@group(2) @binding(1) var light_sampler:sampler;
@vertex fn vs_main(@location(0) position:vec2<f32>,@location(1) uv:vec2<f32>,@location(2) tint:vec4<f32>)->VertexOut {
 var out:VertexOut;out.position=vec4<f32>(position,0.0,1.0);out.uv=uv;out.tint=tint;return out;
}
@fragment fn fs_main(in:VertexOut)->@location(0) vec4<f32> {
 let color=textureSample(image,image_sampler,in.uv)*in.tint;
 let uv=(in.position.xy-domain.xy)/domain.zw;
 let light=textureSample(illumination,light_sampler,uv).rgb;
 return vec4<f32>(color.rgb*light,color.a);
}
