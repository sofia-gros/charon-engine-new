struct Optical { transport:vec4<f32>, emission:vec4<f32> };
struct Params { extent:vec4<u32>, quality:vec4<f32>, sky:vec4<f32>, ambient:vec4<f32> };
@group(0) @binding(0) var<storage,read> optics:array<Optical>;
@group(0) @binding(1) var<storage,read> sources:array<vec4<f32>>;
@group(0) @binding(2) var<storage,read_write> base:array<vec4<f32>>;
@group(0) @binding(3) var<storage,read> previous:array<vec4<f32>>;
@group(0) @binding(4) var<storage,read_write> next:array<vec4<f32>>;
@group(0) @binding(5) var<storage,read_write> result:array<vec4<f32>>;
@group(0) @binding(6) var output:texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(7) var<uniform> params:Params;
@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id:vec3<u32>){
 let x=id.x;if x>=params.extent.x{return;}
 var sky=params.sky.rgb;
 for(var y=0u;y<params.extent.y;y++){
  let i=y*params.extent.x+x;let light=max(sources[i].rgb,sky);
  base[i]=vec4<f32>(light,0.0);next[i]=base[i];
  sky*=optics[i].transport.yzw*(1.0-optics[i].transport.x);
 }
}
fn incoming(x:i32,y:i32)->vec3<f32>{
 if x<0||y<0||x>=i32(params.extent.x)||y>=i32(params.extent.y){return vec3<f32>(0.0);}
 let i=u32(y)*params.extent.x+u32(x);
 return max(previous[i].rgb*optics[i].transport.yzw*(1.0-optics[i].transport.x),sources[i].rgb)*params.quality.x;
}
@compute @workgroup_size(64)
fn propagate(@builtin(global_invocation_id) id:vec3<u32>){
 let i=id.x;if i>=params.extent.x*params.extent.y{return;}
 let x=i32(i%params.extent.x);let y=i32(i/params.extent.x);
 let light=max(base[i].rgb,max(max(incoming(x-1,y),incoming(x+1,y)),max(incoming(x,y-1),incoming(x,y+1))));
 next[i]=vec4<f32>(light,0.0);
}
@compute @workgroup_size(64)
fn finish(@builtin(global_invocation_id) id:vec3<u32>){
 let i=id.x;if i>=params.extent.x*params.extent.y{return;}
 let x=i32(i%params.extent.x);let y=i32(i/params.extent.x);var opacity=0.0;
 for(var dy=-1;dy<=1;dy++){for(var dx=-1;dx<=1;dx++){
  if dx==0&&dy==0{continue;}
  let nx=x+dx;let ny=y+dy;
  if nx>=0&&ny>=0&&nx<i32(params.extent.x)&&ny<i32(params.extent.y){opacity+=optics[u32(ny)*params.extent.x+u32(nx)].transport.x;}
 }}
 let ao=1.0-params.quality.y*opacity/8.0;
 let light=clamp(max((previous[i].rgb+params.ambient.rgb)*ao,sources[i].rgb),vec3<f32>(0.0),vec3<f32>(1.0));
 result[i]=vec4<f32>(light,ao);textureStore(output,vec2<i32>(x,y),vec4<f32>(light,1.0));
}
