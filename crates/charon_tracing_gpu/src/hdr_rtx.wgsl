@group(1) @binding(0) var acceleration:acceleration_structure;
@group(1) @binding(1) var<storage,read> faces:array<vec4<u32>>;
fn hardware_hit(p:vec3<f32>,d:vec3<f32>,range:f32)->Hit {
 var query:ray_query;
 rayQueryInitialize(&query,acceleration,RayDesc(0u,255u,EPS,range,p,d));
 loop {if !rayQueryProceed(&query){break;}}
 let hit=rayQueryGetCommittedIntersection(&query);
 if hit.kind==0u{return empty_hit(p+d*range,range);}
 let face=faces[hit.primitive_index/2u];var n=vec3<f32>(0.);n[face.y]=select(-1.,1.,face.z==1u);
 return Hit(face.x,hit.t,n,p+d*hit.t,true);
}
