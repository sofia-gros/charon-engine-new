mod gallery;
use charon_tracing::{charon_lighting::*, *};
use charon_tracing_gpu::GpuTrace;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(path) = std::env::args().skip_while(|a| a != "--gallery").nth(1) {
        return gallery::run(std::path::Path::new(&path));
    }
    let mode = if std::env::args().any(|a| a == "--rtx") {
        Mode::Rtx
    } else {
        Mode::Path
    };
    let g = Geometry {
        width: 16,
        height: 12,
        origin: [-12., 8.],
        cell_size: 1.,
    };
    let q = Quality {
        samples_per_update: 4,
        bounces: 2,
        max_steps: 96,
        max_samples: 16,
    };
    let (mut gpu, name) = pollster::block_on(GpuTrace::headless(g, mode, Platform::Pc, q))?;
    println!("Tracing adapter: {name}; {mode:?}");
    for case in 0..5 {
        gpu.reset();
        let mut scene = Scene::new(
            g,
            Settings {
                max_distance: 24.,
                sky: if case == 0 { [0.6, 0.8, 1.] } else { [0.; 3] },
                ambient: [0.; 3],
                ao_strength: 0.5,
                ..Settings::default()
            },
        )?;
        let mut grid = OpticalGrid::new(g)?;
        if case > 0 {
            for y in 0..g.height {
                if case != 2 || y != 6 {
                    grid.set(
                        8,
                        y,
                        if case == 3 {
                            Optical {
                                opacity: 0.1,
                                transmission: [0.6, 0.8, 1.],
                                ..Optical::AIR
                            }
                        } else {
                            Optical::OPAQUE
                        },
                    )?;
                }
            }
        }
        scene.set_layer(1, grid)?;
        if case > 0 {
            scene.set_light(
                1,
                PointLight {
                    position: [-8.5, 14.5],
                    color: [1., 0.5, 0.1],
                    intensity: 1.,
                },
            )?;
        }
        let input = TraceScene::new(scene.prepare())?;
        let mut cpu = charon_tracing::hdr::Accumulator::default();
        let mut expected = None;
        let mut expected_hdr = None;
        let mut previous_specular: Option<Vec<[f32; 3]>> = None;
        for _ in 0..4 {
            gpu.render(&input)?;
            let current = gpu.readback_hdr()?;
            if let Some(previous) = &previous_specular {
                let difference = current
                    .specular
                    .iter()
                    .flatten()
                    .zip(previous.iter().flatten())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0_f32, f32::max);
                if difference > 1e-5 {
                    return Err(
                        format!("static specular flicker in case {case}: {difference}").into(),
                    );
                }
            }
            previous_specular = Some(current.specular);
            let field = cpu.update(&input, q)?;
            expected = Some(field.light_field());
            expected_hdr = Some(field);
        }
        let actual_hdr = gpu.readback_hdr()?;
        let expected_hdr = expected_hdr.unwrap();
        let mut hdr_max = 0_f32;
        for (a, b) in actual_hdr
            .diffuse
            .iter()
            .chain(&actual_hdr.specular)
            .chain(&actual_hdr.emission)
            .zip(
                expected_hdr
                    .diffuse
                    .iter()
                    .chain(&expected_hdr.specular)
                    .chain(&expected_hdr.emission),
            )
        {
            for k in 0..3 {
                hdr_max = hdr_max.max((a[k] - b[k]).abs());
            }
        }
        if hdr_max > 0.002 {
            return Err(format!("unclipped HDR CPU/GPU mismatch in case {case}: {hdr_max}").into());
        }
        let actual = gpu.readback()?;
        let expected = expected.unwrap();
        let differences: Vec<_> = actual
            .rgba
            .iter()
            .flatten()
            .zip(expected.rgba.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .collect();
        let max = differences.iter().copied().fold(0_f32, f32::max);
        let rmse =
            (differences.iter().map(|x| x * x).sum::<f32>() / differences.len() as f32).sqrt();
        println!(
            "case {case}: samples={}, max error {max}, RMSE {rmse}",
            gpu.samples()
        );
        if max > 0.08 || rmse > 0.01 {
            for (i, (a, b)) in actual.rgba.iter().zip(&expected.rgba).enumerate() {
                if (a[0] - b[0]).abs() > 0.03 {
                    eprintln!("cell {i} ({},{}): GPU {a:?} CPU {b:?}", i % 16, i / 16);
                }
            }
            return Err(format!("CPU/GPU path mismatch in case {case}").into());
        }
        let mut dark = input.clone();
        dark.input.settings.sky = [0.; 3];
        dark.input.sources.fill([0.; 3]);
        gpu.render(&dark)?;
        if gpu.samples() != q.samples_per_update {
            return Err("same-revision input did not reset history".into());
        }
        if gpu.readback()?.rgba.iter().any(|p| p[..3] != [0.; 3]) {
            return Err("old light remained after removing all sources".into());
        }
    }
    let mut emissive = TraceScene::new(
        Scene::new(
            g,
            Settings {
                sky: [0.; 3],
                ambient: [0.; 3],
                ao_strength: 0.,
                max_distance: 24.,
                ..Settings::default()
            },
        )?
        .prepare(),
    )?;
    emissive.materials[20] = Material {
        emission: [12., 3., 0.2],
        dynamic: true,
        ..Material::AIR
    };
    // Material emission alone must illuminate neighbors and reflect.
    gpu.render(&emissive)?;
    let hdr = gpu.readback_hdr()?;
    assert_eq!(hdr.emission[20], [12., 3., 0.2]);
    emissive.materials[20] = Material::AIR;
    emissive.input.sources.fill([0.; 3]);
    gpu.render(&emissive)?;
    let dark = gpu.readback_hdr()?;
    assert!(
        dark.diffuse
            .iter()
            .chain(&dark.specular)
            .chain(&dark.emission)
            .flatten()
            .all(|v| *v == 0.),
        "old HDR light or reflections remained"
    );
    // Run Lite with the actual Mobile policy on the same compute adapter.
    let lite = Quality::for_mode(Mode::LitePath);
    let (mut mobile, _) = pollster::block_on(GpuTrace::headless(
        g,
        Mode::LitePath,
        Platform::Mobile,
        lite,
    ))?;
    let scene = TraceScene::new(
        Scene::new(
            g,
            Settings {
                max_distance: 20.,
                ..Settings::default()
            },
        )?
        .prepare(),
    )?;
    mobile.render(&scene)?;
    assert_eq!(mobile.samples(), 1);
    assert!(
        mobile
            .readback()?
            .rgba
            .iter()
            .flatten()
            .all(|v| v.is_finite())
    );
    // The public switching API must reject unsupported requests without replacing
    // a functioning backend or discarding its accumulated samples.
    use charon_tracing_gpu::{SwitchRequest, Tracing, TracingSlot};
    let device = mobile.device();
    let mut slot = TracingSlot {
        active: Some(Tracing::new(mobile, 30)?),
    };
    let old_samples = slot.active.as_ref().unwrap().tracer.samples();
    let request = SwitchRequest {
        mode: Mode::Path,
        platform: Platform::Mobile,
        quality: q,
        hz: 30,
    };
    assert!(!pollster::block_on(slot.try_switch(
        &device,
        request,
        scene.clone()
    ))?);
    assert_eq!(slot.active.as_ref().unwrap().tracer.samples(), old_samples);
    let request = SwitchRequest {
        mode: Mode::Rtx,
        platform: Platform::Mobile,
        quality: q,
        hz: 30,
    };
    assert!(!pollster::block_on(slot.try_switch(
        &device,
        request,
        scene.clone()
    ))?);
    assert_eq!(slot.active.as_ref().unwrap().tracer.samples(), old_samples);
    assert!(!device.info().rtx);
    let request = SwitchRequest {
        mode: Mode::Rtx,
        platform: Platform::Pc,
        quality: q,
        hz: 30,
    };
    assert!(!pollster::block_on(slot.try_switch(
        &device,
        request,
        scene.clone()
    ))?);
    assert_eq!(slot.active.as_ref().unwrap().tracer.samples(), old_samples);
    let request = SwitchRequest {
        mode: Mode::Path,
        platform: Platform::Pc,
        quality: q,
        hz: 30,
    };
    assert!(pollster::block_on(slot.try_switch(
        &device,
        request,
        scene.clone()
    ))?);
    assert_eq!(slot.active.as_ref().unwrap().tracer.mode(), Mode::Path);
    let request = SwitchRequest {
        mode: Mode::Legacy,
        platform: Platform::Pc,
        quality: q,
        hz: 30,
    };
    assert!(pollster::block_on(
        slot.try_switch(&device, request, scene)
    )?);
    assert!(slot.active.is_none());
    println!("Lite/Mobile policy, finite outputs, accumulation and invalidation OK");
    Ok(())
}
