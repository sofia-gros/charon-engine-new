struct Params { w:u32, h:u32, x0:u32, y0:u32, x1:u32, y1:u32, dt:f32, diffusion:f32 }
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> old:array<f32>;
@group(0) @binding(2) var<storage,read_write> next:array<f32>;
fn sample_field(base:u32, w:u32, h:u32, position:vec2f) -> f32 {
 let pos = clamp(position,vec2f(0.),vec2f(f32(w-1u),f32(h-1u)));
 let a=vec2u(floor(pos)); let b=min(a+vec2u(1u),vec2u(w-1u,h-1u)); let f=fract(pos);
 return mix(mix(old[base+a.y*w+a.x],old[base+a.y*w+b.x],f.x),mix(old[base+b.y*w+a.x],old[base+b.y*w+b.x],f.x),f.y);
}
@compute @workgroup_size(128)
fn main(@builtin(global_invocation_id) id:vec3u) {
 let un=(p.w+1u)*p.h; let width=p.x1-p.x0;
 let area=width*(p.y1-p.y0); let out=id.x;
 if out>=2u*area {return;}
 let local=out%area; let x=p.x0+local%width; let y=p.y0+local/width;
 if out<area {
  let i=y*(p.w+1u)+x;
  let vy=sample_field(un,p.w,p.h+1u,vec2f(f32(x)-.5,f32(y)+.5));
  var lap=0.; if y+1u<p.h {lap=old[i-1u]+old[i+1u]+old[i-p.w-1u]+old[i+p.w+1u]-4.*old[i];}
  next[out]=sample_field(0u,p.w+1u,p.h,vec2f(f32(x)-p.dt*old[i],f32(y)-p.dt*vy))+p.diffusion*lap;
 } else {
  let i=un+y*p.w+x;
  let ux=sample_field(0u,p.w+1u,p.h,vec2f(f32(x)+.5,f32(y)-.5));
  var lap=0.; if x+1u<p.w {lap=old[i-1u]+old[i+1u]+old[i-p.w]+old[i+p.w]-4.*old[i];}
  next[out]=sample_field(un,p.w,p.h+1u,vec2f(f32(x)-p.dt*ux,f32(y)-p.dt*old[i]))+p.diffusion*lap;
 }
}
