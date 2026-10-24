use charon_render::{
    Camera2D, Gpu, IndexedImage, LightMaterial, Lighting, PointLight, TileAppearance,
};
use std::sync::Arc;
fn config() -> Lighting {
    let mut materials = vec![LightMaterial::default(); 256];
    materials[1].opacity = 1.;
    Lighting {
        materials,
        lights: vec![],
        ambient: 0.8,
        interior_darkness: 0.9,
        interior_radius: 16,
    }
}
fn image(width: usize, height: usize, cells: &[u8]) -> IndexedImage {
    let mut image = IndexedImage::new(width, height, 32);
    for t in 0..image.tile_count() {
        let r = image.rect(t);
        let mut bytes = vec![];
        for y in r.y..r.y + r.height {
            bytes.extend_from_slice(&cells[y * width + r.x..y * width + r.x + r.width]);
        }
        image.replace(t, bytes);
    }
    image
}
#[test]
#[ignore = "requires Vulkan/software GPU"]
fn real_shader_darkens_interiors_blocks_thin_walls_and_relights_holes_without_idle_work() {
    let instance = charon_render::instance();
    let adapter = pollster::block_on(charon_render::adapter(&instance, None, true)).unwrap();
    let mut gpu =
        pollster::block_on(Gpu::new(&adapter, wgpu::TextureFormat::Rgba8Unorm, 96, 64)).unwrap();
    let atlas = Arc::new(charon_assets::Texture::from_rgba(1, 1, vec![255; 4]).unwrap());
    let looks = vec![
        TileAppearance {
            color: [180, 180, 180, 255],
            source: None
        };
        256
    ];
    let mut cells = vec![0; 96 * 64];
    for y in 16..48 {
        for x in 8..40 {
            cells[y * 96 + x] = 1;
        }
    }
    let mut lighting = config();
    gpu.set_lighting(Some(lighting.clone())).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    let pixels = gpu.readback(96, 64).unwrap();
    let red = |p: &[u8], x: usize, y: usize| p[(y * 96 + x) * 4];
    assert!(
        red(&pixels, 24, 32) + 60 < red(&pixels, 8, 32),
        "solid center did not darken"
    );
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    // Above made equivalent but independent tiles: upload them, then verify reuse.
    gpu.readback(96, 64).unwrap();
    gpu.readback(96, 64).unwrap();
    assert_eq!(gpu.lighting_stats().cells_updated, 0);
    // A source left of a one-pixel wall illuminates its front, not the far side.
    cells.fill(0);
    for y in 0..64 {
        cells[y * 96 + 48] = 1;
    }
    lighting.ambient = 0.1;
    lighting.interior_darkness = 0.;
    lighting.lights = vec![PointLight {
        position: [32.5, 32.5],
        color: [2., 0., 0.],
        radius: 64.,
    }];
    gpu.set_lighting(Some(lighting.clone())).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    let blocked = gpu.readback(96, 64).unwrap();
    assert!(red(&blocked, 48, 32) > 120, "wall front remained unlit");
    assert_eq!(
        red(&blocked, 64, 32),
        18,
        "light leaked through one-pixel wall"
    );
    lighting.materials[1].opacity = 0.5;
    gpu.set_lighting(Some(lighting.clone())).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    let transmitted = gpu.readback(96, 64).unwrap();
    assert!(
        (62..=64).contains(&red(&transmitted, 64, 32)),
        "partial transmission did not attenuate light"
    );
    assert_eq!(
        transmitted[(32 * 96 + 64) * 4 + 1],
        18,
        "red light changed the green channel"
    );
    lighting.materials[1].opacity = 1.;
    gpu.set_lighting(Some(lighting.clone())).unwrap();
    cells[32 * 96 + 48] = 0;
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    let open = gpu.readback(96, 64).unwrap();
    assert!(
        red(&open, 64, 32) > red(&blocked, 64, 32) + 60,
        "one-pixel hole did not transmit light"
    );
    assert_eq!(
        red(&open, 64, 40),
        18,
        "light leaked away from the narrow opening"
    );
    // Move a source while terrain is unchanged. Its previous lighting must clear.
    lighting.lights[0].position = [80.5, 32.5];
    gpu.set_lighting(Some(lighting.clone())).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    let moved = gpu.readback(96, 64).unwrap();
    assert_eq!(red(&moved, 16, 40), 18);
    // Disable returns the exact unlit pixels. Re-enable must rebuild cached light.
    gpu.set_lighting(None).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    assert_eq!(red(&gpu.readback(96, 64).unwrap(), 24, 32), 180);
    gpu.set_lighting(Some(lighting)).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    assert!(red(&gpu.readback(96, 64).unwrap(), 24, 32) < 180);
    // A diagonal ray touching the corner of a solid cell cannot cut through it.
    cells.fill(0);
    cells[16 * 96 + 17] = 1;
    let mut corner = config();
    corner.ambient = 0.1;
    corner.lights.push(PointLight {
        position: [16.5, 16.5],
        color: [2., 0., 0.],
        radius: 64.,
    });
    gpu.set_lighting(Some(corner)).unwrap();
    gpu.upload_indexed(&image(96, 64, &cells), &looks, &atlas)
        .unwrap();
    assert_eq!(
        red(&gpu.readback(96, 64).unwrap(), 18, 18),
        18,
        "diagonal ray leaked across a solid corner"
    );
    gpu.set_camera(Camera2D::new([48., 0.], [48., 64.]).unwrap());
    gpu.readback(48, 64).unwrap();
    println!(
        "lighting_contracts=interior,thin_wall,hole,source_move,disable,reenable,idle stats={:?}",
        gpu.lighting_stats()
    );
}

