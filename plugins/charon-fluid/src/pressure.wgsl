// Cooperative PCG with a two-level SPD preconditioner. Coefficients and f64
// residual validation live on CPU; every Krylov iteration stays on GPU.
override LANES:u32=128u;
struct Row { adjacent:vec4u, a:vec4f, diagonal:f32, rhs:f32, pad:vec2f }
struct Work { p:f32, r:f32, z:f32, d:f32, product:f32 }
struct Preconditioner { blocks:u32, block_size:u32, coarse_offset:u32, rhs_offset:u32, data:array<f32> }
@group(0) @binding(0) var<storage,read> rows:array<Row>;
@group(0) @binding(1) var<storage,read_write> work:array<Work>;
@group(0) @binding(2) var<storage,read_write> pc:Preconditioner;
@group(0) @binding(3) var<storage,read> right:array<f32>;
var<workgroup> sums:array<vec2f,128>;
var<workgroup> rz:f32;
var<workgroup> threshold:f32;
var<workgroup> alpha:f32;
var<workgroup> beta:f32;
var<workgroup> done:u32;
var<workgroup> iterations:u32;
fn reduce(value:vec2f, lane:u32) -> vec2f {
 sums[lane]=value; workgroupBarrier();
 for(var stride=LANES/2u; stride>0u; stride/=2u) {
  if lane<stride {sums[lane]+=sums[lane+stride];} workgroupBarrier();
 }
 let result=sums[0]; workgroupBarrier(); return result;
}
fn precondition(lane:u32) {
 let n=arrayLength(&rows); let block_size=pc.block_size;
 let indices=n*(block_size+1u);
 for(var b=lane;b<pc.blocks;b+=LANES) {
  var value=0.;
  for(var k=0u;k<block_size;k+=1u) {
   let j=pc.data[indices+b*block_size+k]; if j>=0. {value+=work[u32(j)].r;}
  }
  pc.data[pc.rhs_offset+b]=value;
 }
 storageBarrier();
 for(var b=lane;b<pc.blocks;b+=LANES) {
  var value=0.;
  for(var j=0u;j<pc.blocks;j+=1u) {value+=pc.data[pc.coarse_offset+b*pc.blocks+j]*pc.data[pc.rhs_offset+j];}
  pc.data[pc.rhs_offset+pc.blocks+b]=value;
 }
 storageBarrier();
 for(var i=lane;i<n;i+=LANES) {
  if pc.blocks==0u {work[i].z=work[i].r/rows[i].diagonal; continue;}
  let b=u32(pc.data[n*block_size+i])/block_size;
  var value=pc.data[pc.rhs_offset+pc.blocks+b];
  for(var k=0u;k<block_size;k+=1u) {
   let j=pc.data[indices+b*block_size+k];
   if j>=0. {value+=pc.data[i*block_size+k]*work[u32(j)].r;}
  }
  work[i].z=value;
 }
 storageBarrier();
}
@compute @workgroup_size(LANES)
fn main(@builtin(local_invocation_index) lane:u32) {
 let n=arrayLength(&rows);
 for(var i=lane;i<n;i+=LANES) {work[i]=Work(0.,right[i],0.,0.,0.);}
 storageBarrier(); precondition(lane);
 var local=vec2f(0.);
 for(var i=lane;i<n;i+=LANES) {
  work[i].d=work[i].z; local+=vec2f(work[i].r*work[i].z,work[i].r*work[i].r);
 }
 storageBarrier(); let initial=reduce(local,lane);
 if lane==0u {rz=initial.x; threshold=max(initial.y*1e-10,1e-24); done=select(0u,1u,initial.y<=threshold); iterations=0u;}
 workgroupBarrier();
 for(var iteration=0u;iteration<800u;iteration+=1u) {
  if workgroupUniformLoad(&done)!=0u {break;}
  var local_dot=0.;
  for(var i=lane;i<n;i+=LANES) {
   var ap=rows[i].diagonal*work[i].d;
   for(var k=0u;k<4u;k+=1u) {let j=rows[i].adjacent[k]; if j!=0xffffffffu {ap-=rows[i].a[k]*work[j].d;}}
   work[i].product=ap; local_dot+=work[i].d*ap;
  }
  storageBarrier(); let denominator=reduce(vec2f(local_dot,0.),lane).x;
  if lane==0u {alpha=rz/denominator;}
  workgroupBarrier();
  for(var i=lane;i<n;i+=LANES) {work[i].p+=alpha*work[i].d; work[i].r-=alpha*work[i].product;}
  storageBarrier(); precondition(lane);
  var next=vec2f(0.);
  for(var i=lane;i<n;i+=LANES) {next+=vec2f(work[i].r*work[i].z,work[i].r*work[i].r);}
  let norm=reduce(next,lane);
  if lane==0u {done=select(0u,1u,norm.y<=threshold); beta=norm.x/rz; rz=norm.x; iterations=iteration+1u;}
  workgroupBarrier();
  for(var i=lane;i<n;i+=LANES) {work[i].d=work[i].z+beta*work[i].d;}
  storageBarrier();
 }
 if lane==0u {work[0].product=f32(iterations);}
}
