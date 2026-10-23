// Symmetric aggregation V-cycle. Dispatch boundaries order different levels;
// no workgroup waits or spins on another workgroup, and no float atomics.
override LANES:u32=128u;
struct Row { adjacent:vec4u, a:vec4f, diagonal:f32, rhs:f32, pad:vec2f }
struct Work { p:f32, r:f32, z:f32, d0:f32, d1:f32, product:f32 }
struct Partial { dot:f32, rr:f32, rz:f32, pad:f32 }
struct Control { rz:f32, tolerance:f32, alpha:f32, beta:f32, iteration:u32, done:u32, pad:vec2u }
struct Node { first:u32, count:u32, parent:u32, child_first:u32, child_count:u32, pad:u32, diagonal:f32, boundary:f32 }
struct Value { rhs:f32, x0:f32, x1:f32, pad:f32 }
struct Params { offset:u32, count:u32, coarse_offset:u32, coarse_count:u32, inverse_offset:u32, direct:u32, pad:vec2u }
@group(0) @binding(0) var<storage,read> rows:array<Row>;
@group(0) @binding(1) var<storage,read_write> work:array<Work>;
@group(0) @binding(2) var<storage,read_write> partials:array<Partial>;
@group(0) @binding(3) var<storage,read_write> control:Control;
@group(0) @binding(4) var<storage,read> nodes:array<Node>;
@group(0) @binding(5) var<storage,read> data:array<u32>;
@group(0) @binding(6) var<storage,read_write> values:array<Value>;
@group(0) @binding(7) var<uniform> params:Params;
var<workgroup> sums:array<vec2f,128>;
var<workgroup> status:u32;
fn value(i:u32,second:bool) -> f32 {
 if second {return values[i].x1;} return values[i].x0;
}
fn applied(i:u32,second:bool) -> f32 {
 let x=value(i,second); let node=nodes[i]; var ax=node.boundary*x;
 for(var k=0u;k<node.count;k+=1u) {
  let j=data[node.first+2u*k]; let weight=bitcast<f32>(data[node.first+2u*k+1u]);
  ax+=weight*(x-value(j,second));
 }
 return ax;
}
@compute @workgroup_size(LANES)
fn rhs(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u) {
 if control.done!=0u {return;}
 for(var j=id.x;j<params.count;j+=groups.x*LANES) {
  let i=params.offset+j; let r=work[j].r;
  values[i]=Value(r,0.,(2./3.)*r/nodes[i].diagonal,0.);
 }
}
@compute @workgroup_size(LANES)
fn smooth_first(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u) {
 if control.done!=0u {return;}
 for(var j=id.x;j<params.count;j+=groups.x*LANES) {
  let i=params.offset+j;
  values[i].x1=values[i].x0+(2./3.)*(values[i].rhs-applied(i,false))/nodes[i].diagonal;
 }
}
@compute @workgroup_size(LANES)
fn smooth_second(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u) {
 if control.done!=0u {return;}
 for(var j=id.x;j<params.count;j+=groups.x*LANES) {
  let i=params.offset+j;
  values[i].x0=values[i].x1+(2./3.)*(values[i].rhs-applied(i,true))/nodes[i].diagonal;
 }
}
@compute @workgroup_size(LANES)
fn restrict_residual(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u) {
 if control.done!=0u {return;}
 for(var j=id.x;j<params.count;j+=groups.x*LANES) {
  let i=params.offset+j; let node=nodes[i]; var b=0.;
  for(var k=0u;k<node.child_count;k+=1u) {
   let child=data[node.child_first+k]; b+=values[child].rhs-applied(child,false);
  }
  values[i]=Value(b,0.,(2./3.)*b/nodes[i].diagonal,0.);
 }
}
@compute @workgroup_size(LANES)
fn coarse_solve(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u) {
 if control.done!=0u {return;}
 for(var j=id.x;j<params.count;j+=groups.x*LANES) {
  var x=0.;
  for(var k=0u;k<params.count;k+=1u) {
   x+=bitcast<f32>(data[params.inverse_offset+j*params.count+k])*values[params.offset+k].rhs;
  }
  values[params.offset+j].x0=x;
 }
}
// Evaluate prolongation on demand from immutable fine/coarse x0 fields.
// Only x1 is written, so neighbouring groups never read an in-flight value.
fn prolonged(i:u32) -> f32 {return values[i].x0+values[nodes[i].parent].x0;}
@compute @workgroup_size(LANES)
fn prolong_smooth(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u) {
 if control.done!=0u {return;}
 for(var j=id.x;j<params.count;j+=groups.x*LANES) {
  let i=params.offset+j; let node=nodes[i]; let x=prolonged(i); var ax=node.boundary*x;
  for(var k=0u;k<node.count;k+=1u) {
   let other=data[node.first+2u*k]; let weight=bitcast<f32>(data[node.first+2u*k+1u]);
   ax+=weight*(x-prolonged(other));
  }
  values[i].x1=x+(2./3.)*(values[i].rhs-ax)/node.diagonal;
 }
}
@compute @workgroup_size(LANES)
fn finish(@builtin(global_invocation_id) id:vec3u,@builtin(num_workgroups) groups:vec3u,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) group:vec3u) {
 if lane==0u {status=control.done;}
 workgroupBarrier(); if workgroupUniformLoad(&status)!=0u {return;}
 var local=vec2f(0.);
 for(var i=id.x;i<params.count;i+=groups.x*LANES) {
  let r=work[i].r;
  // The finest final Jacobi sweep can directly produce PCG z and dot products.
  // A one-level direct hierarchy has no pending sweep.
  var z=values[i].x0;
  if params.coarse_count!=0u || params.direct==0u {
   z=values[i].x1+(2./3.)*(values[i].rhs-applied(i,true))/nodes[i].diagonal;
  }
  work[i].z=z;
  local+=vec2f(r*r,r*z);
 }
 sums[lane]=local; workgroupBarrier();
 for(var stride=LANES/2u;stride>0u;stride/=2u) {
  if lane<stride {sums[lane]+=sums[lane+stride];} workgroupBarrier();
 }
 if lane==0u {partials[group.x].rr=sums[0].x; partials[group.x].rz=sums[0].y;}
}
