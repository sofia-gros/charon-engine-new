//! Fixed physical input, independent of rendering speed. Run on the same adapter
//! before/after a solver change; software-adapter results are not hardware FPS.
use charon_fluid::{gpu::GpuContext, Config, Fluid};
use std::{sync::Arc, time::Instant};

fn main() {
    let gpu = Arc::new(GpuContext::new().expect("Vulkan/DX12 compute adapter required"));
    eprintln!("adapter: {:?}", gpu.adapter_info());
    let (w, h) = (128, 100);
    let mut solid = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            solid[y * w + x] = x == 0
                || y == 0
                || x == w - 1
                || y == h - 1
                || ((48..80).contains(&x)
                    && (50..75).contains(&y)
                    && (!(50..78).contains(&x) || y >= 73));
        }
    }
    let mut fluid = Fluid::new(
        Config {
            width: w,
            height: h,
            workers: 4,
            density: [1., 0.8],
            drag: 0.8,
            viscosity: 0.5,
            pressure_cutoff: 0.01,
            sleep_tolerance: 0.005,
        },
        solid,
    )
    .unwrap();
    for y in 60..73 {
        for x in 50..78 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 53..56 {
        for x in 50..78 {
            fluid.add(x, y, 1, 1.).unwrap();
        }
    }
    fluid.use_gpu(gpu.clone());
    let mut times = Vec::new();
    let (mut pressure, mut velocity, mut transport, mut kernel, mut iterations, mut substeps) =
        (0., 0., 0., gpu.timing_available().then_some(0_f64), 0, 0);
    let (mut upload, mut readback, mut divergence) = (0_u64, 0_u64, 0_f64);
    let mut previous = 0.;
    for tick in 0..240 {
        let speed = match tick {
            30..=59 | 150..=179 => 15.,
            90..=119 | 210..=239 => -15.,
            _ => 0.,
        };
        let start = Instant::now();
        let s = fluid
            .step(1. / 60., [-(speed - previous) * 60., 180.])
            .unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.);
        previous = speed;
        pressure += s.pressure_ms;
        velocity += s.velocity_ms;
        transport += s.transport_ms;
        iterations += s.pressure_iterations;
        substeps += s.substeps;
        let g = s.gpu.unwrap();
        kernel = kernel.zip(g.pressure_ms).map(|(sum, value)| sum + value);
        upload += g.upload_bytes;
        readback += g.readback_bytes;
        divergence = divergence.max(s.max_divergence);
        let expected = [364., 84.];
        for (a, b) in s.volume.into_iter().zip(expected) {
            assert!((a - b).abs() < 1e-7);
        }
        assert!(s.max_divergence < 1e-3, "tick={tick} {s:?}");
    }
    let total: f64 = times.iter().sum();
    times.sort_by(f64::total_cmp);
    println!("{{\"adapter_identity\":\"{}\",\"ticks\":240,\"grid\":[128,100],\"workers\":4,\"water\":364,\"oil\":84,\"tick_mean_ms\":{},\"tick_p95_ms\":{},\"tick_max_ms\":{},\"pressure_host_total_ms\":{},\"pressure_kernel_total_ms\":{},\"velocity_host_total_ms\":{},\"transport_total_ms\":{},\"pressure_iterations\":{},\"substeps\":{},\"upload_bytes\":{},\"readback_bytes\":{},\"max_divergence\":{}}}",
        gpu.identity(), total/240., times[227], times[239], pressure, kernel.map_or_else(|| String::from("null"), |v| v.to_string()), velocity, transport, iterations, substeps, upload, readback, divergence);
}
