// Global ordering is provided by successive dispatches. No workgroup spins on
// another workgroup. New and old CG directions occupy separate scalar fields.
struct Row { adjacent:vec4u, a:vec4f, diagonal:f32, rhs:f32, pad:vec2f }
struct Work { p:f32, r:f32, z:f32, d0:f32, d1:f32, product:f32 }
struct Partial { dot:f32, rr:f32, rz:f32, pad:f32 }
struct Control { rz:f32, tolerance:f32, alpha:f32, beta:f32, iteration:u32, done:u32, pad:vec2u }
@group(0) @binding(0) var<storage,read> rows:array<Row>;
@group(0) @binding(1) var<storage,read_write> work:array<Work>;
@group(0) @binding(2) var<storage,read_write> partials:array<Partial>;
@group(0) @binding(3) var<storage,read_write> control:Control;
var<workgroup> sums:array<f32,128>;
var<workgroup> status:u32;
fn finished(lane:u32) -> bool {
 if lane==0u {status=control.done;}
 workgroupBarrier();
 return workgroupUniformLoad(&status)!=0u;
}
fn reduce(value:f32,lane:u32) -> f32 {
 sums[lane]=value; workgroupBarrier();
 for(var stride=64u;stride>0u;stride/=2u) {
  if lane<stride {sums[lane]+=sums[lane+stride];} workgroupBarrier();
 }
 let total=sums[0]; workgroupBarrier(); return total;
}
@compute @workgroup_size(128)
fn initialize(@builtin(global_invocation_id) id:vec3u,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3u) {
 let i=id.x; var rr=0.; var rz=0.;
 if i<arrayLength(&rows) {
  let r=rows[i].rhs; let z=r/rows[i].diagonal;
  work[i]=Work(0.,r,z,0.,0.,0.); rr=r*r; rz=r*z;
 }
 let total_rr=reduce(rr,lane); let total_rz=reduce(rz,lane);
 if lane==0u {partials[group.x]=Partial(0.,total_rr,total_rz,0.);}
}
@compute @workgroup_size(128)
fn start(@builtin(local_invocation_index) lane:u32) {
 var rr=0.; var rz=0.;
 for(var i=lane;i<arrayLength(&partials);i+=128u) {rr+=partials[i].rr; rz+=partials[i].rz;}
 let total_rr=reduce(rr,lane); let total_rz=reduce(rz,lane);
 if lane==0u {
  control.rz=total_rz; control.tolerance=max(total_rr*1e-10,1e-24);
  control.alpha=0.; control.beta=0.; control.iteration=0u;
  control.done=select(0u,1u,total_rr<=control.tolerance);
 }
}
fn old_direction(i:u32) -> f32 {
 if control.iteration%2u==0u {return work[i].d1;} return work[i].d0;
}
fn new_direction(i:u32) -> f32 {return work[i].z+control.beta*old_direction(i);}
@compute @workgroup_size(128)
fn product(@builtin(global_invocation_id) id:vec3u,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3u) {
 if finished(lane) {return;}
 let i=id.x; var dot=0.;
 if i<arrayLength(&rows) {
  let d=new_direction(i); var ap=rows[i].pad.x*d;
  // Difference form preserves each closed component's constant nullspace
  // exactly, despite rounding diag and variable-density weights to f32.
  for(var k=0u;k<4u;k+=1u) {
   let j=rows[i].adjacent[k];
   if j!=0xffffffffu {ap+=rows[i].a[k]*(d-new_direction(j));}
   else {ap+=rows[i].a[k]*d;}
  }
  if control.iteration%2u==0u {work[i].d0=d;} else {work[i].d1=d;}
  work[i].product=ap; dot=d*ap;
 }
 let total=reduce(dot,lane); if lane==0u {partials[group.x].dot=total;}
}
@compute @workgroup_size(128)
fn alpha(@builtin(local_invocation_index) lane:u32) {
 if finished(lane) {return;}
 var dot=0.; for(var i=lane;i<arrayLength(&partials);i+=128u) {dot+=partials[i].dot;}
 let denominator=reduce(dot,lane);
 if lane==0u {
  if denominator<=0. || denominator!=denominator {control.done=2u;}
  else {control.alpha=control.rz/denominator;}
 }
}
@compute @workgroup_size(128)
fn update(@builtin(global_invocation_id) id:vec3u,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3u) {
 if finished(lane) {return;}
 let i=id.x; var rr=0.; var rz=0.;
 if i<arrayLength(&rows) {
  var d=work[i].d0; if control.iteration%2u!=0u {d=work[i].d1;}
  work[i].p+=control.alpha*d; work[i].r-=control.alpha*work[i].product;
  work[i].z=work[i].r/rows[i].diagonal; rr=work[i].r*work[i].r; rz=work[i].r*work[i].z;
 }
 let total_rr=reduce(rr,lane); let total_rz=reduce(rz,lane);
 if lane==0u {partials[group.x].rr=total_rr; partials[group.x].rz=total_rz;}
}
@compute @workgroup_size(128)
fn beta(@builtin(local_invocation_index) lane:u32) {
 if finished(lane) {return;}
 var rr=0.; var rz=0.;
 for(var i=lane;i<arrayLength(&partials);i+=128u) {rr+=partials[i].rr; rz+=partials[i].rz;}
 let total_rr=reduce(rr,lane); let total_rz=reduce(rz,lane);
 if lane==0u {
  control.done=select(0u,1u,total_rr<=control.tolerance);
  control.beta=total_rz/control.rz; control.rz=total_rz; control.iteration+=1u;
 }
}
@compute @workgroup_size(128)
fn collect(@builtin(global_invocation_id) id:vec3u) {
 let first=id.x*4u; let n=arrayLength(&rows); if first>=n {return;}
 var values=vec4f(0.); for(var k=0u;k<4u;k+=1u) {if first+k<n {values[k]=work[first+k].p;}}
 partials[id.x]=Partial(values.x,values.y,values.z,values.w);
}
