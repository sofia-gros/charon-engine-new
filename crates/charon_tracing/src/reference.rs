use super::*;
#[derive(Clone, Copy)]
struct Hit {
    index: Option<usize>,
    pos: [f32; 2],
    normal: [f32; 2],
    trans: [f32; 3],
    distance: f32,
    escaped: bool,
}
fn random(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / 16777216.
}
fn walk(scene: &TraceScene, p: [f32; 2], d: [f32; 2], range: f32, max_steps: u32) -> Hit {
    let g = scene.input.geometry;
    let mut cell = [p[0].floor() as i32, p[1].floor() as i32];
    let mut trans = [1.; 3];
    let step = d.map(|v| if v < 0. { -1 } else { 1 });
    let delta = d.map(|v| if v.abs() < 1e-7 { 1e20 } else { 1. / v.abs() });
    let mut edge = std::array::from_fn::<_, 2, _>(|a| {
        if step[a] > 0 {
            (cell[a] as f32 + 1. - p[a]) * delta[a]
        } else {
            (p[a] - cell[a] as f32) * delta[a]
        }
    });
    for _ in 0..max_steps {
        let axis = usize::from(edge[1] < edge[0]);
        let t = edge[axis];
        if t >= range {
            return Hit {
                index: None,
                pos: [p[0] + d[0] * range, p[1] + d[1] * range],
                normal: [0.; 2],
                trans,
                distance: range,
                escaped: false,
            };
        }
        cell[axis] += step[axis];
        edge[axis] += delta[axis];
        let mut normal = [0.; 2];
        normal[axis] = -step[axis] as f32;
        let pos = [p[0] + d[0] * t, p[1] + d[1] * t];
        if cell[0] < 0 || cell[1] < 0 || cell[0] >= g.width as i32 || cell[1] >= g.height as i32 {
            return Hit {
                index: None,
                pos,
                normal,
                trans,
                distance: t,
                escaped: true,
            };
        }
        let index = cell[1] as usize * g.width as usize + cell[0] as usize;
        let optic = scene.input.optics[index];
        if optic.opacity >= 0.999 {
            return Hit {
                index: Some(index),
                pos,
                normal,
                trans,
                distance: t,
                escaped: false,
            };
        }
        for (c, value) in trans.iter_mut().enumerate() {
            *value *= (1. - optic.opacity) * optic.transmission[c];
        }
    }
    // Budget exhaustion is occluded, never an untested clear path.
    Hit {
        index: None,
        pos: p,
        normal: [0.; 2],
        trans: [0.; 3],
        distance: 0.,
        escaped: false,
    }
}
pub fn visibility(
    scene: &TraceScene,
    from: [f32; 2],
    to: [f32; 2],
    max_steps: u32,
) -> Result<[f32; 3], String> {
    scene.validate()?;
    if from.iter().chain(to.iter()).any(|v| !v.is_finite()) || max_steps == 0 {
        return Err("invalid visibility ray".into());
    }
    let g = scene.input.geometry;
    if g.index(from).is_none() || g.index(to).is_none() {
        return Err("visibility endpoints must be inside the optical grid".into());
    }
    let p = std::array::from_fn(|a| (from[a] - g.origin[a]) / g.cell_size);
    let q: [f32; 2] = std::array::from_fn(|a| (to[a] - g.origin[a]) / g.cell_size);
    Ok(visible(scene, p, q, max_steps))
}
fn visible(scene: &TraceScene, p: [f32; 2], q: [f32; 2], steps: u32) -> [f32; 3] {
    let v = [q[0] - p[0], q[1] - p[1]];
    let distance = v[0].hypot(v[1]);
    if distance < 1e-5 {
        return [1.; 3];
    }
    let d = v.map(|v| v / distance);
    let hit = walk(scene, p, d, distance, steps);
    let target = (q[1].floor() as u32 * scene.input.geometry.width + q[0].floor() as u32) as usize;
    if hit.distance >= distance - 0.001 || hit.index == Some(target) {
        hit.trans
    } else {
        [0.; 3]
    }
}
fn point(scene: &TraceScene, index: u32) -> [f32; 2] {
    let g = scene.input.geometry;
    [
        (index % g.width) as f32 + 0.5,
        (index / g.width) as f32 + 0.5,
    ]
}
fn direct(
    scene: &TraceScene,
    quality: Quality,
    emitters: &[Emitter],
    p: [f32; 2],
    normal: [f32; 2],
    range: f32,
) -> [f32; 3] {
    let g = scene.input.geometry;
    let mut light = [0.; 3];
    for emitter in emitters {
        let q = std::array::from_fn(|a| (emitter.position[a] - g.origin[a]) / g.cell_size);
        let v: [f32; 2] = [q[0] - p[0], q[1] - p[1]];
        let distance = v[0].hypot(v[1]);
        if distance > range {
            continue;
        }
        let cosine = if normal == [0.; 2] || distance < 1e-5 {
            1.
        } else {
            ((normal[0] * v[0] + normal[1] * v[1]) / distance).max(0.)
        };
        let trans = visible(scene, p, q, quality.max_steps);
        let world = distance * g.cell_size;
        let weight =
            cosine * scene.input.settings.falloff_per_unit.powf(world) / (1. + world / 16.);
        for c in 0..3 {
            light[c] += trans[c] * emitter.radiance[c] * weight;
        }
    }
    for d in [
        [-0.8660254, -0.5],
        [-0.5, -0.8660254],
        [0., -1.],
        [0.5, -0.8660254],
        [0.8660254, -0.5],
    ] {
        let cosine = if normal == [0.; 2] {
            1.
        } else {
            (normal[0] * d[0] + normal[1] * d[1]).max(0.)
        };
        let hit = walk(scene, p, d, range, quality.max_steps);
        if hit.escaped {
            let weight = 0.2
                * cosine
                * scene
                    .input
                    .settings
                    .falloff_per_unit
                    .powf(hit.distance * g.cell_size);
            for (c, value) in light.iter_mut().enumerate() {
                *value += hit.trans[c] * scene.input.settings.sky[c] * weight;
            }
        }
    }
    light
}
pub(crate) fn direct_primary(
    scene: &TraceScene,
    quality: Quality,
    emitters: &[Emitter],
    index: u32,
) -> [f32; 3] {
    direct(
        scene,
        quality,
        emitters,
        point(scene, index),
        [0.; 2],
        scene.input.settings.max_distance / scene.input.geometry.cell_size,
    )
}
pub(crate) fn sample(
    scene: &TraceScene,
    quality: Quality,
    emitters: &[Emitter],
    index: u32,
    sample: u32,
) -> [f32; 3] {
    if quality.bounces == 0 || scene.input.optics[index as usize].opacity >= 0.999 {
        return [0.; 3];
    }
    let g = scene.input.geometry;
    let mut rng = index.wrapping_mul(747796405) ^ sample.wrapping_mul(2891336453) ^ 0x9e3779b9;
    if rng == 0 {
        rng = 1;
    }
    let mut pos = point(scene, index);
    let angle = random(&mut rng) * std::f32::consts::TAU;
    let mut dir = [angle.cos(), angle.sin()];
    let mut throughput = [1.; 3];
    let mut radiance = [0.; 3];
    let mut range = scene.input.settings.max_distance / g.cell_size;
    for _ in 0..quality.bounces {
        let hit = walk(scene, pos, dir, range, quality.max_steps);
        let Some(cell) = hit.index else {
            break;
        };
        range -= hit.distance;
        if range <= 0.001 {
            break;
        }
        for (c, value) in throughput.iter_mut().enumerate() {
            *value *= hit.trans[c]
                * scene.reflectance[cell][c]
                * scene
                    .input
                    .settings
                    .falloff_per_unit
                    .powf(hit.distance * g.cell_size);
        }
        pos = [
            hit.pos[0] + hit.normal[0] * 0.001,
            hit.pos[1] + hit.normal[1] * 0.001,
        ];
        let bounce = direct(scene, quality, emitters, pos, hit.normal, range);
        for c in 0..3 {
            radiance[c] += throughput[c] * bounce[c];
        }
        let tangent = [-hit.normal[1], hit.normal[0]];
        let s = random(&mut rng) * 2. - 1.;
        let c = (1. - s * s).sqrt();
        dir = [
            hit.normal[0] * c + tangent[0] * s,
            hit.normal[1] * c + tangent[1] * s,
        ];
    }
    radiance
}
pub(crate) fn filter_indirect(
    scene: &TraceScene,
    values: &[[f32; 3]],
    steps: u32,
) -> Vec<[f32; 3]> {
    let g = scene.input.geometry;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if scene.input.optics[index].opacity >= 0.999 {
                return *value;
            }
            let p = point(scene, index as u32);
            let mut sum = *value;
            let mut weight = 1.;
            for dy in -2_i32..=2 {
                for dx in -2_i32..=2 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let x = index as i32 % g.width as i32 + dx;
                    let y = index as i32 / g.width as i32 + dy;
                    if x < 0 || y < 0 || x >= g.width as i32 || y >= g.height as i32 {
                        continue;
                    }
                    let j = (y as u32 * g.width + x as u32) as usize;
                    if scene.input.optics[j].opacity >= 0.999 {
                        continue;
                    }
                    let q = point(scene, j as u32);
                    if visible(scene, p, q, steps).iter().any(|v| *v < 0.05) {
                        continue;
                    }
                    let w = 1. / (1. + (dx * dx + dy * dy) as f32);
                    for (c, value) in sum.iter_mut().enumerate() {
                        *value += values[j][c] * w;
                    }
                    weight += w;
                }
            }
            sum.map(|v| v / weight)
        })
        .collect()
}
pub fn solve_sample(scene: &TraceScene, quality: Quality) -> Result<LightField, String> {
    Accumulator::default().update(scene, quality)
}
