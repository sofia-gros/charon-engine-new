// All positions, prism depths and extinction coefficients use world units.
struct Params { extent:vec4<u32>, budget:vec4<u32>, geometry:vec4<f32>, sky_falloff:vec4<f32>, ambient:vec4<f32>, quality:vec4<u32> }
struct Optical { transport:vec4<f32>, source:vec4<f32>, diffuse:vec4<f32>, material:vec4<f32>, absorption:vec4<f32>, normal:vec4<f32>, emission:vec4<f32> }
struct Emitter {index:u32,cdf:f32,pdf:f32,padding:f32}
struct Result { diffuse:vec4<f32>, specular:vec4<f32>, emission:vec4<f32>, guide:vec4<f32> }
struct Hit { index:u32, distance:f32, normal:vec3<f32>, position:vec3<f32>, found:bool }
struct BoxHit { distance:f32,normal:vec3<f32>,found:bool }
@group(0) @binding(0) var<uniform> params:Params;
@group(0) @binding(1) var<storage,read> optics:array<Optical>;
@group(0) @binding(2) var<storage,read> emitters:array<Emitter>;
@group(0) @binding(3) var<storage,read_write> history:array<vec4<f32>>;
@group(0) @binding(4) var<storage,read_write> result:array<Result>;
@group(0) @binding(5) var image:texture_storage_2d<rgba16float,write>;
@group(0) @binding(6) var<storage,read> dirty:array<u32>;
const USE_RTX:bool = false;
const EPS:f32 = 0.002;
fn empty_hit(p:vec3<f32>,distance:f32)->Hit {return Hit(0xffffffffu,distance,vec3<f32>(0.),p,false);}
fn unit(v:vec3<f32>)->vec3<f32>{return v/max(length(v),1.e-10);}
fn random(state:ptr<function,u32>)->f32 { *state=*state^(*state<<13u);*state=*state^(*state>>17u);*state=*state^(*state<<5u);return f32(*state>>8u)/16777216.; }
fn occupied(i:u32)->bool {return optics[i].transport.w>=0.999 || optics[i].absorption.w>0. || max(optics[i].emission.x,max(optics[i].emission.y,optics[i].emission.z))>0.;}
fn cell_index(p:vec3<f32>)->u32 {
 let xy=vec2<i32>(floor((p.xy-params.geometry.xy)/params.geometry.z));
 if any(xy<vec2<i32>(0)) || any(xy>=vec2<i32>(params.extent.xy)){return 0xffffffffu;}
 return u32(xy.y)*params.extent.x+u32(xy.x);
}
fn slab(p:vec3<f32>,d:vec3<f32>,lo:vec3<f32>,hi:vec3<f32>)->BoxHit {
 var enter=-1.e20;var exit=1.e20;var en=vec3<f32>(0.);var ex=vec3<f32>(0.);
 for(var k=0u;k<3u;k++) {
  if abs(d[k])<1.e-7 {if p[k]<lo[k] || p[k]>hi[k]{return BoxHit(0.,en,false);}continue;}
  let a=(lo[k]-p[k])/d[k];let b=(hi[k]-p[k])/d[k];let near=min(a,b);let far=max(a,b);
  if near>enter {enter=near;en=vec3<f32>(0.);en[k]=select(1.,-1.,d[k]>0.);}
  if far<exit {exit=far;ex=vec3<f32>(0.);ex[k]=select(-1.,1.,d[k]>0.);}
 }
 if exit<enter || exit<EPS{return BoxHit(0.,en,false);}
 if enter>=EPS{return BoxHit(enter,en,true);}return BoxHit(exit,ex,true);
}
fn software_hit(p:vec3<f32>,d:vec3<f32>,range:f32)->Hit {
 let size=params.geometry.z;var t=0.;let lower=params.geometry.xy;let upper=lower+vec2<f32>(params.extent.xy)*size;
 for(var k=0u;k<2u;k++) {if p[k]<lower[k] || p[k]>=upper[k] {
  if abs(d[k])<1.e-7{return empty_hit(p,range);}
  t=max(t,min((lower[k]-p[k])/d[k],(upper[k]-p[k])/d[k])+EPS);
 }}
 var cell=vec2<i32>(floor((p.xy+d.xy*t-lower)/size));
 for(var iteration=0u;iteration<params.budget.z;iteration++) {
  if any(cell<vec2<i32>(0)) || any(cell>=vec2<i32>(params.extent.xy)) || t>range {return empty_hit(p+d*range,range);}
  let i=u32(cell.y)*params.extent.x+u32(cell.x);let depth=optics[i].material.w;
  let lo=vec3<f32>(lower+vec2<f32>(cell)*size,-depth);let hi=vec3<f32>(lo.xy+vec2<f32>(size),depth);
  var edges=vec2<f32>(1.e20);
  for(var k=0u;k<2u;k++){if abs(d[k])>1.e-7{edges[k]=(select(lo[k],hi[k],d[k]>0.)-p[k])/d[k];}}
  let axis=select(1u,0u,edges.x<edges.y);let next=edges[axis];
  if occupied(i) {
   for(var attempt=0u;attempt<2u;attempt++) {
    let offset=select(0.,t+EPS*2.,attempt==1u);let box=slab(p+d*offset,d,lo,hi);let distance=box.distance+offset;
    if box.found && distance<=range && distance>=t-EPS && distance<=next+EPS {
     let at=p+d*distance;let probe=at+d*select(-EPS*2.,EPS*2.,dot(d,box.normal)>0.);let j=cell_index(probe);var same=false;
     if j!=0xffffffffu && dot(d,box.normal)>0. && optics[i].transport.w<0.999 {if optics[j].transport.w>=0.999 && abs(probe.z)<optics[j].material.w {return Hit(j,distance,-box.normal,at,true);}}
     if j!=0xffffffffu {same=j!=i && occupied(j) && (optics[j].transport.w>=0.999)==(optics[i].transport.w>=0.999) && optics[j].absorption.w==optics[i].absorption.w && optics[j].material.z==optics[i].material.z && abs(probe.z)<optics[j].material.w;}
     if !same{return Hit(i,distance,box.normal,at,true);}
    }
   }
  }
  if next>=range || next>1.e19{return empty_hit(p+d*range,range);}
  t=next;cell[axis]+=select(-1,1,d[axis]>0.);
 }
 return Hit(0xffffffffu,t,vec3<f32>(0.),p+d*t,true);
}
fn intersect(p:vec3<f32>,d:vec3<f32>,range:f32)->Hit {
 if USE_RTX {return hardware_hit(p,d,range);}return software_hit(p,d,range);
}
fn attenuation(p:vec3<f32>,d:vec3<f32>,distance:f32,shadow:bool,endpoint_index:u32)->vec3<f32> {
 var t=0.;var trans=vec3<f32>(1.);
 let lower=params.geometry.xy;let upper=lower+vec2<f32>(params.extent.xy)*params.geometry.z;
 for(var k=0u;k<2u;k++){if p[k]<lower[k] || p[k]>=upper[k] {
  if abs(d[k])<1.e-7{return trans;}
  t=max(t,min((lower[k]-p[k])/d[k],(upper[k]-p[k])/d[k])+EPS);
 }}
 for(var step=0u;step<params.budget.z;step++) {
  if t>=distance-EPS{return trans;}
  let q=p+d*(t+EPS*0.1);let i=cell_index(q);if i==0xffffffffu{return trans;}
  let xy=vec2<u32>(i%params.extent.x,i/params.extent.x);var next=distance;
  for(var k=0u;k<2u;k++){if abs(d[k])>1.e-7 {
   let edge=params.geometry[k]+(f32(xy[k])+select(0.,1.,d[k]>0.))*params.geometry.z;
   let candidate=(edge-p[k])/d[k];if candidate>t+1.e-6 {next=min(next,candidate);}
  }}
  let o=optics[i];
  if abs(d.z)>1.e-7 {for(var side=0u;side<2u;side++){let plane=select(-o.material.w,o.material.w,side==1u);let crossing=(plane-p.z)/d.z;if crossing>t+1.e-6{next=min(next,crossing);}}}
  if abs(q.z)<o.material.w {
   if shadow && o.transport.w>=0.999 && next-t>EPS*0.1 {if i==endpoint_index{return trans;}return vec3<f32>(0.);}
   let sigma=o.absorption.rgb-log(max(o.transport.rgb*(1.-min(o.transport.w,0.998)),vec3<f32>(1.e-6)));
   trans*=exp(-sigma*(next-t));
  }
  t=next;
 }
 if t<distance-EPS{return vec3<f32>(0.);}return trans;
}
fn visibility(p:vec3<f32>,q:vec3<f32>)->vec3<f32> {
 let v=q-p;let distance=length(v);if distance<EPS{return vec3<f32>(1.);}
 return attenuation(p,v/distance,distance,true,cell_index(q));
}
fn hemisphere(n:vec3<f32>,state:ptr<function,u32>)->vec3<f32> {
 let r=sqrt(random(state));let angle=random(state)*6.28318530718;var tangent=vec3<f32>(1.,0.,0.);
 if abs(n.z)<0.9{tangent=unit(vec3<f32>(-n.y,n.x,0.));}
 let b=cross(n,tangent);return unit(tangent*(r*cos(angle))+b*(r*sin(angle))+n*sqrt(1.-r*r));
}
fn point(i:u32,state:ptr<function,u32>)->vec3<f32> {
 let jitter_x=random(state);let jitter_y=random(state);
 let xy=vec2<f32>(f32(i%params.extent.x),f32(i/params.extent.x))+vec2<f32>(jitter_x,jitter_y);
 return vec3<f32>(params.geometry.xy+xy*params.geometry.z,select(0.,optics[i].material.w+EPS*2.,occupied(i)));
}
fn light_point(i:u32,state:ptr<function,u32>)->vec3<f32> {
 var q=point(i,state);
 if max(optics[i].emission.x,max(optics[i].emission.y,optics[i].emission.z))==0. {
  let center=params.geometry.xy+(vec2<f32>(f32(i%params.extent.x),f32(i/params.extent.x))+vec2<f32>(0.5))*params.geometry.z;
  q=vec3<f32>(center+(q.xy-center)*2./params.geometry.z,q.z);
 }
 q.z=optics[i].material.w+2.;return q;
}
fn fresnel(o:Optical,cosine:f32)->f32 {
 var f0=o.material.y;if o.absorption.w>0.{f0=pow((o.material.z-1.)/(o.material.z+1.),2.);}
 return f0+(1.-f0)*pow(1.-clamp(cosine,0.,1.),5.);
}
fn choose_light(u:f32)->Emitter {
 var lo=0u;var hi=params.budget.w;
 loop {if lo>=hi {break;}let mid=(lo+hi)/2u;if emitters[mid].cdf<=u{lo=mid+1u;}else{hi=mid;}}
 return emitters[min(lo,params.budget.w-1u)];
}
fn direct(p:vec3<f32>,n:vec3<f32>,state:ptr<function,u32>,range:f32)->vec3<f32> {
 if params.budget.w==0u{return vec3<f32>(0.);}
 let selected=choose_light(random(state));let source=selected.index;
 let q=light_point(source,state);let v=q-p;let dist=length(v);
 if dist<EPS || dist>range{return vec3<f32>(0.);}
 let cosine=max(dot(n,v/dist),0.);
 let weight=optics[source].source.a/max(selected.pdf,1.e-30)*cosine/(1.+dist*dist)*pow(params.sky_falloff.w,dist);
 return optics[source].source.rgb*visibility(p+n*EPS*2.,q)*weight;
}
fn stable_direct(p:vec3<f32>,n:vec3<f32>,range:f32,specular:bool,material:Optical)->vec3<f32> {
 if params.budget.w==0u{return vec3<f32>(0.);}
 let limit=select(32u,64u,params.quality.x>1u);let count=min(params.budget.w,limit);let full=params.budget.w<=limit;var sum=vec3<f32>(0.);
 for(var j=0u;j<count;j++) {
  var selected=emitters[j];var probability=1.;
  if !full{selected=choose_light((f32(j)+0.5)/f32(count));probability=selected.pdf*f32(count);}
  let source=selected.index;let o=optics[source];let area=o.source.a;
  let side=select(2.,params.geometry.z,max(o.emission.x,max(o.emission.y,o.emission.z))>0.);let taps=select(1u,4u,side>params.geometry.z||specular);
  for(var tap=0u;tap<taps;tap++) {
   var uv=vec2<f32>(0.5);if taps>1u{uv=vec2<f32>(0.25)+vec2<f32>(f32(tap%2u),f32(tap/2u))*0.5;}
   let xy=params.geometry.xy+(vec2<f32>(f32(source%params.extent.x),f32(source/params.extent.x))+vec2<f32>(0.5))*params.geometry.z+(uv-vec2<f32>(0.5))*side;
   let q=vec3<f32>(xy,o.material.w+2.);let v=q-p;let distance=length(v);if distance<EPS||distance>range{continue;}
   let l=v/distance;let nl=max(dot(n,l),0.);var weight=area*nl/(1.+distance*distance)*pow(params.sky_falloff.w,distance)/(probability*f32(taps));
   if specular {
    let view=unit(vec3<f32>(0.,-0.85,0.5268));let h=unit(l+view);let nv=max(dot(n,view),0.001);let nh=max(dot(n,h),0.);
    let a2=max(pow(material.material.x,4.),0.0025);let denom=nh*nh*(a2-1.)+1.;let distribution=a2/(3.14159265359*denom*denom);
    let k=pow(material.material.x+1.,2.)/8.;let geometry=nl/max(nl*(1.-k)+k,0.001)*nv/(nv*(1.-k)+k);
    weight*=distribution*fresnel(material,max(dot(l,h),0.))*geometry/max(4.*nl*nv,0.0001);
   }
   sum+=o.source.rgb*visibility(p+n*EPS*2.,q)*weight;
  }
 }
 return sum;
}
struct SurfaceRay { p:vec3<f32>,d:vec3<f32>,weight:vec3<f32>,range:f32,reflections:u32,transmissions:u32 }
fn surface_trace(p:vec3<f32>,d:vec3<f32>)->vec3<f32> {
 var stack:array<SurfaceRay,16>;var count=1u;stack[0]=SurfaceRay(p,d,vec3<f32>(1.),params.geometry.w,0u,0u);var radiance=vec3<f32>(0.);
 for(var visit=0u;visit<128u;visit++) {
  if count==0u{break;}count--;var ray=stack[count];
  if ray.range<EPS||max(ray.weight.x,max(ray.weight.y,ray.weight.z))<1.e-4{continue;}
  let h=intersect(ray.p,ray.d,ray.range);
  if !h.found{if ray.p.y+ray.d.y*ray.range<params.geometry.y{radiance+=ray.weight*params.sky_falloff.rgb*max(-ray.d.y,0.);}continue;}
  if h.index==0xffffffffu{continue;}
  ray.weight*=attenuation(ray.p,ray.d,h.distance,false,0xffffffffu)*pow(params.sky_falloff.w,h.distance);ray.range-=h.distance;
  let o=optics[h.index];let entering=dot(ray.d,h.normal)<0.;if entering{radiance+=ray.weight*o.emission.rgb;}let n=select(-h.normal,h.normal,entering);
  if o.absorption.w>0. {
   if ray.transmissions>=params.quality.y{continue;}
   let eta=select(o.material.z,1./o.material.z,entering);let cosine=-dot(ray.d,n);let k=1.-eta*eta*(1.-cosine*cosine);var f=1.;if k>=0.{f=fresnel(o,cosine);}
   if ray.reflections<params.quality.x && count<16u {
    let dir=unit(ray.d+2.*cosine*n);stack[count]=SurfaceRay(h.position+dir*EPS*2.,dir,ray.weight*f,ray.range,ray.reflections+1u,ray.transmissions+1u);count++;
   }
   if k>=0. && count<16u {
    let dir=unit(eta*ray.d+(eta*cosine-sqrt(k))*n);stack[count]=SurfaceRay(h.position+dir*EPS*2.,dir,ray.weight*(1.-f),ray.range,ray.reflections,ray.transmissions+1u);count++;
   }
  }else if o.transport.w<0.999 {
   if ray.transmissions<params.quality.y && count<16u {stack[count]=SurfaceRay(h.position+ray.d*EPS*2.,ray.d,ray.weight,ray.range,ray.reflections,ray.transmissions+1u);count++;}
  }else{
   let f=fresnel(o,max(-dot(ray.d,n),0.));let lighting=stable_direct(h.position,n,ray.range,false,o)+params.ambient.rgb;
   radiance+=ray.weight*o.diffuse.rgb*lighting*(1.-f);
   if o.material.y>0. && o.material.x>=0.25 {radiance+=ray.weight*stable_direct(h.position,n,ray.range,true,o);}
   if o.material.x<0.25 && o.material.y>0.2 && ray.reflections<params.quality.x && count<16u {
    let dir=unit(reflect(ray.d,n));stack[count]=SurfaceRay(h.position+dir*EPS*2.,dir,ray.weight*f,ray.range,ray.reflections+1u,ray.transmissions);count++;
   }
  }
 }
 return radiance;
}
fn stable_surface(i:u32)->vec3<f32> {
 let o=optics[i];let n=o.normal.xyz;let xy=params.geometry.xy+(vec2<f32>(f32(i%params.extent.x),f32(i/params.extent.x))+vec2<f32>(0.5))*params.geometry.z;
 let p=vec3<f32>(xy,select(0.,o.material.w+EPS*2.,occupied(i)));let incident=unit(vec3<f32>(0.,0.85,-0.5268));let cosine=max(-dot(incident,n),0.);let f=fresnel(o,cosine);var radiance=vec3<f32>(0.);
 if o.material.y>0. {
  if o.material.x<0.25{radiance=surface_trace(p+n*EPS*2.,unit(reflect(incident,n)))*f;}
  else{radiance=stable_direct(p,n,params.geometry.w,true,o);}
 }
 if o.absorption.w>0. {
  let eta=1./o.material.z;let k=1.-eta*eta*(1.-cosine*cosine);
  if k>0.{let dir=unit(eta*incident+(eta*cosine-sqrt(k))*n);radiance+=surface_trace(p+dir*EPS*4.,dir)*(1.-f);}
 }
 return radiance;
}
fn trace_diffuse(p:vec3<f32>,d:vec3<f32>,state:ptr<function,u32>)->vec3<f32> {
 var pos=p;var dir=d;var throughput=vec3<f32>(1.);var radiance=vec3<f32>(0.);var remaining=params.geometry.w;
 let limit=params.extent.w;var reflected=0u;var transmitted=0u;
 for(var step=0u;step<limit+params.quality.y+1u;step++) {
  if remaining<EPS{break;}
  let h=intersect(pos,dir,remaining);
  if !h.found {
   if pos.y+dir.y*remaining<params.geometry.y{return radiance+throughput*params.sky_falloff.rgb*max(-dir.y,0.);}
   return radiance;
  }
  if h.index==0xffffffffu{return radiance;}
  throughput*=attenuation(pos,dir,h.distance,false,0xffffffffu)*pow(params.sky_falloff.w,h.distance);
  let o=optics[h.index];remaining-=h.distance;
  if o.transport.w<0.999 && o.absorption.w==0. {
   if transmitted>=params.quality.y{break;}transmitted++;pos=h.position+dir*EPS*2.;continue;
  }
  if o.absorption.w>0. {
   if transmitted>=params.quality.y{break;}
   let entering=dot(dir,h.normal)<0.;let n=select(-h.normal,h.normal,entering);
   let eta=select(o.material.z,1./o.material.z,entering);let cosine=-dot(dir,n);let k=1.-eta*eta*(1.-cosine*cosine);
   if k<0.{dir=dir+2.*cosine*n;}
   else {throughput*=1.-fresnel(o,cosine);dir=unit(eta*dir+(eta*cosine-sqrt(k))*n);}
   transmitted++;pos=h.position+dir*EPS*2.;
  }else{
   let n=select(-h.normal,h.normal,dot(dir,h.normal)<0.);
   let diffuse=o.diffuse.rgb*(1.-fresnel(o,max(-dot(dir,n),0.)));
   radiance+=throughput*diffuse*direct(h.position,n,state,remaining);
   if reflected>=limit{break;}
   throughput*=diffuse;dir=hemisphere(n,state);
   pos=h.position+n*EPS*2.;reflected++;
  }
  if remaining<EPS || max(throughput.x,max(throughput.y,throughput.z))<0.0001{break;}
 }
 return radiance;
}
@compute @workgroup_size(8,8)
fn trace(@builtin(global_invocation_id) id:vec3<u32>) {
 if any(id.xy>=params.extent.xy){return;}
 let i=id.y*params.extent.x+id.x;let o=optics[i];var hd=history[i];
 if dirty[i]!=0u{hd=vec4<f32>(0.);}
 let dynamic=o.normal.w>0.;let cap=select(params.budget.y,min(params.budget.y,params.quality.w*params.extent.z),dynamic && dirty[i]!=0u);
 let xy=params.geometry.xy+(vec2<f32>(f32(i%params.extent.x),f32(i/params.extent.x))+vec2<f32>(0.5))*params.geometry.z;
 let center_point=vec3<f32>(xy,select(0.,o.material.w+EPS*2.,occupied(i)));
 let current_direct=stable_direct(center_point,o.normal.xyz,params.geometry.w,false,o);
 let spec=stable_surface(i);
 let sample_count=min(params.extent.z,params.budget.y-u32(hd.a));
 for(var sample=0u;sample<sample_count;sample++) {
  let sample_index=params.budget.x*params.extent.z+sample;var state=(i*747796405u)^(sample_index*2891336453u)^0x9e3779b9u;state=max(state,1u);
  let p=point(i,&state);let n=o.normal.xyz;
  var indirect=trace_diffuse(p+n*EPS*2.,hemisphere(n,&state),&state);
  if params.ambient.w>0. {
   let reference=max(max(current_direct.x,max(current_direct.y,current_direct.z)),max(params.sky_falloff.x,max(params.sky_falloff.y,params.sky_falloff.z))*0.125+max(params.ambient.x,max(params.ambient.y,params.ambient.z))*2.+0.01);
   let ceiling=reference*params.ambient.w;indirect*=min(1.,ceiling/max(max(indirect.x,max(indirect.y,indirect.z)),1.e-20));
  }
  if hd.a<f32(cap){hd=vec4<f32>(hd.rgb+indirect,hd.a+1.);}
  else if dynamic {let a=1./f32(cap);hd=vec4<f32>(hd.rgb*(1.-a)+indirect,hd.a);}
 }
 history[i]=hd;
 let resolved_direct=current_direct;
 result[i]=Result(vec4<f32>(resolved_direct+hd.rgb/max(hd.a,1.)+params.ambient.rgb,1.),vec4<f32>(spec,1.),vec4<f32>(o.emission.rgb,1.),vec4<f32>(o.normal.xyz,o.material.w+select(0.,256.,occupied(i))+select(0.,512.,o.absorption.w>0.)));
}
// Spatial filtering reads a completed trace dispatch, so neighboring cells never
// race their current-frame write. Discontinuities are checked along the kernel.
@compute @workgroup_size(8,8)
fn resolve(@builtin(global_invocation_id) id:vec3<u32>) {
 if any(id.xy>=params.extent.xy){return;}
 let i=id.y*params.extent.x+id.x;let center=result[i];let radius=i32(params.quality.z);var diffuse=vec3<f32>(0.);var spec=vec3<f32>(0.);var total=0.;
 for(var y=-radius;y<=radius;y++){for(var x=-radius;x<=radius;x++) {
  let xy=vec2<i32>(id.xy)+vec2<i32>(x,y);if any(xy<vec2<i32>(0))||any(xy>=vec2<i32>(params.extent.xy)){continue;}
  let j=u32(xy.y)*params.extent.x+u32(xy.x);let other=result[j];
  if abs(center.guide.a-other.guide.a)>0.1 || dot(center.guide.xyz,other.guide.xyz)<0.95 || distance(optics[i].diffuse.rgb,optics[j].diffuse.rgb)>0.05 || abs(optics[i].material.x-optics[j].material.x)>0.1 || abs(optics[i].material.z-optics[j].material.z)>0.01 {continue;}
  // Never filter across an intervening wall or material boundary.
  var valid=true;let steps=max(abs(x),abs(y));
  for(var k=1;k<=steps;k++) {
   let previous=vec2<i32>(id.xy)+vec2<i32>(round(vec2<f32>(f32(x),f32(y))*f32(k-1)/f32(steps)));
   let mid=vec2<i32>(id.xy)+vec2<i32>(round(vec2<f32>(f32(x),f32(y))*f32(k)/f32(steps)));
   let m=u32(mid.y)*params.extent.x+u32(mid.x);let a=u32(previous.y)*params.extent.x+u32(mid.x);let b=u32(mid.y)*params.extent.x+u32(previous.x);
   if abs(result[m].guide.a-center.guide.a)>0.1 || abs(result[a].guide.a-center.guide.a)>0.1 || abs(result[b].guide.a-center.guide.a)>0.1 {valid=false;}
  }
  if !valid {continue;}
  let weight=exp(-f32(x*x+y*y)*0.5);diffuse+=other.diffuse.rgb*weight;spec+=other.specular.rgb*weight;total+=weight;
 }}
 let xy=vec2<i32>(id.xy);let width=i32(params.extent.x);
 textureStore(image,xy,vec4<f32>(min(diffuse/max(total,1.e-6),vec3<f32>(65504.)),1.));
 textureStore(image,xy+vec2<i32>(width,0),vec4<f32>(min(select(spec/max(total,1.e-6),center.specular.rgb,optics[i].material.x<0.25),vec3<f32>(65504.)),1.));
 textureStore(image,xy+vec2<i32>(width*2,0),center.emission);
 textureStore(image,xy+vec2<i32>(width*3,0),center.guide);
}
