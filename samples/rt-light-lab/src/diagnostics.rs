//! Exercise the real cave map, particle adapter, production budgets and scheduler.
use charon_tracing::{Mode, Platform, Quality};
use charon_tracing_gpu::{GpuTrace, Tracing};
use std::{
    error::Error,
    time::{Duration, Instant},
};
pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let mode = match option("--lighting").map(String::as_str).unwrap_or("path") {
        "path" => Mode::Path,
        "rtx" => Mode::Rtx,
        "lite_path" => Mode::LitePath,
        _ => return Err("--trace-check supports path, rtx, lite_path".into()),
    };
    if !cfg!(feature = "bridge") && args.iter().any(|a| a == "--motion") {
        return Err("--motion requires the bridge feature".into());
    }
    let frames: u32 = option("--frames")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(8);
    if frames == 0 {
        return Err("--frames must be positive".into());
    }
    let mut runtime = charon_m3_sample::runtime::Runtime::start_with_map(
        30,
        cfg!(feature = "bridge"),
        charon_m4_sample::world::map(),
    )?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        // Wait for the same worker snapshot the interactive adapter consumes.
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.shared.view.lock().unwrap().ticks < 2 {
            runtime.check()?;
            if Instant::now() > deadline {
                return Err("cave worker snapshot timed out".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let build = |frame: u32| -> Result<charon_tracing::TraceScene, String> {
            #[allow(unused_mut)]
            let mut view = (**runtime.shared.view.lock().unwrap()).clone();
            #[cfg(feature = "bridge")]
            if args.iter().any(|a| a == "--motion") {
                use charon_physics::sim::material::CellData;
                let snapshot = view.sand.as_mut().ok_or("particle snapshot missing")?;
                // Deterministic moving snapshots through the production particle
                // adapter, independent of this adapter's render speed.
                let y = 100 + (frame % 24) as usize;
                snapshot.cells[y * snapshot.width + 80] = CellData::sand(19);
                snapshot.cells[110 * snapshot.width + 120 + (frame % 8) as usize] =
                    CellData::fire(220);
            }
            #[cfg(not(feature = "bridge"))]
            let _ = frame;
            let player = *runtime.shared.player.lock().unwrap();
            let full = crate::materials::from_view(
                &view,
                player.position,
                Default::default(),
                true,
                true,
                mode,
            )?;
            if mode == Mode::LitePath {
                charon_tracing::TraceScene::at_resolution(&full.input, 8.)
            } else {
                Ok(full)
            }
        };
        let initial = build(0)?;
        let q = Quality::for_mode(mode);
        let (gpu, name) = pollster::block_on(GpuTrace::headless(
            initial.input.geometry,
            mode,
            Platform::Pc,
            q,
        ))?;
        println!(
            "DEMO CHECK: {name}; {mode:?}; {}x{}; {} emitters; {q:?}",
            initial.input.geometry.width,
            initial.input.geometry.height,
            initial.emitters().len()
        );
        let mut tracing = Tracing::new(gpu, 30)?;
        // Submitting changing snapshots must never bypass the 30 Hz compute lane.
        tracing.submit(initial.clone())?;
        tracing.submit(initial)?;
        assert_eq!(
            tracing.tracer.samples(),
            0,
            "submit dispatched outside compute schedule"
        );
        for frame in 0..frames {
            let input = build(frame)?;
            tracing.submit(input)?;
            let start = Instant::now();
            tracing.tick(Duration::from_secs_f64(1. / 30.))?;
            let field = tracing.tracer.readback()?;
            if field.rgba.iter().flatten().any(|v| !v.is_finite()) {
                return Err("non-finite cave output".into());
            }
            if matches!(mode, Mode::Path | Mode::Rtx) {
                let hdr = tracing.tracer.readback_hdr()?;
                if hdr
                    .diffuse
                    .iter()
                    .chain(&hdr.specular)
                    .chain(&hdr.emission)
                    .flatten()
                    .any(|v| !v.is_finite() || *v < 0.)
                {
                    return Err("invalid cave HDR output".into());
                }
            }
            println!(
                "frame={frame} spp={} GPU={:?} wall_ms={:.1} bytes={} faces={}",
                tracing.tracer.samples(),
                tracing.tracer.metrics().gpu_ms,
                start.elapsed().as_secs_f64() * 1000.,
                tracing.tracer.metrics().estimated_bytes,
                tracing.tracer.metrics().faces
            );
        }
        println!(
            "DEMO CHECK passed: actual cave input, scheduled dispatch, finite readback; this is not a visual-quality or hardware-RTX claim unless run on that hardware."
        );
        Ok(())
    })();
    runtime.shutdown()?;
    result
}
