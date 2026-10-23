use super::*;
fn domain(w: usize, h: usize, workers: usize) -> Fluid {
    let mut solid = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            solid[y * w + x] = x == 0 || y == 0 || x == w - 1 || y == h - 1;
        }
    }
    Fluid::new(
        Config {
            width: w,
            height: h,
            workers,
            density: [1., 0.8],
            drag: 0.8,
            viscosity: 0.5,
            pressure_cutoff: 0.01,
            sleep_tolerance: 1e-3,
        },
        solid,
    )
    .unwrap()
}
fn conserved(fluid: &Fluid, initial: [f64; 2]) {
    for (a, b) in fluid.volumes().into_iter().zip(initial) {
        assert!((a - b).abs() < 1e-7, "volume drift {a} vs {b}");
    }
    assert!(fluid
        .state
        .fraction
        .iter()
        .all(|v| v[0] >= -1e-10 && v[1] >= -1e-10 && v[0] + v[1] <= 1. + 1e-8));
}
#[test]
fn hydrostatic_oil_layer_stays_sharp_and_sleeps_then_wakes() {
    let mut fluid = domain(34, 34, 4);
    for y in 17..33 {
        for x in 1..33 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 13..17 {
        for x in 1..33 {
            fluid.add(x, y, 1, 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    for _ in 0..100 {
        let s = fluid.step(1. / 60., [0., 120.]).unwrap();
        assert!(s.gpu.is_none(), "CPU steps must not claim GPU measurements");
        assert!(s.max_divergence < 1e-4, "{s:?}");
        conserved(&fluid, initial);
    }
    assert!(fluid.stats().sleeping, "{:?}", fluid.stats());
    assert!(fluid
        .state
        .fraction
        .iter()
        .all(|v| v[0] < 1e-7 || v[1] < 1e-7));
    let stats = fluid.step(1. / 60., [-900., 120.]).unwrap();
    assert!(!stats.sleeping && stats.max_speed > 1.);
}
#[test]
fn subcell_film_is_supported_by_the_floor_without_a_gravity_current() {
    let mut fluid = domain(26, 30, 2);
    for x in 1..25 {
        fluid.add(x, 28, 0, 0.2).unwrap();
    }
    let initial = fluid.volumes();
    for _ in 0..100 {
        let s = fluid.step(1. / 60., [0., 120.]).unwrap();
        assert!(s.max_speed < 0.01, "{s:?}");
        conserved(&fluid, initial);
    }
    assert!(fluid.stats().sleeping);
}
#[test]
fn sealed_full_tank_supports_pressure_without_an_air_reference() {
    let mut fluid = domain(18, 18, 2);
    for y in 1..17 {
        for x in 1..17 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    for _ in 0..80 {
        let s = fluid.step(1. / 60., [40., 120.]).unwrap();
        conserved(&fluid, initial);
        assert!(s.max_divergence < 1e-4);
    }
    assert!(fluid.stats().sleeping);
}
#[test]
fn viscosity_dissipates_shear_energy_without_losing_full_tank_volume() {
    let mut a = domain(18, 18, 2);
    a.config.viscosity = 0.;
    a.config.drag = 0.;
    a.config.sleep_tolerance = 0.;
    for y in 1..17 {
        for x in 1..17 {
            a.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 1..17 {
        for x in 2..17 {
            a.state.u[y * 19 + x] = if y % 2 == 0 { 1. } else { -1. };
        }
    }
    let mut b = Fluid::new(
        Config {
            viscosity: 5.,
            ..a.config
        },
        a.solid.clone(),
    )
    .unwrap();
    b.state = a.state.clone();
    let initial = a.volumes();
    let free = a.step(1. / 60., [0., 0.]).unwrap();
    let viscous = b.step(1. / 60., [0., 0.]).unwrap();
    assert!(
        viscous.rms_speed < 0.9 * free.rms_speed,
        "{viscous:?} vs {free:?}"
    );
    conserved(&a, initial);
    conserved(&b, initial);
}
#[test]
fn dam_break_turns_a_corner_conserves_species_and_is_worker_independent() {
    let mut a = domain(42, 34, 4);
    // A submerged baffle makes the path non-rectangular.
    for y in 24..33 {
        a.set_solid(23, y, true).unwrap();
    }
    for y in 3..33 {
        for x in 1..12 {
            a.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 1..3 {
        for x in 1..12 {
            a.add(x, y, 1, 1.).unwrap();
        }
    }
    let mut b = Fluid::new(
        Config {
            workers: 1,
            ..a.config
        },
        a.solid.clone(),
    )
    .unwrap();
    b.state = a.state.clone();
    let initial = a.volumes();
    let mut transported = 0.;
    for tick in 0..180 {
        let s = a
            .step(1. / 60., [0., 120.])
            .unwrap_or_else(|e| panic!("tick={tick} {e:?}"));
        b.step(1. / 60., [0., 120.]).unwrap();
        assert_eq!(a.signature(), b.signature(), "worker order tick={tick}");
        assert!(s.max_divergence < 1e-3, "{s:?}");
        conserved(&a, initial);
        transported = (1..33)
            .flat_map(|y| (24..41).map(move |x| (x, y)))
            .map(|(x, y)| a.fraction(x, y).unwrap().iter().sum::<f64>())
            .sum();
    }
    assert!(
        transported > 5.,
        "water did not travel over the baffle: {transported}"
    );
}
#[test]
fn separated_droplet_falls_and_invalid_edits_do_not_destroy_liquid() {
    let mut fluid = domain(26, 40, 2);
    for y in 4..9 {
        for x in 10..15 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    let before = fluid.signature();
    assert_eq!(fluid.set_solid(12, 5, true), Err(Error::Occupied));
    assert_eq!(fluid.add(12, 5, 1, 1.), Err(Error::Occupied));
    assert!(matches!(
        fluid.step(f64::NAN, [0., 120.]),
        Err(Error::InvalidInput)
    ));
    assert_eq!(fluid.signature(), before);
    for _ in 0..30 {
        fluid.step(1. / 60., [0., 120.]).unwrap();
        conserved(&fluid, initial);
    }
    let center = fluid
        .state
        .fraction
        .iter()
        .enumerate()
        .map(|(i, v)| (i / 26) as f64 * (v[0] + v[1]))
        .sum::<f64>()
        / initial[0];
    let t = 0.5;
    let drag = fluid.config.drag;
    let g = 120.;
    let expected = 6. + g / drag * (t - (1. - (-drag * t).exp()) / drag);
    assert!(
        (center - expected).abs() < 1.5,
        "falling liquid centroid {center}, analytic gravity/drag {expected}"
    );
    let before = fluid.signature();
    assert!(fluid.step(0.1, [1e9, 1e9]).is_err());
    assert_eq!(fluid.signature(), before, "failed tick did not roll back");
}

/// Explicit compute contract test; run with a Vulkan/DX12 adapter, including
/// software adapters in Cloud. Never silently skips missing devices.
#[cfg(feature = "gpu")]
#[test]
#[ignore = "requires compute adapter; run --features gpu -- --ignored"]
fn gpu_compute_matches_reference_and_preserves_two_phase_volume() {
    let gpu = std::sync::Arc::new(gpu::GpuContext::new().unwrap());
    eprintln!("compute adapter: {:?}", gpu.adapter_info());
    for n in [37, 1024, 4096] {
        let rows: Vec<_> = (0..n)
            .map(|i| {
                (
                    [
                        if i > 0 { i - 1 } else { usize::MAX },
                        if i + 1 < n { i + 1 } else { usize::MAX },
                        usize::MAX,
                        usize::MAX,
                    ],
                    [1., 1., 0., 0.],
                    2.1,
                )
            })
            .collect();
        let expected: Vec<_> = (0..n).map(|i| ((i * 17 % 53) as f64) / 53.).collect();
        let rhs: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(i, (adj, a, d))| {
                d * expected[i]
                    - (0..4)
                        .filter(|&k| adj[k] != usize::MAX)
                        .map(|k| a[k] * expected[adj[k]])
                        .sum::<f64>()
            })
            .collect();
        let mut p = vec![0.; n];
        let (iterations, work) = gpu.solve(&rows, &rhs, &mut p).unwrap();
        assert_eq!(work.kernel_ms.is_some(), gpu.timing_available());
        assert!(work.dispatches > 0 && work.upload_bytes > 0 && work.readback_bytes > 0);
        assert_eq!(
            work.upload_bytes,
            (n * 48) as u64 * u64::from(work.dispatches)
        );
        let readback = if gpu.timing_available() {
            (n as u64 * 20).next_multiple_of(8) + 16
        } else {
            n as u64 * 20
        };
        assert_eq!(work.readback_bytes, readback * u64::from(work.dispatches));
        let error = p
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0., f64::max);
        assert!(error < 1e-6, "n={n} error={error}");
        eprintln!("manufactured n={n} iterations={iterations} error={error:e}");
    }
    // Compare every advected face, not only the final visual surface.
    let mut cpu = domain(26, 30, 2);
    for y in 15..29 {
        for x in 1..25 {
            cpu.add(x, y, 0, 1.).unwrap();
        }
    }
    for (i, u) in cpu.state.u.iter_mut().enumerate() {
        *u = (i % 17) as f64 * 0.1;
    }
    for (i, v) in cpu.state.v.iter_mut().enumerate() {
        *v = -((i % 13) as f64) * 0.07;
    }
    let velocity = gpu
        .advect(
            cpu.config,
            &cpu.state.u,
            &cpu.state.v,
            cpu.bounds(),
            1. / 60.,
        )
        .unwrap();
    cpu.advect_velocity(1. / 60.);
    let error = velocity
        .u
        .iter()
        .chain(&velocity.v)
        .zip(cpu.state.u.iter().chain(&cpu.state.v))
        .map(|(a, b)| (a - b).abs())
        .fold(0., f64::max);
    assert!(error < 5e-6, "velocity error={error}");
    eprintln!("all MAC faces max error={error:e}");
    let untimed_gpu = std::sync::Arc::new(gpu::GpuContext::with_timing(false).unwrap());
    assert!(!untimed_gpu.timing_available());
    // Closed pressure component, free surface with oil, and falling droplet.
    for scene in 0..3 {
        let mut cpu = domain(26, 30, 2);
        for y in 1..29 {
            for x in 1..25 {
                let (fill, phase) = match scene {
                    0 => (true, 0),
                    1 => (y >= 12, usize::from(y < 16)),
                    _ => (y < 7 && (9..14).contains(&x), 0),
                };
                if fill {
                    cpu.add(x, y, phase, 1.).unwrap();
                }
            }
        }
        let mut accelerated = Fluid::new(cpu.config, cpu.solid.clone()).unwrap();
        accelerated.state = cpu.state.clone();
        accelerated.use_gpu(gpu.clone());
        let mut untimed = Fluid::new(cpu.config, cpu.solid.clone()).unwrap();
        untimed.state = cpu.state.clone();
        untimed.use_gpu(untimed_gpu.clone());
        let initial = cpu.volumes();
        let mut total_kernel_ms = 0.;
        for tick in 0..90 {
            let acceleration = if scene == 1 && (30..45).contains(&tick) {
                [-180., 120.]
            } else {
                [0., 120.]
            };
            cpu.step(1. / 60., acceleration).unwrap();
            let s = accelerated
                .step(1. / 60., acceleration)
                .unwrap_or_else(|e| panic!("scene={scene} tick={tick} {e:?}"));
            conserved(&accelerated, initial);
            let work = s.gpu.expect("GPU stage metrics");
            assert_eq!(work.velocity_ms.is_some(), gpu.timing_available());
            assert_eq!(work.pressure_ms.is_some(), gpu.timing_available());
            for value in [work.velocity_ms, work.pressure_ms].into_iter().flatten() {
                assert!(value.is_finite() && value >= 0.);
                total_kernel_ms += value;
            }
            if s.sleeping && s.substeps == 0 {
                assert_eq!(
                    (work.upload_bytes, work.readback_bytes, work.dispatches),
                    (0, 0, 0)
                );
                assert!(
                    work.velocity_ms.is_none_or(|v| v == 0.)
                        && work.pressure_ms.is_none_or(|v| v == 0.)
                );
            } else {
                assert!(
                    work.dispatches >= s.substeps as u32
                        && work.upload_bytes > 0
                        && work.readback_bytes > 0
                );
            }
            if tick < 8 {
                let other = untimed.step(1. / 60., acceleration).unwrap();
                assert_eq!(
                    accelerated.signature(),
                    untimed.signature(),
                    "profiling changed physics scene={scene} tick={tick}"
                );
                let work = other.gpu.unwrap();
                assert!(work.velocity_ms.is_none() && work.pressure_ms.is_none());
                assert!(work.dispatches > 0);
            }
            assert!(s.max_divergence < 1e-3, "scene={scene} tick={tick} {s:?}");
            let phase_difference = cpu
                .state
                .fraction
                .iter()
                .zip(&accelerated.state.fraction)
                .map(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs())
                .sum::<f64>();
            assert!(
                phase_difference < initial.iter().sum::<f64>() * 0.03,
                "scene={scene} tick={tick} difference={phase_difference}"
            );
        }
        if gpu.timing_available() {
            assert!(total_kernel_ms > 0.);
        }
        if scene == 0 {
            assert!(accelerated.stats().sleeping);
        }
        eprintln!(
            "scene={scene} steps=90 gpu_kernel_ms={total_kernel_ms:.3} conserved={:?}",
            accelerated.volumes()
        );
    }
}
