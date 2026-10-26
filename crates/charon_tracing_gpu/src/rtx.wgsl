@group(1) @binding(0) var acceleration:acceleration_structure;
@group(1) @binding(1) var<storage,read> faces:array<vec4<u32>>;
fn opaque_hit(p:vec2<f32>,d:vec2<f32>,range:f32)->Hit {
 var query:ray_query;
 rayQueryInitialize(&query,acceleration,RayDesc(0u,255u,0.0001,range,vec3<f32>(p,0.),vec3<f32>(d,0.)));
 loop {if !rayQueryProceed(&query){break;}}
 let intersection=rayQueryGetCommittedIntersection(&query);
 if intersection.kind==0u {return empty_hit(p,range);}
 let face=faces[intersection.primitive_index/2u];var normal=vec2<f32>(0.);normal[face.y]=select(-1.,1.,face.z==1u);
 return Hit(face.x,p+d*intersection.t,normal,vec3<f32>(1.),intersection.t,false,true);
}

// A separate entry point verifies actual hardware hits, never the DDA fallback.
@group(0) @binding(7) var<storage,read_write> probe_output:array<vec4<f32>>;
@compute @workgroup_size(1)
fn verify_rtx() {
 let hit=opaque_hit(vec2<f32>(0.5,1.5),vec2<f32>(1.,0.),3.);
 probe_output[0]=vec4<f32>(select(0.,1.,hit.found),hit.distance,f32(hit.index),0.);
}
