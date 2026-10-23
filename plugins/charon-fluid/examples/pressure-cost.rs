//! Large-domain fixed-input cost, including the actual MAC/VOF update.
//! `--surface` selects an open surface with alternating horizontal forcing.
use charon_fluid::{gpu::GpuContext, Config, Fluid};
use std::{sync::Arc, time::Instant};

fn main() {
    let surface = std::env::args().skip(1).any(|arg| {
        assert_eq!(arg, "--surface");
        true
    });
    let gpu = Arc::new(GpuContext::new().expect("Vulkan/DX12 compute adapter required"));
    eprintln!("adapter: {:?}", gpu.adapter_info());
    let (width, height) = (130, 146);
    let solid = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            x == 0 || x == width - 1 || y == 0 || y == height - 1
        })
        .collect();
    let mut fluid = Fluid::new(
        Config {
            width,
            height,
            workers: 4,
            density: [1., 0.8],
            drag: 0.8,
            viscosity: 0.5,
            pressure_cutoff: 0.01,
            sleep_tolerance: 0.,
        },
        solid,
    )
    .unwrap();
    let top = if surface { 74 } else { 1 };
    let oil_bottom = if surface { 80 } else { 9 };
    for y in top..height - 1 {
        for x in 1..width - 1 {
            fluid.add(x, y, usize::from(y < oil_bottom), 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    fluid.use_gpu(gpu.clone());
    println!("{{\"adapter_identity\":\"{}\",\"scene\":\"{}\",\"grid\":[130,146],\"workers\":4,\"initial_volume\":[{},{}],\"ticks\":[",gpu.identity(),if surface {"surface"} else {"sealed"},initial[0],initial[1]);
    for tick in 0..3 {
        let acceleration = [if surface { [40., -40., 0.][tick] } else { 0. }, 120.];
        let start = Instant::now();
        let s = fluid.step(1. / 60., acceleration).unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        for (a, b) in s.volume.into_iter().zip(initial) {
            assert!((a - b).abs() < 1e-7);
        }
        assert!(s.max_divergence < 1e-3, "{s:?}");
        if !surface {
            assert!(s.max_speed < 1e-5, "{s:?}");
        }
        let g = s.gpu.unwrap();
        let kernel = g
            .pressure_ms
            .map_or_else(|| "null".to_owned(), |v| v.to_string());
        println!("{{\"tick\":{tick},\"tick_ms\":{elapsed},\"pressure_host_ms\":{},\"pressure_kernel_ms\":{kernel},\"pressure_assembly_ms\":{},\"pressure_setup_ms\":{},\"readbacks\":{},\"pressure_iterations\":{},\"substeps\":{},\"wet_cells\":{},\"max_divergence\":{},\"max_speed\":{},\"upload_bytes\":{},\"readback_bytes\":{},\"dispatches\":{},\"pressure_workgroups\":{}}}{}",s.pressure_ms,s.pressure_assembly_ms,g.pressure_setup_ms,g.readbacks,s.pressure_iterations,s.substeps,s.wet_cells,s.max_divergence,s.max_speed,g.upload_bytes,g.readback_bytes,g.dispatches,g.pressure_workgroups,if tick==2 {""} else {","});
    }
    println!("]}}");
}
