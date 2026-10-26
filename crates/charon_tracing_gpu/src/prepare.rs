use super::*;
use std::sync::Arc;
fn range(scene: &TraceScene, range: std::ops::Range<usize>) -> Vec<Optical> {
    let g = scene.input.geometry;
    range
        .map(|i| {
            let x = (i as u32 % g.width) as i32;
            let y = (i as u32 / g.width) as i32;
            let mut opacity = 0.;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && ny >= 0 && nx < g.width as i32 && ny < g.height as i32 {
                        opacity += scene.input.optics[ny as usize * g.width as usize + nx as usize]
                            .opacity;
                    }
                }
            }
            let o = scene.input.optics[i];
            Optical {
                transport: [
                    o.transmission[0],
                    o.transmission[1],
                    o.transmission[2],
                    o.opacity,
                ],
                source: [
                    scene.input.sources[i][0],
                    scene.input.sources[i][1],
                    scene.input.sources[i][2],
                    0.,
                ],
                surface: [
                    scene.reflectance[i][0],
                    scene.reflectance[i][1],
                    scene.reflectance[i][2],
                    1. - scene.input.settings.ao_strength * opacity / 8.,
                ],
            }
        })
        .collect()
}
pub fn optics(
    scene: &TraceScene,
    pool: Option<&Arc<charon_tasks::TaskPool>>,
) -> Result<Vec<Optical>, String> {
    let n = scene.input.geometry.len();
    if let Some(pool) = pool
        && n >= 4096
    {
        let input = Arc::new(scene.clone());
        let jobs = (0..n)
            .step_by(4096)
            .map(|start| start..(start + 4096).min(n))
            .collect();
        return pool
            .map(jobs, move |r| range(&input, r))
            .map(|parts| parts.into_iter().flatten().collect());
    }
    Ok(range(scene, 0..n))
}
