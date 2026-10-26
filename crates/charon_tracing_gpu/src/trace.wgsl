struct Params { extent:vec4<u32>, budget:vec4<u32>, geometry:vec4<f32>, sky_falloff:vec4<f32>, ambient:vec4<f32> }
struct Emitter {position:vec4<f32>,color:vec4<f32>}
struct RawLight {direct:vec4<f32>,indirect:vec4<f32>}
struct Optical {transport:vec4<f32>,source:vec4<f32>,surface:vec4<f32>}
struct Hit {index:u32,pos:vec2<f32>,normal:vec2<f32>,trans:vec3<f32>,distance:f32,escaped:bool,found:bool}
@group(0) @binding(0) var<uniform> params:Params;
@group(0) @binding(1) var<storage,read> optics:array<Optical>;
@group(0) @binding(2) var<storage,read> emitters:array<Emitter>;
@group(0) @binding(3) var<storage,read_write> history:array<vec4<f32>>;
@group(0) @binding(4) var<storage,read_write> result:array<vec4<f32>>;
@group(0) @binding(5) var image:texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(6) var<storage,read_write> raw:array<RawLight>;
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
const SKY_DIRECTIONS:array<vec2<f32>,5>=array<vec2<f32>,5>(vec2<f32>(-0.8660254,-0.5),vec2<f32>(-0.5,-0.8660254),vec2<f32>(0.,-1.),vec2<f32>(0.5,-0.8660254),vec2<f32>(0.8660254,-0.5));
fn position_of(index:u32)->vec2<f32> {return vec2<f32>(f32(index%params.extent.x)+0.5,f32(index/params.extent.x)+0.5);}
fn direct_light(p:vec2<f32>,normal:vec2<f32>,range:f32)->vec3<f32> {
 var light=vec3<f32>(0.);
 for(var i=0u;i<params.budget.w;i++) {
  let emitter=emitters[i];let q=(emitter.position.xy-params.geometry.xy)/params.geometry.z;
  let v=q-p;let distance=length(v);if distance>range {continue;}
  var cosine=1.;if any(normal!=vec2<f32>(0.))&&distance>=0.00001 {cosine=max(dot(normal,v)/distance,0.);}
  let trans=visibility(p,q);let world=distance*params.geometry.z;
  let weight=cosine*pow(params.sky_falloff.w,world)/(1.+world/16.);
  light+=trans*emitter.color.rgb*weight;
 }
 for(var i=0u;i<5u;i++) {
  let d=SKY_DIRECTIONS[i];var cosine=1.;if any(normal!=vec2<f32>(0.)){cosine=max(dot(normal,d),0.);}
  let hit=walk(p,d,range);
  if hit.escaped {light+=hit.trans*params.sky_falloff.rgb*(0.2*cosine*pow(params.sky_falloff.w,hit.distance*params.geometry.z));}
 }
 return light;
}
fn sample_path(index:u32,sample_index:u32)->vec3<f32> {
 if params.extent.w==0u || optics[index].transport.w>=0.999 {return vec3<f32>(0.);}
 var state=(index*747796405u)^(sample_index*2891336453u)^0x9e3779b9u;if state==0u{state=1u;}
 var pos=position_of(index);let angle=random(&state)*6.28318530718;var dir=vec2<f32>(cos(angle),sin(angle));
 var radiance=vec3<f32>(0.);var throughput=vec3<f32>(1.);var range=params.geometry.w/params.geometry.z;
 for(var bounce=0u;bounce<params.extent.w;bounce++) {
  let hit=walk(pos,dir,range);if !hit.found {break;}
  range-=hit.distance;if range<=0.001{break;}
  throughput*=hit.trans*optics[hit.index].surface.rgb*pow(params.sky_falloff.w,hit.distance*params.geometry.z);
  pos=hit.pos+hit.normal*0.001;
  radiance+=throughput*direct_light(pos,hit.normal,range);
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
 raw[index].direct=vec4<f32>(direct_light(position_of(index),vec2<f32>(0.),params.geometry.w/params.geometry.z),optics[index].surface.a);
 raw[index].indirect=vec4<f32>(sum/f32(params.budget.x+params.extent.z),0.);
}
@compute @workgroup_size(8,8)
fn resolve(@builtin(global_invocation_id) id:vec3<u32>) {
 if any(id.xy>=params.extent.xy) {return;}
 let index=id.y*params.extent.x+id.x;let p=position_of(index);var indirect=raw[index].indirect.rgb;var weight=1.;
 if params.extent.w>0u && optics[index].transport.w<0.999 {
  for(var dy=-2;dy<=2;dy++) {for(var dx=-2;dx<=2;dx++) {
   if dx==0&&dy==0{continue;}let q=vec2<i32>(id.xy)+vec2<i32>(dx,dy);
   if any(q<vec2<i32>(0))||any(q>=vec2<i32>(params.extent.xy)){continue;}
   let j=u32(q.y)*params.extent.x+u32(q.x);if optics[j].transport.w>=0.999{continue;}
   if any(visibility(p,position_of(j))<vec3<f32>(0.05)){continue;}
   let w=1./(1.+f32(dx*dx+dy*dy));indirect+=raw[j].indirect.rgb*w;weight+=w;
  }}
 }
 let ao=raw[index].direct.a;
 let rgb=clamp(max((raw[index].direct.rgb+indirect/weight+params.ambient.rgb)*ao,optics[index].source.rgb),vec3<f32>(0.),vec3<f32>(1.));
 let value=vec4<f32>(rgb,ao);result[index]=value;textureStore(image,vec2<i32>(id.xy),value);
}
