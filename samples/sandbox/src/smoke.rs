//! GPU correctness cases, including snapshots deliberately skipped by the renderer.
use charon_render::{Gpu, Result};
use charon_runtime::Simulation;
use charon_sand::{Command, SandWorld, WALL};

pub fn verify_updates(gpu: &mut Gpu, width: usize, height: usize) -> Result<()> {
    let mut world = SandWorld::new(width, height, 2)?;
    let initial = world.step(vec![]);
    gpu.smoke_tiled(&initial.image)?;
    let idle = world.step(vec![]);
    assert_eq!(idle.converted_cells, 0);
    assert_eq!(gpu.smoke_tiled(&idle.image)?, 0, "idle image was uploaded");
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
    let bytes = gpu.smoke_tiled(&edited.image)?;
    assert!(bytes > 0 && bytes <= edited.image.byte_len());
    if edited.image.tile_count() > 2 {
        assert!(bytes < edited.image.byte_len());
    }
    let cleared = world.step(vec![Command::Clear]);
    gpu.smoke_tiled(&cleared.image)?;
    assert_eq!(initial.image.to_rgba(), cleared.image.to_rgba());
    let reset_idle = world.step(vec![]);
    assert_eq!(
        gpu.smoke_tiled(&reset_idle.image)?,
        0,
        "idle reset image was uploaded"
    );
    gpu.smoke_overlay(&reset_idle.image)?;
    gpu.smoke_sprites(&reset_idle.image)?;
    let atlas = std::sync::Arc::new(charon_assets::Texture::from_rgba(
        2,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    )?);
    let mut looks = vec![
        charon_render::TileAppearance {
            color: [16, 23, 36, 255],
            source: None
        };
        256
    ];
    looks[WALL as usize].source = Some([0, 0, 2, 2]);
    let uploaded = gpu.smoke_indexed(&reset_idle.indices, &looks, &atlas)?;
    assert_eq!(uploaded, [width * height, 8192, 16]);
    assert_eq!(
        gpu.smoke_indexed(&reset_idle.indices, &looks, &atlas)?,
        [0; 3]
    );
    // Animate the surface of sleeping terrain without editing or uploading cells.
    looks[WALL as usize].source = Some([1, 0, 1, 2]);
    assert_eq!(
        gpu.smoke_indexed(&reset_idle.indices, &looks, &atlas)?,
        [0, 8192, 0]
    );
    world.step(vec![Command::Paint {
        x: 4,
        y: 4,
        material: WALL,
    }]);
    let skipped = world.step(vec![]);
    assert!(gpu.smoke_indexed(&skipped.indices, &looks, &atlas)?[0] > 0);
    // Pan, zoom and out-of-world sampling must agree on RGBA and indexed paths.
    for origin in [[1., 2.], [-4., -3.]] {
        gpu.set_camera(charon_render::Camera2D::new(
            origin,
            [width as f32 / 2., height as f32 / 2.],
        )?);
        gpu.smoke_tiled(&skipped.image)?;
        gpu.smoke_indexed(&skipped.indices, &looks, &atlas)?;
        if origin[0] > 0. {
            gpu.smoke_sprites(&skipped.image)?;
        }
    }
    gpu.set_camera(charon_render::Camera2D::new(
        [0., 0.],
        [width as f32, height as f32],
    )?);
    println!("partial_upload_checks=idle,skipped_snapshots,edge_tiles,clear");
    Ok(())
}
