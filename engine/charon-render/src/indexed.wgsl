@group(0) @binding(0) var indices: texture_2d<u32>;
struct Appearance { color: vec4<u32>, rect: vec4<u32> };
struct Palette { entries: array<Appearance, 256> };
@group(0) @binding(1) var<uniform> palette: Palette;
@group(0) @binding(3) var<uniform> camera: vec4<f32>;
@group(0) @binding(2) var atlas: texture_2d<f32>;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    let p = array(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.))[i];
    var out: Out;
    out.position = vec4(p,0.,1.);
    out.uv = vec2((p.x+1.)*0.5,(1.-p.y)*0.5);
    return out;
}
@fragment fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(indices);
    let pixel = vec2<i32>(floor(camera.xy+in.uv*camera.zw));
    if any(pixel < vec2(0)) || any(pixel >= vec2<i32>(dimensions)) { return vec4(0.,0.,0.,1.); }
    let material = textureLoad(indices,pixel,0).r;
    let look = palette.entries[material];
    let color = vec4<f32>(look.color)/255.;
    if look.rect.z == 0u { return color; }
    let uv = vec2<u32>(pixel)%look.rect.zw+look.rect.xy;
    let texel = textureLoad(atlas,vec2<i32>(uv),0);
    return vec4(texel.rgb*texel.a+color.rgb*(1.-texel.a),color.a);
}
