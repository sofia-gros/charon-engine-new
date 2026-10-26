struct Params { extent:vec4<u32>, budget:vec4<u32>, geometry:vec4<f32>, sky_falloff:vec4<f32>, ambient:vec4<f32> }
struct Optical {transport:vec4<f32>,source:vec4<f32>,surface:vec4<f32>}
struct Hit {index:u32,pos:vec2<f32>,normal:vec2<f32>,trans:vec3<f32>,distance:f32,escaped:bool,found:bool}
@group(0) @binding(0) var<uniform> params:Params;
@group(0) @binding(1) var<storage,read> optics:array<Optical>;
@group(0) @binding(2) var<storage,read> emitters:array<u32>;
@group(0) @binding(3) var<storage,read_write> history:array<vec4<f32>>;
@group(0) @binding(4) var<storage,read_write> result:array<vec4<f32>>;
@group(0) @binding(5) var image:texture_storage_2d<rgba8unorm,write>;
const USE_RTX:bool = false;
fn empty_hit(p:vec2<f32>,distance:f32)->Hit {return Hit(0xffffffffu,p,vec2<f32>(0.),vec3<f32>(1.),distance,false,false);}
fn random(state:ptr<function,u32>)->f32 {
 *state=*state^(*state<<13u);*state=*state^(*state>>17u);*state=*state^(*state<<5u);return f32(*state>>8u)/16777216.;
}
fn walk(p:vec2<f32>,d:vec2<f32>,range:f32)->Hit {
 var cell=vec2<i32>(floor(p));var trans=vec3<f32>(1.);
 let step=vec2<i32>(select(vec2<f32>(1.),vec2<f32>(-1.),d<vec2<f32>(0.)));
 let delta=select(vec2<f32>(1.e20),1./max(abs(d),vec2<f32>(1.e-20)),abs(d)>=vec2<f32>(1.e-7));
 var edge=select((p-vec2<f32>(cell))*delta,(vec2<f32>(cell)+vec2<f32>(1.)-p)*delta,step>vec2<i32>(0));
 var hardware=empty_hit(p,range);
 let start=u32(cell.y)*params.extent.x+u32(cell.x);
 let use_hardware=USE_RTX && optics[start].transport.w<0.999;
 if use_hardware {
  hardware=opaque_hit(p,d,range);
  // Opaque/air scenes use hardware traversal directly. Grid stepping is only
  // needed to integrate partially transmitting materials such as water.
  if params.budget.y==0u {
   if hardware.found {return hardware;}
   let borders=select((vec2<f32>(0.)-p)/d,(vec2<f32>(params.extent.xy)-p)/d,d>=vec2<f32>(0.));
   let exits=select(vec2<f32>(1.e20),borders,abs(d)>=vec2<f32>(1.e-7));
   let distance=min(exits.x,exits.y);
   if distance<range {var h=empty_hit(p+d*distance,distance);h.escaped=true;return h;}
   return empty_hit(p+d*range,range);
  }
 }
 for(var iteration=0u;iteration<params.budget.z;iteration++) {
  let axis=select(0u,1u,edge.y<edge.x);let t=edge[axis];
  if t>=range {var h=empty_hit(p+d*range,range);h.trans=trans;return h;}
  cell[axis]+=step[axis];edge[axis]+=delta[axis];
  var normal=vec2<f32>(0.);normal[axis]=-f32(step[axis]);let pos=p+d*t;
  if any(cell<vec2<i32>(0)) || any(cell>=vec2<i32>(params.extent.xy)) {var h=empty_hit(pos,t);h.escaped=true;h.trans=trans;return h;}
  let index=u32(cell.y)*params.extent.x+u32(cell.x);let optic=optics[index];
  if optic.transport.w>=0.999 {
   if use_hardware && hardware.found {hardware.trans=trans;return hardware;}
   return Hit(index,pos,normal,trans,t,false,true);
  }
  trans*=optic.transport.xyz*(1.-optic.transport.w);
 }
 var exhausted=empty_hit(p,0.);exhausted.trans=vec3<f32>(0.);return exhausted;
}
fn visibility(p:vec2<f32>,q:vec2<f32>)->vec3<f32> {
 let v=q-p;let distance=length(v);if distance<0.00001{return vec3<f32>(1.);}
 let hit=walk(p,v/distance,distance);let target_cell=u32(floor(q.y))*params.extent.x+u32(floor(q.x));
 if hit.distance>=distance-0.001 || (hit.found && hit.index==target_cell) {return hit.trans;}return vec3<f32>(0.);
}
// Primary illumination is deterministic even when edits reset a one-sample
// Lite frame. Only secondary diffuse transport uses Monte Carlo samples.
fn stable_primary(pos:vec2<f32>,range:f32)->vec3<f32> {
 var value=vec3<f32>(0.);let count=min(params.budget.w,16u);
 for(var j=0u;j<count;j++) {
  let source=emitters[min(u32((f32(j)+0.5)*f32(params.budget.w)/f32(count)),params.budget.w-1u)];
  let q=vec2<f32>(f32(source%params.extent.x)+0.5,f32(source/params.extent.x)+0.5);let dist=length(q-pos);
  if dist>0.001 && dist<=range {value+=visibility(pos,q)*optics[source].source.rgb*f32(params.budget.w)/f32(count)*pow(params.sky_falloff.w,dist*params.geometry.z)/(1.+dist);}
 }
 if max(params.sky_falloff.x,max(params.sky_falloff.y,params.sky_falloff.z))>0. {
  for(var j=0u;j<16u;j++) {
   let a=3.14159265359+(f32(j)+0.5)*3.14159265359/16.;let d=vec2<f32>(cos(a),sin(a));let h=walk(pos,d,range);
   if h.escaped {value+=h.trans*params.sky_falloff.rgb*pow(params.sky_falloff.w,h.distance*params.geometry.z)/32.;}
  }
 }
 return value;
}
fn sample_path(index:u32,sample_index:u32)->vec3<f32> {
 var state=(index*747796405u)^(sample_index*2891336453u)^0x9e3779b9u;if state==0u{state=1u;}
 var pos=vec2<f32>(f32(index%params.extent.x)+0.5,f32(index/params.extent.x)+0.5);
 let angle=random(&state)*6.28318530718;var dir=vec2<f32>(cos(angle),sin(angle));
 var radiance=optics[index].source.rgb+stable_primary(pos,params.geometry.w/params.geometry.z);var throughput=vec3<f32>(1.);var range=params.geometry.w/params.geometry.z;
 for(var bounce=0u;bounce<=params.extent.w;bounce++) {
  if bounce>0u && params.budget.w>0u {
   let source=emitters[min(u32(random(&state)*f32(params.budget.w)),params.budget.w-1u)];
   let q=vec2<f32>(f32(source%params.extent.x)+0.5,f32(source/params.extent.x)+0.5);let dist=length(q-pos);
   if dist>0.001 && dist<=range {
    let trans=visibility(pos,q);let weight=f32(params.budget.w)*pow(params.sky_falloff.w,dist*params.geometry.z)/(1.+dist);
    radiance+=throughput*trans*optics[source].source.rgb*weight;
   }
  }
  let hit=walk(pos,dir,range);throughput*=hit.trans*pow(params.sky_falloff.w,hit.distance*params.geometry.z);
  if hit.escaped {if bounce>0u && dir.y<0.{radiance+=throughput*params.sky_falloff.rgb;}break;}
  if !hit.found || bounce==params.extent.w {break;}
  throughput*=optics[hit.index].surface.rgb;range-=hit.distance;if range<0.001{break;}
  pos=hit.pos+hit.normal*0.001;
  let tangent=vec2<f32>(-hit.normal.y,hit.normal.x);let s=random(&state)*2.-1.;let c=sqrt(1.-s*s);dir=hit.normal*c+tangent*s;
 }
 return radiance;
}
@compute @workgroup_size(8,8)
fn trace(@builtin(global_invocation_id) id:vec3<u32>) {
 if any(id.xy>=params.extent.xy) {return;}
 let index=id.y*params.extent.x+id.x;var sum=vec3<f32>(0.);
 if params.budget.x>0u {sum=history[index].rgb;}
 for(var sample=0u;sample<params.extent.z;sample++) {sum+=sample_path(index,params.budget.x+sample);}
 history[index]=vec4<f32>(sum,0.);
 let ao=optics[index].surface.a;
 let rgb=clamp(max((sum/f32(params.budget.x+params.extent.z)+params.ambient.rgb)*ao,optics[index].source.rgb),vec3<f32>(0.),vec3<f32>(1.));
 let value=vec4<f32>(rgb,ao);result[index]=value;textureStore(image,vec2<i32>(id.xy),value);
}
