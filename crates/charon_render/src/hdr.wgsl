struct Out { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@group(0) @binding(0) var source:texture_2d<f32>;
@group(0) @binding(1) var source_sampler:sampler;
@group(0) @binding(2) var bloom:texture_2d<f32>;
@group(0) @binding(3) var<uniform> settings:vec4<f32>;
@vertex fn vs(@builtin(vertex_index) i:u32)->Out {
 let uv=vec2<f32>(f32((i<<1u)&2u),f32(i&2u));var out:Out;out.position=vec4<f32>(uv*vec2<f32>(2.,-2.)+vec2<f32>(-1.,1.),0.,1.);out.uv=uv;return out;
}
fn bright(c:vec3<f32>)->vec3<f32> {let l=max(c.x,max(c.y,c.z));return c*max(l-settings.z,0.)/max(l,0.00001);}
@fragment fn bloom_x(in:Out)->@location(0) vec4<f32> {
 let texel=1./vec2<f32>(textureDimensions(source));var sum=vec3<f32>(0.);var total=0.;
 for(var i=-4;i<=4;i++){let weight=exp(-f32(i*i)/8.);let c=textureSample(source,source_sampler,in.uv+vec2<f32>(f32(i)*texel.x*4.,0.)).rgb;sum+=bright(c)*weight;total+=weight;}
 return vec4<f32>(sum/total,1.);
}
@fragment fn bloom_y(in:Out)->@location(0) vec4<f32> {
 let texel=1./vec2<f32>(textureDimensions(source));var sum=vec3<f32>(0.);var total=0.;
 for(var i=-4;i<=4;i++){let weight=exp(-f32(i*i)/8.);sum+=textureSample(source,source_sampler,in.uv+vec2<f32>(0.,f32(i)*texel.y)).rgb*weight;total+=weight;}
 return vec4<f32>(sum/total,1.);
}
fn aces(c:vec3<f32>)->vec3<f32> {return clamp((c*(2.51*c+0.03))/(c*(2.43*c+0.59)+0.14),vec3<f32>(0.),vec3<f32>(1.));}
@fragment fn present(in:Out)->@location(0) vec4<f32> {
 let c=textureSample(source,source_sampler,in.uv).rgb+textureSample(bloom,source_sampler,in.uv).rgb*settings.y;
 return vec4<f32>(aces(c*settings.x),1.);
}
