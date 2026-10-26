use charon_tracing::{charon_lighting::*, *};
use charon_tracing_gpu::GpuTrace;
fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut cpu = Accumulator::default();
        let mut expected = None;
        for _ in 0..4 {
            gpu.render(&input)?;
            expected = Some(cpu.update(&input, q)?);
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
    // Regression: moving lights must not randomize direct illumination, even at 1 spp.
    let direct_q = Quality {
        samples_per_update: 1,
        max_samples: 16,
        bounces: 0,
        ..q
    };
    let (mut direct, _) = pollster::block_on(GpuTrace::headless(g, mode, Platform::Pc, direct_q))?;
    let mut input = TraceScene::new(
        Scene::new(
            g,
            Settings {
                sky: [0.; 3],
                ambient: [0.; 3],
                ao_strength: 0.,
                ..Settings::default()
            },
        )?
        .prepare(),
    )?;
    for x in [-8.5, -8.4, -8.3] {
        input.point_lights = vec![PointLight {
            position: [x, 14.5],
            color: [1., 0.5, 0.1],
            intensity: 1.,
        }];
        direct.render(&input)?;
        assert_eq!(direct.samples(), 1);
        let first = direct.readback()?;
        let expected = solve_sample(&input, direct_q)?;
        for (a, b) in first
            .rgba
            .iter()
            .flatten()
            .zip(expected.rgba.iter().flatten())
        {
            assert!((a - b).abs() < 0.001);
        }
        for _ in 1..16 {
            direct.render(&input)?;
        }
        assert_eq!(first.rgba, direct.readback()?.rgba);
    }
    println!("Moving sub-cell direct light: 1/16 spp identical; CPU/GPU parity OK");
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
        ..Default::default()
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
    let runtime = slot.active.take().unwrap();
    let worker = charon_tracing_gpu::TracingWorker::spawn(runtime, 144, || Ok(None))?;
    worker.set_enabled(false);
    assert!(
        worker
            .readback()?
            .rgba
            .iter()
            .flatten()
            .all(|v| v.is_finite())
    );
    slot.commit_worker(worker);
    assert!(slot.threaded.is_some());
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
