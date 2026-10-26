// Independent, single-ray probe: no HDR transport, stack, lights, or history.
struct Probe { origin_range:vec4<f32>, direction:vec4<f32> }
@group(0) @binding(0) var<uniform> probe:Probe;
@group(0) @binding(1) var<storage,read_write> output:array<vec4<f32>>;
@group(1) @binding(0) var acceleration:acceleration_structure;
@group(1) @binding(1) var<storage,read> faces:array<vec4<u32>>;
@compute @workgroup_size(1)
fn main() {
 var query:ray_query;
 rayQueryInitialize(&query,acceleration,RayDesc(0u,255u,0.002,probe.origin_range.w,probe.origin_range.xyz,probe.direction.xyz));
 loop {if !rayQueryProceed(&query){break;}}
 let hit=rayQueryGetCommittedIntersection(&query);
 if hit.kind==0u {output[0]=vec4<f32>(0.,-1.,-1.,0.);return;}
 let face=faces[hit.primitive_index/2u];
 output[0]=vec4<f32>(1.,hit.t,f32(face.x),f32(face.y));
}
