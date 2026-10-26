//! Reference implementation for the HDR surface renderer. Physics stays 2D;
//! intersections use cell prisms and Beer–Lambert absorption in world units.
use crate::{HdrField, Quality, TraceScene};
type V = [f32; 3];
pub fn add(a: V, b: V) -> V {
    std::array::from_fn(|k| a[k] + b[k])
}
pub fn mul(a: V, b: V) -> V {
    std::array::from_fn(|k| a[k] * b[k])
}
pub fn scale(a: V, b: f32) -> V {
    a.map(|v| v * b)
}
pub fn dot(a: V, b: V) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
pub fn normalize(a: V) -> V {
    scale(a, 1. / dot(a, a).sqrt().max(1e-10))
}
fn random(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / 16777216.
}
fn seed(index: u32, sample: u32) -> u32 {
    (index.wrapping_mul(747796405) ^ sample.wrapping_mul(2891336453) ^ 0x9e3779b9).max(1)
}
fn max3(v: V) -> f32 {
    v.into_iter().fold(0., f32::max)
}
const EPS: f32 = 0.002;
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub index: usize,
    pub distance: f32,
    pub normal: V,
    pub position: V,
}
fn occupied(s: &TraceScene, i: usize) -> bool {
    s.input.optics[i].opacity >= 0.999
        || s.materials[i].dielectric
        || max3(s.materials[i].emission) > 0.
}
/// Axis-aligned prism intersection. The returned normal points out of the prism.
fn slab(p: V, d: V, lo: V, hi: V) -> Option<(f32, V)> {
    let mut enter = -1e20_f32;
    let mut exit = 1e20_f32;
    let mut en = [0.; 3];
    let mut ex = [0.; 3];
    for k in 0..3 {
        if d[k].abs() < 1e-7 {
            if p[k] < lo[k] || p[k] > hi[k] {
                return None;
            }
            continue;
        }
        let a = (lo[k] - p[k]) / d[k];
        let b = (hi[k] - p[k]) / d[k];
        let near = a.min(b);
        let far = a.max(b);
        if near > enter {
            enter = near;
            en = [0.; 3];
            en[k] = if d[k] > 0. { -1. } else { 1. };
        }
        if far < exit {
            exit = far;
            ex = [0.; 3];
            ex[k] = if d[k] > 0. { 1. } else { -1. };
        }
    }
    if exit < enter || exit < EPS {
        None
    } else if enter >= EPS {
        Some((enter, en))
    } else {
        Some((exit, ex))
    }
}
/// Same prism geometry as the hardware boundary mesh, traversed without RT APIs.
pub fn intersect(s: &TraceScene, p: V, d: V, range: f32, max_steps: u32) -> Option<Hit> {
    let g = s.input.geometry;
    let size = g.cell_size;
    let mut t = 0_f32;
    for k in 0..2 {
        if p[k] < g.origin[k] || p[k] >= g.origin[k] + [g.width, g.height][k] as f32 * size {
            if d[k].abs() < 1e-7 {
                return None;
            }
            let a = (g.origin[k] - p[k]) / d[k];
            let b = (g.origin[k] + [g.width, g.height][k] as f32 * size - p[k]) / d[k];
            t = t.max(a.min(b) + EPS);
        }
    }
    let q = add(p, scale(d, t));
    let mut cell = [
        ((q[0] - g.origin[0]) / size).floor() as i32,
        ((q[1] - g.origin[1]) / size).floor() as i32,
    ];
    for _ in 0..max_steps {
        if cell[0] < 0
            || cell[1] < 0
            || cell[0] >= g.width as i32
            || cell[1] >= g.height as i32
            || t > range
        {
            return None;
        }
        let i = cell[1] as usize * g.width as usize + cell[0] as usize;
        let lo = [
            g.origin[0] + cell[0] as f32 * size,
            g.origin[1] + cell[1] as f32 * size,
            -s.materials[i].depth,
        ];
        let hi = [lo[0] + size, lo[1] + size, s.materials[i].depth];
        let mut edges = [1e20_f32; 2];
        for k in 0..2 {
            if d[k].abs() > 1e-7 {
                edges[k] = ((if d[k] > 0. { hi[k] } else { lo[k] }) - p[k]) / d[k];
            }
        }
        let axis = if edges[0] < edges[1] { 0 } else { 1 };
        let next = edges[axis];
        if occupied(s, i) {
            for attempt in 0..2 {
                let offset = if attempt == 0 { 0. } else { t + EPS * 2. };
                if let Some((distance, normal)) = slab(add(p, scale(d, offset)), d, lo, hi) {
                    let distance = distance + offset;
                    if distance <= range && distance >= t - EPS && distance <= next + EPS {
                        let at = add(p, scale(d, distance));
                        let probe = add(
                            at,
                            scale(
                                d,
                                if dot(d, normal) > 0. {
                                    EPS * 2.
                                } else {
                                    -EPS * 2.
                                },
                            ),
                        );
                        let neighbor = g.index([probe[0], probe[1]]);
                        if dot(d, normal) > 0. && s.input.optics[i].opacity < 0.999 {
                            if let Some(j) = neighbor {
                                if s.input.optics[j].opacity >= 0.999
                                    && probe[2].abs() < s.materials[j].depth
                                {
                                    return Some(Hit {
                                        index: j,
                                        distance,
                                        normal: scale(normal, -1.),
                                        position: at,
                                    });
                                }
                            }
                        }
                        let same = neighbor.is_some_and(|j| {
                            j != i
                                && occupied(s, j)
                                && (s.input.optics[j].opacity >= 0.999)
                                    == (s.input.optics[i].opacity >= 0.999)
                                && s.materials[j].dielectric == s.materials[i].dielectric
                                && s.materials[j].ior == s.materials[i].ior
                                && probe[2].abs() < s.materials[j].depth
                        });
                        if !same {
                            return Some(Hit {
                                index: i,
                                distance,
                                normal,
                                position: at,
                            });
                        }
                    }
                }
            }
        }
        if next >= range || next > 1e19 {
            return None;
        }
        t = next;
        cell[axis] += if d[axis] > 0. { 1 } else { -1 };
    }
    // A budget-exhausted ray must block, never become an environment sample.
    Some(Hit {
        index: usize::MAX,
        distance: t,
        normal: [0.; 3],
        position: add(p, scale(d, t)),
    })
}

