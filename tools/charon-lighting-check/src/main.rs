use charon_lighting::*;
use charon_lighting_gpu::GpuLighting;
use serde_json::json;
use std::{path::Path, time::Instant};
fn fixtures(g: Geometry) -> Result<Vec<(&'static str, Prepared)>, String> {
    let settings = Settings {
        ambient: [0.; 3],
        max_distance: 64.,
        ao_strength: 0.6,
        ..Settings::default()
    };
    let mut scene = Scene::new(g, settings)?;
    let mut cases = Vec::new();
    let mut roof = OpticalGrid::new(g)?;
    roof.raster_rect([-16., 24., 96., 4.], Optical::OPAQUE)?;
    cases.push(("surface", scene.prepare()));
    scene.set_layer(1, roof.clone())?;
    cases.push(("sealed_cave", scene.prepare()));
    let mut open = OpticalGrid::new(g)?;
    open.raster_rect([-16., 24., 36., 4.], Optical::OPAQUE)?;
    open.raster_rect([24., 24., 56., 4.], Optical::OPAQUE)?;
    scene.set_layer(1, open)?;
    cases.push(("narrow_opening", scene.prepare()));
    scene.set_layer(1, roof)?;
    cases.push(("closed_opening", scene.prepare()));
    let mut dark = scene.settings();
    dark.sky = [0.; 3];
    scene.set_settings(dark)?;
    scene.remove_layer(1);
    let mut water = OpticalGrid::new(g)?;
    water.raster_rect(
        [16., 0., 4., 64.],
        Optical {
            opacity: 0.1,
            transmission: [0.4, 0.7, 1.],
            ..Optical::AIR
        },
    )?;
    scene.set_layer(1, water)?;
    scene.set_light(
        1,
        PointLight {
            position: [8., 40.],
            color: [1.; 3],
            intensity: 1.,
        },
    )?;
    cases.push(("colored_water", scene.prepare()));
    let mut contact = OpticalGrid::new(g)?;
    contact.raster_rect([36., 32., 4., 16.], Optical::OPAQUE)?;
    contact.raster_rect([36., 44., 16., 4.], Optical::OPAQUE)?;
    scene.set_layer(1, contact)?;
    scene.set_light(
        1,
        PointLight {
            position: [44., 36.],
            color: [1., 0.7, 0.3],
            intensity: 1.,
        },
    )?;
    cases.push(("contacts_ao", scene.prepare()));
    scene.remove_light(1);
    let mut ore = OpticalGrid::new(g)?;
    ore.raster_rect(
        [60., 36., 4., 4.],
        Optical {
            emission_color: [0.2, 0.8, 1.],
            emission_strength: 1.,
            ..Optical::OPAQUE
        },
    )?;
    scene.set_layer(1, ore)?;
    cases.push(("emitting_ore", scene.prepare()));
    let mut off = OpticalGrid::new(g)?;
    off.raster_rect([60., 36., 4., 4.], Optical::OPAQUE)?;
    scene.set_layer(1, off)?;
    cases.push(("ore_removed", scene.prepare()));
    scene.remove_layer(1);
    scene.set_light(
        1,
        PointLight {
            position: [8., 40.],
            color: [1.; 3],
            intensity: 1.,
        },
    )?;
    cases.push(("point_light", scene.prepare()));
    scene.set_light(
        1,
        PointLight {
            position: [72., 40.],
            color: [1.; 3],
            intensity: 1.,
        },
    )?;
    cases.push(("moved_light", scene.prepare()));
    scene.set_light(
        1,
        PointLight {
            position: [-8., 40.],
            color: [1.; 3],
            intensity: 1.,
        },
    )?;
    cases.push(("offscreen_light", scene.prepare()));
    scene.remove_light(1);
    cases.push(("removed_light", scene.prepare()));
    let mut zero = scene.prepare();
    zero.settings.max_distance = 0.;
    cases.push(("zero_steps", zero));
    scene.set_light(
        1,
        PointLight {
            position: [8., 40.],
            color: [1.; 3],
            intensity: 1.,
        },
    )?;
    let mut odd = scene.prepare();
    odd.settings.max_distance = g.cell_size;
    cases.push(("one_step", odd));
    Ok(cases)
}
fn save(path: &Path, field: &LightField) -> Result<(), String> {
    let mut image = image::RgbImage::new(field.geometry.width, field.geometry.height);
    for (i, pixel) in image.pixels_mut().enumerate() {
        let color = field.rgba[i];
        *pixel = image::Rgb(
            [color[0], color[1], color[2]]
                .map(|v| (v.clamp(0., 1.).powf(1. / 2.2) * 255.).round() as u8),
        );
    }
    image.save(path).map_err(|e| e.to_string())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let report = args.get(1).map(String::as_str);
    let image_dir = args.get(2).map(Path::new);
    if let Some(dir) = image_dir {
        std::fs::create_dir_all(dir)?;
    }
    let mut records = Vec::new();
    let mut startup = Vec::new();
    let mut adapter = String::new();
    for cell_size in [1., 2., 4.] {
        let g = Geometry {
            width: (96. / cell_size) as u32,
            height: (64. / cell_size) as u32,
            origin: [-16., 0.],
            cell_size,
        };
        let (mut gpu, name) = pollster::block_on(GpuLighting::headless(g))?;
        adapter = name;
        if gpu.readback().is_ok() {
            return Err("uncomputed GPU field was readable".into());
        }
        let cases = fixtures(g)?;
        for warmup in 0..2 {
            gpu.solve(&cases[0].1)?;
            gpu.readback()?;
            startup
                .push(json!({"cell_size":cell_size,"warmup":warmup,"gpu_us":gpu.metrics().gpu_us}));
        }
        for (name, input) in cases {
            let started = Instant::now();
            let cpu = solve_cpu(&input)?;
            let cpu_us = started.elapsed().as_secs_f64() * 1e6;
            gpu.solve(&input)?;
            let started = Instant::now();
            let result = gpu.readback()?;
            let readback_us = started.elapsed().as_secs_f64() * 1e6;
            let max_error = cpu
                .rgba
                .iter()
                .flatten()
                .zip(result.rgba.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0_f32, f32::max);
            if max_error > 0.0002 {
                return Err(
                    format!("CPU/GPU mismatch {name} resolution {cell_size}: {max_error}").into(),
                );
            }
            if matches!(name, "ore_removed" | "removed_light" | "zero_steps")
                && result.rgba.iter().any(|v| v[..3] != [0.; 3])
            {
                return Err(format!("stale light in {name}").into());
            }
            if name == "offscreen_light" && result.sample([0., 40.]).unwrap()[0] <= 0. {
                return Err("off-screen light did not illuminate viewport".into());
            }
            if matches!(name, "sealed_cave" | "closed_opening")
                && result.sample([22., 40.]).unwrap()[0] != 0.
            {
                return Err("sealed cave leaked light".into());
            }
            let m = gpu.metrics();
            records.push(json!({"case":name,"cell_size":cell_size,"width":g.width,"height":g.height,"max_error":max_error,"cpu_us":cpu_us,"gpu_us":m.gpu_us,"encoding_us":m.encoding_us,"readback_completion_us":readback_us,"upload_bytes":m.upload_bytes,"resident_bytes":m.resident_bytes,"dispatches":m.dispatches}));
            if cell_size == 2.
                && let Some(dir) = image_dir
            {
                save(&dir.join(format!("{name}-cpu.png")), &cpu)?;
                save(&dir.join(format!("{name}-gpu.png")), &result)?;
                let a = image::open(dir.join(format!("{name}-cpu.png")))?.into_rgb8();
                let b = image::open(dir.join(format!("{name}-gpu.png")))?.into_rgb8();
                let delta = a
                    .as_raw()
                    .iter()
                    .zip(b.as_raw())
                    .map(|(x, y)| x.abs_diff(*y))
                    .max()
                    .unwrap_or(0);
                if delta > 1 {
                    return Err(format!("PNG mismatch {name}: {delta}").into());
                }
            }
        }
    }
    let output = json!({"schema":1,"adapter":adapter,"warmup_passes_per_resolution":2,"startup":startup,"cases":records,"tolerance":0.0002,"note":"Single diagnostic samples, not a throughput benchmark. GPU timestamps include compute passes and barriers; readback completion includes waiting and transfer. Images encode linear light with display gamma."});
    if let Some(path) = report {
        std::fs::write(path, serde_json::to_string_pretty(&output)? + "\n")?;
    }
    println!(
        "M4: {} CPU/GPU cases, 3 resolutions, stale-light removal and off-screen sources OK / {}",
        output["cases"].as_array().unwrap().len(),
        adapter
    );
    Ok(())
}
