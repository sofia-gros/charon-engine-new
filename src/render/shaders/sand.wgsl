// Fullscreen presentation + bloom for emissive cells.
//
// Pass 1 (`fs_main`): sample the grid texture with nearest-neighbour
// filtering so individual cells stay crisp. Emissive cells (fire, acid)
// already carry their bright base color from `CellData::to_rgba`.
//
// Pass 2 (`fs_bloom`): a separable 9-tap Gaussian blur of the emissive
// channel, added back on top of the base image for the glow. We approximate
// the emissive mask by extracting the luminance above a fixed threshold —
// only fire and acid hit that threshold under the current palette.

struct Uniforms {
    inv_size: vec2<f32>,
    bloom_strength: f32,
    _pad: f32,
};

@group(0) @binding(0) var grid_tex: texture_2d<f32>;
@group(0) @binding(1) var grid_sampler: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;

struct VsOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOutput {
    let x = f32(i32(vi % 2u)) * 4.0 - 1.0;
    let y = f32(i32(vi / 2u)) * 4.0 - 1.0;
    var out: VsOutput;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// Bright-pass: anything brighter than `threshold` contributes to bloom.
fn bright_pass(c: vec3<f32>) -> vec3<f32> {
    let threshold = 0.78;
    let l = luma(c);
    let knee = max(l - threshold, 0.0) / max(1.0 - threshold, 1e-4);
    return c * knee;
}

@fragment
fn fs_main(in: VsOutput) -> @location(0) vec4<f32> {
    let base = textureSample(grid_tex, grid_sampler, in.uv).rgb;

    // 9-tap Gaussian on the bright pass, separable approximation done in one pass.
    var bloom = vec3<f32>(0.0);
    let weights = array<f32, 5>(0.227027, 0.194594, 0.121622, 0.054054, 0.016216);
    let radius = 6.0;

    // Horizontal
    bloom += bright_pass(base) * weights[0];
    for (var i: i32 = 1; i < 5; i++) {
        let off = f32(i) * radius * u.inv_size.x;
        let a = textureSample(grid_tex, grid_sampler, in.uv + vec2<f32>(off, 0.0)).rgb;
        let b = textureSample(grid_tex, grid_sampler, in.uv - vec2<f32>(off, 0.0)).rgb;
        bloom += (bright_pass(a) + bright_pass(b)) * weights[i];
    }
    // Vertical
    for (var i: i32 = 1; i < 5; i++) {
        let off = f32(i) * radius * u.inv_size.y;
        let a = textureSample(grid_tex, grid_sampler, in.uv + vec2<f32>(0.0, off)).rgb;
        let b = textureSample(grid_tex, grid_sampler, in.uv - vec2<f32>(0.0, off)).rgb;
        bloom += (bright_pass(a) + bright_pass(b)) * weights[i];
    }

    var color = base + bloom * u.bloom_strength;

    // Cheap film-style tonemap (Reinhard) so highlights don't clip.
    color = color / (1.0 + color);
    // Subtle vignette to draw the eye in.
    let v = in.uv - vec2<f32>(0.5);
    let vig = 1.0 - dot(v, v) * 0.35;
    color *= vig;

    return vec4<f32>(color, 1.0);
}