fn attenuation(
    s: &TraceScene,
    p: V,
    d: V,
    length: f32,
    steps: u32,
    shadow: bool,
    target: Option<usize>,
) -> V {
    // Integrate actual cell segment lengths, including the starting cell.
    let g = s.input.geometry;
    let mut t = 0_f32;
    let mut trans = [1.; 3];
    for k in 0..2 {
        let hi = g.origin[k] + [g.width, g.height][k] as f32 * g.cell_size;
        if p[k] < g.origin[k] || p[k] >= hi {
            if d[k].abs() < 1e-7 {
                return trans;
            }
            t = t.max(((g.origin[k] - p[k]) / d[k]).min((hi - p[k]) / d[k]) + EPS);
        }
    }
    for _ in 0..steps {
        if t >= length - EPS {
            return trans;
        }
        let q = add(p, scale(d, t + EPS * 0.1));
        let Some(i) = g.index([q[0], q[1]]) else {
            return trans;
        };
        let xy = [i as u32 % g.width, i as u32 / g.width];
        let mut next = length;
        for k in 0..2 {
            if d[k].abs() > 1e-7 {
                let edge =
                    g.origin[k] + (xy[k] as f32 + if d[k] > 0. { 1. } else { 0. }) * g.cell_size;
                let candidate = (edge - p[k]) / d[k];
                if candidate > t + 1e-6 {
                    next = next.min(candidate);
                }
            }
        }
        let m = s.materials[i];
        if d[2].abs() > 1e-7 {
            for plane in [-m.depth, m.depth] {
                let crossing = (plane - p[2]) / d[2];
                if crossing > t + 1e-6 {
                    next = next.min(crossing);
                }
            }
        }
        if q[2].abs() < m.depth {
            if shadow && s.input.optics[i].opacity >= 0.999 && next - t > EPS * 0.1 {
                return if Some(i) == target { trans } else { [0.; 3] };
            }
            let o = s.input.optics[i];
            let sigma: V = std::array::from_fn(|k| {
                m.absorption[k]
                    - (o.transmission[k] * (1. - o.opacity.min(0.998)))
                        .max(1e-6)
                        .ln()
            });
            trans = mul(trans, sigma.map(|a| (-a * (next - t)).exp()));
        }
        t = next;
    }
    if t < length - EPS { [0.; 3] } else { trans }
}
pub fn visibility(s: &TraceScene, p: V, q: V, steps: u32) -> V {
    let delta = add(q, scale(p, -1.));
    let distance = dot(delta, delta).sqrt();
    if distance < EPS {
        return [1.; 3];
    }
    attenuation(
        s,
        p,
        scale(delta, 1. / distance),
        distance,
        steps,
        true,
        s.input.geometry.index([q[0], q[1]]),
    )
}

