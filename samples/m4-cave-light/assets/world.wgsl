struct VertexOut {
 @builtin(position) position: vec4<f32>,
 @location(0) uv: vec2<f32>,
 @location(1) tint: vec4<f32>,
};
@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var nearest_sampler: sampler;
@group(1) @binding(0) var<uniform> parameters: vec4<f32>;
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) tint: vec4<f32>) -> VertexOut {
 var out: VertexOut; out.position=vec4<f32>(position,0.0,1.0); out.uv=uv; out.tint=tint; return out;
}
@fragment fn fs_main(in:VertexOut)->@location(0) vec4<f32> {
 let color=textureSample(atlas,nearest_sampler,in.uv)*in.tint;
 return vec4<f32>(color.rgb*vec3<f32>(parameters.x, parameters.y, parameters.z),color.a);
}
