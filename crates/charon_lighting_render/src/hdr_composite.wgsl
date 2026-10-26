struct VertexOut { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) tint:vec4<f32> };
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var image_sampler:sampler;
@group(1) @binding(0) var<uniform> domain:vec4<f32>;
@group(2) @binding(0) var illumination:texture_2d<f32>;
@group(2) @binding(1) var light_sampler:sampler;
@vertex fn vs_main(@location(0) position:vec2<f32>,@location(1) uv:vec2<f32>,@location(2) tint:vec4<f32>)->VertexOut {
 var out:VertexOut;out.position=vec4<f32>(position,0.,1.);out.uv=uv;out.tint=tint;return out;
}
fn load(xy:vec2<i32>,plane:i32,size:vec2<i32>)->vec4<f32> {return textureLoad(illumination,clamp(xy,vec2<i32>(0),size-vec2<i32>(1))+vec2<i32>(size.x*plane,0),0);}
@fragment fn fs_main(in:VertexOut)->@location(0) vec4<f32> {
 let color=textureSample(image,image_sampler,in.uv)*in.tint;
 let uv=(in.position.xy-domain.xy)/domain.zw;
 let dim=vec2<i32>(textureDimensions(illumination));let size=vec2<i32>(dim.x/4,dim.y);let p=uv*vec2<f32>(size)-vec2<f32>(0.5);let cell=vec2<i32>(floor(p));
 let nearest=vec2<i32>(floor(uv*vec2<f32>(size)));let guide=load(nearest,3,size);var diffuse=vec3<f32>(0.);var specular=vec3<f32>(0.);var total=0.;let f=fract(p);
 for(var y=0;y<=1;y++){for(var x=0;x<=1;x++) {
  let q=cell+vec2<i32>(x,y);let other=load(q,3,size);
  if abs(guide.a-other.a)>0.1 || dot(guide.xyz,other.xyz)<0.95{continue;}
  if q.x!=nearest.x && q.y!=nearest.y {
   let a=load(vec2<i32>(q.x,nearest.y),3,size);let b=load(vec2<i32>(nearest.x,q.y),3,size);
   if abs(a.a-guide.a)>0.1 || abs(b.a-guide.a)>0.1 {continue;}
  }
  let weight=select(1.-f.x,f.x,x==1)*select(1.-f.y,f.y,y==1);
  diffuse+=load(q,0,size).rgb*weight;specular+=load(q,1,size).rgb*weight;total+=weight;
 }}
 if total>0.00001 {diffuse/=total;specular/=total;}else{diffuse=load(nearest,0,size).rgb;specular=load(nearest,1,size).rgb;}
 // Emission belongs to the particle itself, never to its dark albedo. Do not
 // duplicate it on transparent layers or overlay sprites.
 let emission=load(nearest,2,size).rgb;
 return vec4<f32>(color.rgb*diffuse+specular+emission,color.a);
}