fn environment(s: &TraceScene, p: V, d: V, range: f32) -> V {
    if p[1] + d[1] * range < s.input.geometry.origin[1] {
        scale(s.input.settings.sky, (-d[1]).max(0.))
    } else {
        [0.; 3]
    }
}
// Fixed quadrature for the visible sky; sky visibility must not be a binary
// per-pixel Monte Carlo event in each freshly invalidated particle frame.
fn stable_sky(s: &TraceScene, p: V, n: V, steps: u32) -> V {
    if max3(s.input.settings.sky) == 0. {
        return [0.; 3];
    }
    let mut sum = [0.; 3];
    let mut rng = 0x51ed270b;
    for _ in 0..8 {
        let d = hemisphere(n, &mut rng);
        let env = environment(s, p, d, s.input.settings.max_distance);
        if max3(env) > 0. {
            sum = add(
                sum,
                mul(
                    env,
                    attenuation(s, p, d, s.input.settings.max_distance, steps, true, None),
                ),
            );
        }
    }
    scale(sum, 1. / 8.)
}
fn hemisphere(n: V, rng: &mut u32) -> V {
    let r = random(rng).sqrt();
    let angle = random(rng) * std::f32::consts::TAU;
    let tangent = normalize(if n[2].abs() < 0.9 {
        [-n[1], n[0], 0.]
    } else {
        [1., 0., 0.]
    });
    let b = [
        n[1] * tangent[2] - n[2] * tangent[1],
        n[2] * tangent[0] - n[0] * tangent[2],
        n[0] * tangent[1] - n[1] * tangent[0],
    ];
    normalize(add(
        add(scale(tangent, r * angle.cos()), scale(b, r * angle.sin())),
        scale(n, (1. - r * r).sqrt()),
    ))
}
fn point(s: &TraceScene, i: usize, rng: &mut u32) -> V {
    let g = s.input.geometry;
    [
        g.origin[0] + (i as u32 % g.width) as f32 * g.cell_size + random(rng) * g.cell_size,
        g.origin[1] + (i as u32 / g.width) as f32 * g.cell_size + random(rng) * g.cell_size,
        if occupied(s, i) {
            s.materials[i].depth + EPS * 2.
        } else {
            0.
        },
    ]
}
fn light_point(s: &TraceScene, i: usize, rng: &mut u32) -> V {
    let mut q = point(s, i, rng);
    let g = s.input.geometry;
    if max3(s.materials[i].emission) == 0. && s.input.optics[i].emission_strength == 0. {
        let center = [
            g.origin[0] + (i as u32 % g.width) as f32 * g.cell_size + g.cell_size * 0.5,
            g.origin[1] + (i as u32 / g.width) as f32 * g.cell_size + g.cell_size * 0.5,
        ];
        for k in 0..2 {
            q[k] = center[k] + (q[k] - center[k]) * 2. / g.cell_size;
        }
    }
    q[2] = s.materials[i].depth + 2.;
    q
}
fn source_area(s: &TraceScene, i: usize) -> f32 {
    if max3(s.materials[i].emission) > 0. || s.input.optics[i].emission_strength > 0. {
        s.input.geometry.cell_size.powi(2)
    } else {
        64.
    }
}
/// Power-weighted emitter CDF shared with the GPU upload. Bright compact lights
/// must not get the same probability as one dim pixel in a large flame.
pub fn emitter_distribution(s: &TraceScene, emitters: &[u32]) -> Vec<(u32, f32, f32)> {
    let weights: Vec<_> = emitters
        .iter()
        .map(|&i| {
            let c = s.radiance(i as usize);
            (c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722) * source_area(s, i as usize)
        })
        .collect();
    let total = weights.iter().sum::<f32>().max(1e-30);
    let mut cumulative = 0.;
    weights
        .iter()
        .enumerate()
        .map(|(j, w)| {
            let pdf = *w / total;
            cumulative += pdf;
            (
                emitters[j],
                if j + 1 == weights.len() {
                    1.
                } else {
                    cumulative
                },
                pdf,
            )
        })
        .collect()
}
fn choose_light(distribution: &[(u32, f32, f32)], u: f32) -> (usize, f32) {
    let j = distribution
        .partition_point(|e| e.1 <= u)
        .min(distribution.len() - 1);
    (distribution[j].0 as usize, distribution[j].2)
}
fn direct(
    s: &TraceScene,
    p: V,
    n: V,
    rng: &mut u32,
    emitters: &[u32],
    steps: u32,
    range: f32,
) -> V {
    if emitters.is_empty() {
        return [0.; 3];
    }
    let distribution = emitter_distribution(s, emitters);
    let (source, pdf) = choose_light(&distribution, random(rng));
    // Each source is an area in world space, jittered for a soft penumbra.
    let q = light_point(s, source, rng);
    let v = add(q, scale(p, -1.));
    let dist = dot(v, v).sqrt();
    if dist < EPS || dist > range {
        return [0.; 3];
    }
    let cosine = dot(n, scale(v, 1. / dist)).max(0.);
    let weight = source_area(s, source) / pdf.max(1e-30) * cosine / (1. + dist * dist)
        * s.input.settings.falloff_per_unit.powf(dist);
    scale(
        mul(
            s.radiance(source),
            visibility(s, add(p, scale(n, EPS * 2.)), q, steps),
        ),
        weight,
    )
}
/// Deterministic area-light quadrature for visible surfaces and reflected hits.
/// Small emitter sets are integrated exhaustively; large sets use stratified
/// power sampling. The quadrature has no frame/pixel random seed.
fn stable_direct(
    s: &TraceScene,
    p: V,
    n: V,
    emitters: &[u32],
    steps: u32,
    range: f32,
    specular: Option<crate::Material>,
) -> V {
    if emitters.is_empty() {
        return [0.; 3];
    }
    let limit = if s.surface_quality.reflection_bounces > 1 {
        64
    } else {
        8
    };
    let count = emitters.len().min(limit);
    let full = emitters.len() <= limit;
    let distribution = emitter_distribution(s, emitters);
    let mut sum = [0.; 3];
    for j in 0..count {
        let (source, probability) = if full {
            (emitters[j] as usize, 1.)
        } else {
            let (i, pdf) = choose_light(&distribution, (j as f32 + 0.5) / count as f32);
            (i, pdf * count as f32)
        };
        let g = s.input.geometry;
        let area = source_area(s, source);
        let side = if max3(s.materials[source].emission) > 0. {
            g.cell_size
        } else {
            2.
        };
        let taps = if side > g.cell_size || specular.is_some() {
            4
        } else {
            1
        };
        for tap in 0..taps {
            let uv = if taps == 1 {
                [0.5, 0.5]
            } else {
                [0.25 + (tap % 2) as f32 * 0.5, 0.25 + (tap / 2) as f32 * 0.5]
            };
            let q = [
                g.origin[0]
                    + (source as u32 % g.width) as f32 * g.cell_size
                    + g.cell_size * 0.5
                    + (uv[0] - 0.5) * side,
                g.origin[1]
                    + (source as u32 / g.width) as f32 * g.cell_size
                    + g.cell_size * 0.5
                    + (uv[1] - 0.5) * side,
                s.materials[source].depth + 2.,
            ];
            let v = add(q, scale(p, -1.));
            let distance = dot(v, v).sqrt();
            if distance < EPS || distance > range {
                continue;
            }
            let l = scale(v, 1. / distance);
            let nl = dot(n, l).max(0.);
            let mut weight = area * nl / (1. + distance * distance)
                * s.input.settings.falloff_per_unit.powf(distance)
                / (probability * taps as f32);
            if let Some(m) = specular {
                let view = normalize([0., -0.85, 0.5268]);
                let h = normalize(add(l, view));
                let nv = dot(n, view).max(0.001);
                let nh = dot(n, h).max(0.);
                let a2 = m.roughness.powi(4).max(0.0025);
                let denom = nh * nh * (a2 - 1.) + 1.;
                let distribution = a2 / (std::f32::consts::PI * denom * denom);
                let k = (m.roughness + 1.).powi(2) / 8.;
                let geometry = nl / (nl * (1. - k) + k).max(0.001) * nv / (nv * (1. - k) + k);
                weight *= distribution * m.fresnel(dot(l, h).max(0.)) * geometry
                    / (4. * nl * nv).max(0.0001);
            }
            sum = add(
                sum,
                scale(
                    mul(
                        s.radiance(source),
                        visibility(s, add(p, scale(n, EPS * 2.)), q, steps),
                    ),
                    weight,
                ),
            );
        }
    }
    sum
}
#[derive(Clone, Copy)]
struct SurfaceRay {
    p: V,
    d: V,
    weight: V,
    range: f32,
    reflections: u32,
    transmissions: u32,
}
/// Specular visibility stays ray traced, but visible diffuse hits use bounded,
/// deterministic lighting rather than a second Monte Carlo diffuse path.
fn surface_trace(s: &TraceScene, p: V, d: V, emitters: &[u32], quality: Quality) -> V {
    let mut stack = vec![SurfaceRay {
        p,
        d,
        weight: [1.; 3],
        range: s.input.settings.max_distance,
        reflections: 0,
        transmissions: 0,
    }];
    let mut radiance = [0.; 3];
    for _ in 0..128 {
        let Some(mut ray) = stack.pop() else {
            break;
        };
        if ray.range < EPS || max3(ray.weight) < 1e-4 {
            continue;
        }
        let Some(h) = intersect(s, ray.p, ray.d, ray.range, quality.max_steps) else {
            radiance = add(
                radiance,
                mul(ray.weight, environment(s, ray.p, ray.d, ray.range)),
            );
            continue;
        };
        if h.index == usize::MAX {
            continue;
        }
        ray.weight = mul(
            ray.weight,
            scale(
                attenuation(s, ray.p, ray.d, h.distance, quality.max_steps, false, None),
                s.input.settings.falloff_per_unit.powf(h.distance),
            ),
        );
        ray.range -= h.distance;
        let m = s.materials[h.index];
        if dot(ray.d, h.normal) < 0. {
            radiance = add(radiance, mul(ray.weight, m.emission));
        }
        let entering = dot(ray.d, h.normal) < 0.;
        let n = if entering {
            h.normal
        } else {
            scale(h.normal, -1.)
        };
        if m.dielectric {
            if ray.transmissions >= s.surface_quality.transmission_bounces {
                continue;
            }
            let eta = if entering { 1. / m.ior } else { m.ior };
            let cosine = -dot(ray.d, n);
            let k = 1. - eta * eta * (1. - cosine * cosine);
            let f = if k < 0. { 1. } else { m.fresnel(cosine) };
            if ray.reflections < s.surface_quality.reflection_bounces && stack.len() < 16 {
                let dir = normalize(add(ray.d, scale(n, 2. * cosine)));
                stack.push(SurfaceRay {
                    p: add(h.position, scale(dir, EPS * 2.)),
                    d: dir,
                    weight: scale(ray.weight, f),
                    reflections: ray.reflections + 1,
                    transmissions: ray.transmissions + 1,
                    ..ray
                });
            }
            if k >= 0. && stack.len() < 16 {
                let dir = normalize(add(scale(ray.d, eta), scale(n, eta * cosine - k.sqrt())));
                stack.push(SurfaceRay {
                    p: add(h.position, scale(dir, EPS * 2.)),
                    d: dir,
                    weight: scale(ray.weight, 1. - f),
                    transmissions: ray.transmissions + 1,
                    ..ray
                });
            }
        } else if s.input.optics[h.index].opacity < 0.999 {
            if ray.transmissions < s.surface_quality.transmission_bounces && stack.len() < 16 {
                stack.push(SurfaceRay {
                    p: add(h.position, scale(ray.d, EPS * 2.)),
                    transmissions: ray.transmissions + 1,
                    ..ray
                });
            }
        } else {
            let f = m.fresnel((-dot(ray.d, n)).max(0.));
            let lighting = add(
                stable_direct(
                    s,
                    h.position,
                    n,
                    emitters,
                    quality.max_steps,
                    ray.range,
                    None,
                ),
                s.input.settings.ambient,
            );
            radiance = add(
                radiance,
                mul(
                    ray.weight,
                    scale(
                        mul(mul(m.diffuse, s.reflectance[h.index]), lighting),
                        1. - f,
                    ),
                ),
            );
            if m.specular > 0. && m.roughness >= 0.25 {
                radiance = add(
                    radiance,
                    mul(
                        ray.weight,
                        stable_direct(
                            s,
                            h.position,
                            n,
                            emitters,
                            quality.max_steps,
                            ray.range,
                            Some(m),
                        ),
                    ),
                );
            }
            if m.roughness < 0.25
                && m.specular > 0.2
                && ray.reflections < s.surface_quality.reflection_bounces
                && stack.len() < 16
            {
                let dir = normalize(add(ray.d, scale(n, -2. * dot(ray.d, n))));
                stack.push(SurfaceRay {
                    p: add(h.position, scale(dir, EPS * 2.)),
                    d: dir,
                    weight: scale(ray.weight, f),
                    reflections: ray.reflections + 1,
                    ..ray
                });
            }
        }
    }
    radiance
}
fn stable_surface(s: &TraceScene, q: Quality, index: usize, emitters: &[u32]) -> V {
    let g = s.input.geometry;
    let m = s.materials[index];
    let n = m.normal;
    let p = [
        g.origin[0] + (index as u32 % g.width) as f32 * g.cell_size + g.cell_size * 0.5,
        g.origin[1] + (index as u32 / g.width) as f32 * g.cell_size + g.cell_size * 0.5,
        if occupied(s, index) {
            m.depth + EPS * 2.
        } else {
            0.
        },
    ];
    let incident = normalize([0., 0.85, -0.5268]);
    let cosine = (-dot(incident, n)).max(0.);
    let f = m.fresnel(cosine);
    let mut radiance = [0.; 3];
    if m.specular > 0. {
        if m.roughness < 0.25 && s.surface_quality.reflection_bounces > 0 {
            let reflected = normalize(add(incident, scale(n, -2. * dot(incident, n))));
            radiance = scale(
                surface_trace(s, add(p, scale(n, EPS * 2.)), reflected, emitters, q),
                f,
            );
        } else if m.roughness >= 0.25 {
            radiance = stable_direct(
                s,
                p,
                n,
                emitters,
                q.max_steps,
                s.input.settings.max_distance,
                Some(m),
            );
        }
    }
    if m.dielectric && s.surface_quality.transmission_bounces > 0 {
        let eta = 1. / m.ior;
        let k = 1. - eta * eta * (1. - cosine * cosine);
        if k > 0. {
            let dir = normalize(add(scale(incident, eta), scale(n, eta * cosine - k.sqrt())));
            radiance = add(
                radiance,
                scale(
                    surface_trace(s, add(p, scale(dir, EPS * 4.)), dir, emitters, q),
                    1. - f,
                ),
            );
        }
    }
    radiance
}
fn trace_diffuse(
    s: &TraceScene,
    p: V,
    d: V,
    rng: &mut u32,
    emitters: &[u32],
    quality: Quality,
) -> V {
    if quality.bounces == 0 {
        return [0.; 3];
    }
    let mut pos = p;
    let mut dir = d;
    let mut throughput = [1.; 3];
    let mut radiance = [0.; 3];
    let mut remaining = s.input.settings.max_distance;
    let limit = quality.bounces;
    let mut reflected = 0;
    let mut transmitted = 0;
    for _ in 0..limit + s.surface_quality.transmission_bounces + 1 {
        if remaining < EPS {
            break;
        }
        let Some(h) = intersect(s, pos, dir, remaining, quality.max_steps) else {
            return add(
                radiance,
                mul(
                    throughput,
                    if reflected > 0 {
                        environment(s, pos, dir, remaining)
                    } else {
                        [0.; 3]
                    },
                ),
            );
        };
        if h.index == usize::MAX {
            return radiance;
        }
        throughput = mul(
            throughput,
            attenuation(s, pos, dir, h.distance, quality.max_steps, false, None),
        );
        throughput = scale(
            throughput,
            s.input.settings.falloff_per_unit.powf(h.distance),
        );
        let m = s.materials[h.index];
        remaining -= h.distance;
        if s.input.optics[h.index].opacity < 0.999 && !m.dielectric {
            if transmitted >= s.surface_quality.transmission_bounces {
                break;
            }
            transmitted += 1;
            pos = add(h.position, scale(dir, EPS * 2.));
            continue;
        }
        if m.dielectric {
            if transmitted >= s.surface_quality.transmission_bounces {
                break;
            }
            let entering = dot(dir, h.normal) < 0.;
            let n = if entering {
                h.normal
            } else {
                scale(h.normal, -1.)
            };
            let eta = if entering { 1. / m.ior } else { m.ior };
            let cos = -dot(dir, n);
            let k = 1. - eta * eta * (1. - cos * cos);
            if k < 0. {
                dir = add(dir, scale(n, 2. * cos));
            } else {
                throughput = scale(throughput, 1. - m.fresnel(cos));
                dir = normalize(add(scale(dir, eta), scale(n, eta * cos - k.sqrt())));
            }
            transmitted += 1;
            pos = add(h.position, scale(dir, EPS * 2.));
        } else {
            let n = if dot(dir, h.normal) < 0. {
                h.normal
            } else {
                scale(h.normal, -1.)
            };
            let diffuse = scale(
                mul(m.diffuse, s.reflectance[h.index]),
                1. - m.fresnel((-dot(dir, n)).max(0.)),
            );
            radiance = add(
                radiance,
                mul(
                    mul(throughput, diffuse),
                    direct(
                        s,
                        h.position,
                        n,
                        rng,
                        emitters,
                        quality.max_steps,
                        remaining,
                    ),
                ),
            );
            if reflected >= limit {
                break;
            }
            throughput = mul(throughput, diffuse);
            dir = hemisphere(n, rng);
            pos = add(h.position, scale(n, EPS * 2.));
            reflected += 1;
        }
        if remaining < EPS || max3(throughput) < 0.0001 {
            break;
        }
    }
    radiance
}
#[derive(Clone, Copy, Default)]
pub struct Components {
    pub direct: V,
    pub indirect: V,
    pub specular: V,
}
pub fn sample(s: &TraceScene, q: Quality, index: u32, sample: u32) -> Components {
    sample_with_emitters(s, q, index, sample, &s.emitters(), true)
}
fn sample_with_emitters(
    s: &TraceScene,
    q: Quality,
    index: u32,
    sample: u32,
    emitters: &[u32],
    visible: bool,
) -> Components {
    let i = index as usize;
    let mut rng = seed(index, sample);
    let p = point(s, i, &mut rng);
    let m = s.materials[i];
    let n = m.normal;
    let g = s.input.geometry;
    let center = [
        g.origin[0] + (index % g.width) as f32 * g.cell_size + g.cell_size * 0.5,
        g.origin[1] + (index / g.width) as f32 * g.cell_size + g.cell_size * 0.5,
        p[2],
    ];
    let direct = {
        stable_direct(
            s,
            center,
            n,
            emitters,
            q.max_steps,
            s.input.settings.max_distance,
            None,
        )
    };
    let direct = add(direct, stable_sky(s, center, n, q.max_steps));
    let mut indirect = trace_diffuse(
        s,
        add(p, scale(n, EPS * 2.)),
        hemisphere(n, &mut rng),
        &mut rng,
        emitters,
        q,
    );
    if s.surface_quality.indirect_clamp > 0. {
        let reference = max3(direct)
            .max(max3(s.input.settings.sky) * 0.125 + max3(s.input.settings.ambient) * 2. + 0.01);
        let ceiling = reference * s.surface_quality.indirect_clamp;
        indirect = scale(indirect, (ceiling / max3(indirect).max(1e-20)).min(1.));
    }
    let specular = if visible {
        stable_surface(s, q, i, emitters)
    } else {
        [0.; 3]
    };
    Components {
        direct,
        indirect,
        specular,
    }
}
/// Conservative influence mask. Ray paths have a total world-space distance
/// budget; dilating all changed cells by that budget also covers indirect paths.
pub fn invalidation(old: Option<&TraceScene>, new: &TraceScene) -> Vec<u32> {
    let g = new.input.geometry;
    let n = g.len();
    let Some(old) = old else {
        return vec![1; n];
    };
    if old.input.geometry != g
        || old.input.settings != new.input.settings
        || old.surface_quality != new.surface_quality
    {
        return vec![1; n];
    }
    let w = g.width as usize;
    let h = g.height as usize;
    let stride = w + 1;
    let mut difference = vec![0_i32; (w + 1) * (h + 1)];
    let radius = ((new.input.settings.max_distance + 2.) / g.cell_size).ceil() as usize + 1;
    for i in 0..n {
        if old.input.optics[i] == new.input.optics[i]
            && old.input.sources[i] == new.input.sources[i]
            && old.materials[i] == new.materials[i]
            && old.reflectance[i] == new.reflectance[i]
        {
            continue;
        }
        let x = i % w;
        let y = i / w;
        let x0 = x.saturating_sub(radius);
        let y0 = y.saturating_sub(radius);
        let x1 = (x + radius + 1).min(w);
        let y1 = (y + radius + 1).min(h);
        difference[y0 * stride + x0] += 1;
        difference[y0 * stride + x1] -= 1;
        difference[y1 * stride + x0] -= 1;
        difference[y1 * stride + x1] += 1;
    }
    let local_changed: Vec<bool> = (0..n)
        .map(|i| {
            old.input.optics[i] != new.input.optics[i]
                || old.input.sources[i] != new.input.sources[i]
                || old.materials[i] != new.materials[i]
                || old.reflectance[i] != new.reflectance[i]
        })
        .collect();
    let dynamic_only = (0..n)
        .filter(|&i| local_changed[i])
        .all(|i| old.materials[i].dynamic || new.materials[i].dynamic);
    let mut mask = vec![0; n];
    for y in 0..h {
        for x in 0..w {
            let i = y * stride + x;
            let a = if x > 0 { difference[i - 1] } else { 0 };
            let b = if y > 0 { difference[i - stride] } else { 0 };
            let c = if x > 0 && y > 0 {
                difference[i - stride - 1]
            } else {
                0
            };
            difference[i] += a + b - c;
            let j = y * w + x;
            mask[j] = if difference[i] <= 0 {
                0
            } else if dynamic_only && !local_changed[j] {
                2
            } else {
                1
            };
        }
    }
    mask
}
#[derive(Default)]
pub struct Accumulator {
    old: Option<TraceScene>,
    quality: Option<Quality>,
    indirect: Vec<V>,
    counts: Vec<u32>,
    frame: u32,
    field: Option<HdrField>,
}
impl Accumulator {
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }
    pub fn update(&mut self, s: &TraceScene, q: Quality) -> Result<HdrField, String> {
        s.validate()?;
        q.validate(crate::Mode::Path, crate::Platform::Pc)?;
        let n = s.input.geometry.len();
        let mut dirty = invalidation(self.old.as_ref(), s);
        if self.counts.len() != n || self.quality != Some(q) {
            self.indirect = vec![[0.; 3]; n];
            self.counts = vec![0; n];
            dirty.fill(1);
        }
        if self.old.as_ref().is_some_and(|old| old.same_input(s))
            && self.counts.iter().all(|c| *c >= q.max_samples)
        {
            return Ok(self.field.clone().unwrap());
        }
        let mut f = HdrField {
            geometry: s.input.geometry,
            revision: s.input.revision,
            diffuse: vec![[0.; 3]; n],
            specular: vec![[0.; 3]; n],
            emission: s.materials.iter().map(|m| m.emission).collect(),
        };
        let emitters = s.emitters();
        for i in 0..n {
            if dirty[i] == 2 {
                // Retain a short, exponentially refreshed indirect history in
                // cells influenced by moving particles. Direct/surface terms
                // are always current; changed receiver cells reset immediately.
                let retained = self.counts[i]
                    .min((s.surface_quality.dynamic_history - 1) * q.samples_per_update);
                self.indirect[i] = scale(
                    self.indirect[i],
                    retained as f32 / self.counts[i].max(1) as f32,
                );
                self.counts[i] = retained;
            } else if dirty[i] == 1 {
                self.counts[i] = 0;
                self.indirect[i] = [0.; 3];
            }
            let cap = if s.materials[i].dynamic && dirty[i] != 0 {
                (s.surface_quality.dynamic_history * q.samples_per_update).min(q.max_samples)
            } else {
                q.max_samples
            };
            let count = q.samples_per_update.min(q.max_samples - self.counts[i]);
            for j in 0..count {
                let c = sample_with_emitters(
                    s,
                    q,
                    i as u32,
                    self.frame
                        .wrapping_mul(q.samples_per_update)
                        .wrapping_add(j),
                    &emitters,
                    false,
                );
                if self.counts[i] < cap {
                    self.indirect[i] = add(self.indirect[i], c.indirect);
                    self.counts[i] += 1;
                } else if s.materials[i].dynamic {
                    let a = 1. / cap as f32;
                    self.indirect[i] = add(scale(self.indirect[i], 1. - a), c.indirect);
                }
            }
            let indirect = scale(self.indirect[i], 1. / self.counts[i].max(1) as f32);
            let g = s.input.geometry;
            let m = s.materials[i];
            let p = [
                g.origin[0] + (i as u32 % g.width) as f32 * g.cell_size + g.cell_size * 0.5,
                g.origin[1] + (i as u32 / g.width) as f32 * g.cell_size + g.cell_size * 0.5,
                if occupied(s, i) {
                    m.depth + EPS * 2.
                } else {
                    0.
                },
            ];
            let direct = stable_direct(
                s,
                p,
                m.normal,
                &emitters,
                q.max_steps,
                s.input.settings.max_distance,
                None,
            );
            let direct = add(direct, stable_sky(s, p, m.normal, q.max_steps));
            f.diffuse[i] = add(add(direct, indirect), s.input.settings.ambient);
            f.specular[i] = stable_surface(s, q, i, &emitters);
        }
        self.frame = self.frame.wrapping_add(1);
        self.old = Some(s.clone());
        self.quality = Some(q);
        self.field = Some(f.clone());
        Ok(f)
    }
}
