//! GPU correctness cases, including snapshots deliberately skipped by the renderer.
use charon_render::{Gpu, Result};
use charon_runtime::Simulation;
use charon_sand::{Command, SandWorld, WALL};
use std::path::Path;

pub fn verify_updates(gpu: &mut Gpu, width: usize, height: usize) -> Result<()> {
    let mut world = SandWorld::new(width, height, 2)?;
    let output = Path::new("artifacts/local/partial-smoke.png");
    let initial = world.step(vec![]);
    gpu.smoke_tiled(&initial.image, output)?;
    let idle = world.step(vec![]);
    assert_eq!(idle.converted_cells, 0);
    assert_eq!(
        gpu.smoke_tiled(&idle.image, output)?,
        0,
        "idle image was uploaded"
    );
    // First edit becomes inactive before the renderer receives another frame.
    // The latest frame must still contain and upload that earlier change.
    world.step(vec![Command::Paint {
        x: 4,
        y: 4,
        material: WALL,
    }]);
    world.step(vec![]);
    let edited = world.step(vec![Command::Paint {
        x: width - 2,
        y: height - 2,
        material: WALL,
    }]);
    let bytes = gpu.smoke_tiled(&edited.image, output)?;
    assert!(bytes > 0 && bytes <= edited.image.byte_len());
    if edited.image.tile_count() > 2 {
        assert!(bytes < edited.image.byte_len());
    }
    let cleared = world.step(vec![Command::Clear]);
    gpu.smoke_tiled(&cleared.image, output)?;
    assert_eq!(initial.image.to_rgba(), cleared.image.to_rgba());
    let reset_idle = world.step(vec![]);
    assert_eq!(
        gpu.smoke_tiled(&reset_idle.image, output)?,
        0,
        "idle reset image was uploaded"
    );
    gpu.smoke_overlay(&reset_idle.image, output)?;
    println!("partial_upload_checks=idle,skipped_snapshots,edge_tiles,clear");
    Ok(())
}
