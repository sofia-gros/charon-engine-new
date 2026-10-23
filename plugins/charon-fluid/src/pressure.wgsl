// Cooperative PCG with a two-level SPD preconditioner. Coefficients and f64
// residual validation live on CPU; every Krylov iteration stays on GPU.
override LANES:u32=128u;
// 1 selects the storage path; 1024 uses <16 KiB of workgroup memory.
override CACHE_SIZE:u32=1u;
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
// Each invocation owns its global p/product fields. Neighbour residuals,
// directions and coarse RHS/corrections cooperate through local memory only.
struct Vector { r:f32, z:f32, d:f32 }
var<workgroup> cached:array<Vector,CACHE_SIZE>;
var<workgroup> coarse_cache:array<f32,512>;
fn residual(i:u32) -> f32 {
 if CACHE_SIZE>1u {return cached[i].r;} return work[i].r;
}
fn correction(i:u32) -> f32 {
 if CACHE_SIZE>1u {return cached[i].z;} return work[i].z;
}
fn direction(i:u32) -> f32 {
 if CACHE_SIZE>1u {return cached[i].d;} return work[i].d;
}
fn store_r(i:u32,v:f32) {
 if CACHE_SIZE>1u {cached[i].r=v;} else {work[i].r=v;}
}
fn store_z(i:u32,v:f32) {
 if CACHE_SIZE>1u {cached[i].z=v;} else {work[i].z=v;}
}
fn store_d(i:u32,v:f32) {
 if CACHE_SIZE>1u {cached[i].d=v;} else {work[i].d=v;}
}
fn coarse(i:u32) -> f32 {
 if CACHE_SIZE>1u {return coarse_cache[i];} return pc.data[pc.rhs_offset+i];
}
fn store_coarse(i:u32,v:f32) {
 if CACHE_SIZE>1u {coarse_cache[i]=v;} else {pc.data[pc.rhs_offset+i]=v;}
}
fn synchronize() {
 if CACHE_SIZE>1u {workgroupBarrier();} else {storageBarrier();}
}
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
   let j=pc.data[indices+b*block_size+k]; if j>=0. {value+=residual(u32(j));}
  }
  store_coarse(b,value);
 }
 synchronize();
 for(var b=lane;b<pc.blocks;b+=LANES) {
  var value=0.;
  for(var j=0u;j<pc.blocks;j+=1u) {value+=pc.data[pc.coarse_offset+b*pc.blocks+j]*coarse(j);}
  store_coarse(pc.blocks+b,value);
 }
 synchronize();
 for(var i=lane;i<n;i+=LANES) {
  if pc.blocks==0u {store_z(i,residual(i)/rows[i].diagonal); continue;}
  let b=u32(pc.data[n*block_size+i])/block_size;
  var value=coarse(pc.blocks+b);
  for(var k=0u;k<block_size;k+=1u) {
   let j=pc.data[indices+b*block_size+k];
   if j>=0. {value+=pc.data[i*block_size+k]*residual(u32(j));}
  }
  store_z(i,value);
 }
 synchronize();
}
@compute @workgroup_size(LANES)
fn main(@builtin(local_invocation_index) lane:u32) {
 let n=arrayLength(&rows);
 for(var i=lane;i<n;i+=LANES) {
  work[i]=Work(0.,right[i],0.,0.,0.);
  if CACHE_SIZE>1u {cached[i]=Vector(right[i],0.,0.);}
 }
 synchronize(); precondition(lane);
 var local=vec2f(0.);
 for(var i=lane;i<n;i+=LANES) {
  store_d(i,correction(i)); local+=vec2f(residual(i)*correction(i),residual(i)*residual(i));
 }
 synchronize(); let initial=reduce(local,lane);
 if lane==0u {rz=initial.x; threshold=max(initial.y*1e-10,1e-24); done=select(0u,1u,initial.y<=threshold); iterations=0u;}
 workgroupBarrier();
 for(var iteration=0u;iteration<800u;iteration+=1u) {
  if workgroupUniformLoad(&done)!=0u {break;}
  var local_dot=0.;
  for(var i=lane;i<n;i+=LANES) {
   var ap=rows[i].diagonal*direction(i);
   for(var k=0u;k<4u;k+=1u) {let j=rows[i].adjacent[k]; if j!=0xffffffffu {ap-=rows[i].a[k]*direction(j);}}
   work[i].product=ap; local_dot+=direction(i)*ap;
  }
  synchronize(); let denominator=reduce(vec2f(local_dot,0.),lane).x;
  if lane==0u {alpha=rz/denominator;}
  workgroupBarrier();
  for(var i=lane;i<n;i+=LANES) {work[i].p+=alpha*direction(i); store_r(i,residual(i)-alpha*work[i].product);}
  synchronize(); precondition(lane);
  var next=vec2f(0.);
  for(var i=lane;i<n;i+=LANES) {next+=vec2f(residual(i)*correction(i),residual(i)*residual(i));}
  let norm=reduce(next,lane);
  if lane==0u {done=select(0u,1u,norm.y<=threshold); beta=norm.x/rz; rz=norm.x; iterations=iteration+1u;}
  workgroupBarrier();
  for(var i=lane;i<n;i+=LANES) {store_d(i,correction(i)+beta*direction(i));}
  synchronize();
 }
 if lane==0u {work[0].product=f32(iterations);}
}
