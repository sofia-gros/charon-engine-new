@group(0) @binding(0) var cells: texture_2d<f32>;
@group(0) @binding(1) var<uniform> camera: vec4<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let p = positions[index];
    var out: VertexOutput;
    out.position = vec4(p, 0.0, 1.0);
    out.uv = vec2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return out;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = textureDimensions(cells);
    let pixel = vec2<i32>(floor(camera.xy+in.uv*camera.zw));
    if any(pixel < vec2(0)) || any(pixel >= vec2<i32>(size)) { return vec4(0.,0.,0.,1.); }
    return textureLoad(cells, pixel, 0);
}
