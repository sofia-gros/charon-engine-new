// One cooperative group over compact liquid rows. No per-iteration CPU waits.
// f64 defect correction outside this kernel validates the original operator.
struct Row { adjacent:vec4u, a:vec4f, diagonal:f32, rhs:f32, pad:vec2f }
struct Work { p:f32, r:f32, z:f32, d:f32, product:f32 }
@group(0) @binding(0) var<storage,read> rows:array<Row>;
@group(0) @binding(1) var<storage,read_write> work:array<Work>;
var<workgroup> sums:array<f32,128>;
var<workgroup> rz:f32;
var<workgroup> threshold:f32;
var<workgroup> alpha:f32;
var<workgroup> beta:f32;
var<workgroup> done:u32;
var<workgroup> iterations:u32;
fn reduce(value:f32, lane:u32) -> f32 {
 sums[lane]=value; workgroupBarrier();
 for(var stride=64u; stride>0u; stride/=2u) {
  if lane<stride {sums[lane]+=sums[lane+stride];} workgroupBarrier();
 }
 let result=sums[0]; workgroupBarrier(); return result;
}
@compute @workgroup_size(128)
fn main(@builtin(local_invocation_index) lane:u32) {
 let n=arrayLength(&rows); var local_rz=0.; var local_rr=0.;
 for(var i=lane;i<n;i+=128u) {
  let r=rows[i].rhs; let z=r/rows[i].diagonal;
  work[i]=Work(0.,r,z,z,0.); local_rz+=r*z; local_rr+=r*r;
 }
 storageBarrier();
 let initial_rz=reduce(local_rz,lane); let initial_rr=reduce(local_rr,lane);
 if lane==0u {rz=initial_rz; threshold=max(initial_rr*1e-10,1e-24); done=select(0u,1u,initial_rr<=threshold); iterations=0u;}
 workgroupBarrier();
 for(var iteration=0u;iteration<800u;iteration+=1u) {
  if workgroupUniformLoad(&done)!=0u {break;}
  var local_dot=0.;
  for(var i=lane;i<n;i+=128u) {
   var ap=rows[i].diagonal*work[i].d;
   for(var k=0u;k<4u;k+=1u) {let j=rows[i].adjacent[k]; if j!=0xffffffffu {ap-=rows[i].a[k]*work[j].d;}}
   work[i].product=ap; local_dot+=work[i].d*ap;
  }
  storageBarrier(); let denominator=reduce(local_dot,lane);
  if lane==0u {alpha=rz/denominator;}
  workgroupBarrier(); var rr=0.; var next_rz=0.;
  for(var i=lane;i<n;i+=128u) {
   work[i].p+=alpha*work[i].d; work[i].r-=alpha*work[i].product;
   work[i].z=work[i].r/rows[i].diagonal; rr+=work[i].r*work[i].r; next_rz+=work[i].r*work[i].z;
  }
  storageBarrier(); let norm=reduce(rr,lane); let next=reduce(next_rz,lane);
  if lane==0u {done=select(0u,1u,norm<=threshold); beta=next/rz; rz=next; iterations=iteration+1u;}
  workgroupBarrier();
  for(var i=lane;i<n;i+=128u) {work[i].d=work[i].z+beta*work[i].d;}
  storageBarrier();
 }
 if lane==0u {work[0].product=f32(iterations);}
}
