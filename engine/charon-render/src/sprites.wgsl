struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> Out {
    var out: Out;
    out.position = vec4<f32>(position, 0., 1.);
    out.uv = uv;
    return out;
}
@group(0) @binding(0) var atlas: texture_2d<f32>;
@fragment fn fs_main(in: Out) -> @location(0) vec4<f32> {
    return textureLoad(atlas, vec2<i32>(floor(in.uv)), 0);
}
