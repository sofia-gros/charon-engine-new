// Independent 2D direct lighting: exact grid traversal, no simulation state writes.
struct Optical { value: vec4<f32> }; // emissive RGB, opacity
struct Optics { entries: array<Optical, 256> };
struct Light { position_radius: vec4<f32>, color: vec4<f32> };
struct Settings { values: vec4<f32>, lights: array<Light, 32> };
struct Tile { origin: vec2<u32> };
@group(0) @binding(0) var terrain: texture_2d<u32>;
@group(0) @binding(1) var<uniform> optics: Optics;
@group(0) @binding(2) var<uniform> settings: Settings;
@group(0) @binding(3) var<storage, read> tiles: array<Tile>;
@group(0) @binding(4) var output: texture_storage_2d<rgba16float, write>;
fn optical(p: vec2<i32>) -> vec4<f32> {
    if any(p < vec2(0)) || any(p >= vec2<i32>(textureDimensions(terrain))) { return vec4(0.,0.,0.,1.); }
    return optics.entries[textureLoad(terrain,p,0).r].value;
}
// Supercover DDA. Side cells at a corner crossing also block light. The final
// destination cell receives surface light, rather than shadowing its own front face.
fn visibility(source: vec2<f32>, destination: vec2<f32>) -> f32 {
    var cell = vec2<i32>(floor(source));
    let end = vec2<i32>(floor(destination));
    if all(cell == end) { return 1.; }
    let delta = destination - source;
    let step = vec2<i32>(sign(delta));
    let stride = 1. / max(abs(delta), vec2(0.000001));
    let edge = vec2<f32>(cell) + select(vec2(0.),vec2(1.),delta > vec2(0.));
    var next = abs(edge - source) * stride;
    next = select(next, vec2(1e20), abs(delta) < vec2(0.000001));
    var value = 1.-optical(cell).a;
    for (var i=0u; i<260u; i++) {
        if all(cell == end) { return value; }
        if abs(next.x-next.y) < 0.000001 {
            let a = cell + vec2(step.x,0);
            let b = cell + vec2(0,step.y);
            if any(a != end) { value *= 1.-optical(a).a; }
            if any(b != end) { value *= 1.-optical(b).a; }
            cell += step;
            next += stride;
        } else if next.x < next.y {
            cell.x += step.x; next.x += stride.x;
        } else {
            cell.y += step.y; next.y += stride.y;
        }
        if any(cell != end) { value *= 1.-optical(cell).a; }
        if value < 0.001 { return 0.; }
    }
    return 0.; // Validated radius <=128 bounds traversal; never return leaked light.
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = tiles[id.z].origin + id.xy;
    if any(p >= textureDimensions(terrain)) { return; }
    let destination = vec2<f32>(p) + vec2(.5);
    let material = optical(vec2<i32>(p));
    var depth = settings.values.z;
    if material.a > 0. {
        let directions = array(vec2(1,0),vec2(-1,0),vec2(0,1),vec2(0,-1),vec2(1,1),vec2(-1,1),vec2(1,-1),vec2(-1,-1));
        for (var ray=0u; ray<8u; ray++) {
            for (var distance=1u; distance<=u32(settings.values.z); distance++) {
                if optical(vec2<i32>(p)+directions[ray]*i32(distance)).a < .5 {
                    depth = min(depth, f32(distance)); break;
                }
            }
        }
    } else { depth = 0.; }
    let ambient = settings.values.x * (1.-settings.values.y*depth/settings.values.z);
    var light = vec3(ambient);
    for (var i=0u; i<u32(settings.values.w); i++) {
        let source = settings.lights[i];
        let distance = length(source.position_radius.xy-destination);
        if distance >= source.position_radius.z { continue; }
        let attenuation = 1.-distance/source.position_radius.z;
        light += source.color.rgb * attenuation * attenuation * visibility(source.position_radius.xy,destination);
    }
    textureStore(output,vec2<i32>(p),vec4(light+material.rgb,1.));
}