#[test]
#[ignore = "requires Vulkan/software GPU"]
fn offscreen_occluder_edits_invalidate_visible_light_but_distant_edits_and_idle_do_not() {
    let instance = charon_render::instance();
    let adapter = pollster::block_on(charon_render::adapter(&instance, None, true)).unwrap();
    let mut gpu = pollster::block_on(Gpu::new(
        &adapter,
        wgpu::TextureFormat::Rgba8Unorm,
        512,
        128,
    ))
    .unwrap();
    gpu.set_camera(Camera2D::new([0., 0.], [64., 64.]).unwrap());
    let atlas = Arc::new(charon_assets::Texture::from_rgba(1, 1, vec![255; 4]).unwrap());
    let looks = vec![
        TileAppearance {
            color: [180, 180, 180, 255],
            source: None
        };
        256
    ];
    let mut cells = vec![0; 512 * 128];
    for y in 0..128 {
        cells[y * 512 + 80] = 1;
    }
    let mut terrain = image(512, 128, &cells);
    let mut lighting = config();
    lighting.ambient = 0.1;
    lighting.lights = vec![PointLight {
        position: [96.5, 32.5],
        color: [2., 0., 0.],
        radius: 96.,
    }];
    gpu.set_lighting(Some(lighting)).unwrap();
    gpu.upload_indexed(&terrain, &looks, &atlas).unwrap();
    let blocked = gpu.readback(64, 64).unwrap();
    assert_eq!(blocked[(32 * 64 + 48) * 4], 18);
    assert_eq!(gpu.lighting_stats().cells_updated, 4096);
    let edit = |terrain: &mut IndexedImage, x: usize, y: usize| {
        for t in 0..terrain.tile_count() {
            let r = terrain.rect(t);
            if x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height {
                let mut data = terrain.pixels(t).to_vec();
                data[(y - r.y) * r.width + x - r.x] ^= 1;
                terrain.replace(t, data);
                break;
            }
        }
    };
    edit(&mut terrain, 400, 32);
    gpu.upload_indexed(&terrain, &looks, &atlas).unwrap();
    gpu.readback(64, 64).unwrap();
    assert_eq!(
        gpu.lighting_stats().cells_updated,
        0,
        "distant edit rebuilt the visible field"
    );
    edit(&mut terrain, 80, 32);
    gpu.upload_indexed(&terrain, &looks, &atlas).unwrap();
    let opened = gpu.readback(64, 64).unwrap();
    assert!(opened[(32 * 64 + 48) * 4] > blocked[(32 * 64 + 48) * 4] + 60);
    assert!(
        gpu.lighting_stats().cells_updated > 0,
        "offscreen occluder edit did not invalidate dependent light"
    );
    gpu.readback(64, 64).unwrap();
    assert_eq!(gpu.lighting_stats().dispatches, 0);
    gpu.set_camera(Camera2D::new([448., 0.], [64., 64.]).unwrap());
    let panned = gpu.readback(64, 64).unwrap();
    assert_eq!(gpu.lighting_stats().cells_updated, 4096);
    assert_eq!(
        panned[(32 * 64 + 48) * 4],
        18,
        "newly visible tile used uninitialized light"
    );
}
