//! Deterministic offscreen captures using the production HDR composite and post
//! shaders. Requires a compute adapter, never a window or a hardware RTX claim.
use charon_tracing::{charon_lighting::*, *};
use charon_tracing_gpu::{GpuTrace, wgpu};
use std::{error::Error, path::Path};
use wgpu::util::DeviceExt;
const W: u32 = 96;
const H: u32 = 64;
fn scene(frame: u32) -> Result<TraceScene, String> {
    let g = Geometry {
        width: W,
        height: H,
        origin: [0.; 2],
        cell_size: 1.,
    };
    let settings = Settings {
        sky: [0.14, 0.18, 0.25],
        ambient: [0.002; 3],
        max_distance: 100.,
        falloff_per_unit: 0.995,
        ao_strength: 0.,
    };
    let mut s = TraceScene::new(Scene::new(g, settings)?.prepare())?;
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) as usize;
            if x < 3
                || x >= W - 3
                || y >= H - 3
                || (y < 4 && (x < 30 || x > 55))
                || (x == 64 && y > 14 && y < 43)
            {
                s.input.optics[i] = Optical::OPAQUE;
                s.materials[i] = Material {
                    diffuse: [0.45, 0.29, 0.21],
                    ..Material::STONE
                };
            }
            if y >= 49 && y < H - 3 && x >= 42 && x < 86 {
                s.materials[i] = Material {
                    normal: if y == 49 {
                        [0., -0.98, 0.19899748]
                    } else {
                        [0., 0., 1.]
                    },
                    specular: if y == 49 { 0.02 } else { 0. },
                    ..Material::WATER
                };
            }
            let height = (12 - (x as i32 - 24).abs()).max(0) as u32;
            if y >= H - 3 - height && y < H - 3 && height > 0 {
                s.input.optics[i] = Optical::OPAQUE;
                let grain = ((x * 197 + y * 131) % 256) as f32 / 255. - 0.5;
                s.materials[i] = Material {
                    normal: hdr::normalize([grain * 0.3, grain * 0.17, 1.]),
                    ..Material::SAND
                };
            }
            let fx = 48 + ((frame / 4) % 5);
            let fy = 32;
            if x >= fx && x < fx + 4 && y >= fy - 5 && y < fy + 5 && ((x + y + frame) % 5 != 0) {
                let heat = (1. - (y as f32 - (fy + 3) as f32).abs() / 10.).clamp(0., 1.);
                let color = [1., 0.08 + 0.72 * heat * heat, 0.015 + 0.32 * heat * heat];
                let strength = 4. + 10. * heat;
                s.input.optics[i] = Optical {
                    emission_color: color,
                    emission_strength: strength,
                    ..Optical::AIR
                };
                s.input.sources[i] = color.map(|v| v * strength);
                s.materials[i] = Material {
                    emission: s.input.sources[i],
                    dynamic: true,
                    ..Material::AIR
                };
            }
            // Falling grey particles are opaque, with actual cell-sized air gaps.
            if (x == 35 || x == 38 || x == 41) && (y + frame) % 11 == 0 && y > 12 && y < 45 {
                s.input.optics[i] = Optical::OPAQUE;
                s.materials[i] = Material {
                    diffuse: [0.16; 3],
                    dynamic: true,
                    ..Material::STONE
                };
            }
        }
    }
    Ok(s)
}
fn linear_to_srgb(v: f32) -> u8 {
    ((if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    })
    .clamp(0., 1.)
        * 255.)
        .round() as u8
}
fn albedo(s: &TraceScene) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((W * H * 4) as usize);
    for (i, m) in s.materials.iter().enumerate() {
        let c = if s.input.optics[i].opacity >= 0.999 || m.dielectric {
            m.diffuse
        } else {
            [0.07, 0.09, 0.12]
        };
        for v in c {
            pixels.push(linear_to_srgb(v));
        }
        pixels.push(255);
    }
    pixels
}
fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &wgpu::Texture,
    albedo: &[u8],
    shader: &str,
    hdr: bool,
    size: [u32; 2],
) -> Result<Vec<u8>, String> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gallery display output"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gallery albedo"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        albedo,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(W * 4),
            rows_per_image: Some(H),
        },
        color.size(),
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production lighting composite"),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gallery world composite"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: if hdr {
                    wgpu::TextureFormat::Rgba16Float
                } else {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                },
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let sampler = device.create_sampler(&Default::default());
    let light_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let color_view = color.create_view(&Default::default());
    let light_view = lighting.create_view(&Default::default());
    let texture_group = |binding, view: &wgpu::TextureView, sampler: &wgpu::Sampler| {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(view),
        }];
        if binding != 2 || !hdr {
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gallery textures"),
            layout: &pipeline.get_bind_group_layout(binding),
            entries: &entries,
        })
    };
    let color_group = texture_group(0, &color_view, &sampler);
    let light_group = texture_group(2, &light_view, &light_sampler);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("gallery screen domain"),
        contents: bytemuck::cast_slice(&[0., 0., size[0] as f32, size[1] as f32]),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let domain = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let mut v = Vec::<f32>::new();
    for [x, y, u, t] in [
        [-1., 1., 0., 0.],
        [1., 1., 1., 0.],
        [-1., -1., 0., 1.],
        [-1., -1., 0., 1.],
        [1., 1., 1., 0.],
        [1., -1., 1., 1.],
    ] {
        v.extend([x, y, u, t, 1., 1., 1., 1.]);
    }
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&v),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let post = hdr.then(|| {
        charon_render::gpu::HdrPost::new(device, size, output.format(), Default::default())
    });
    let output_view = output.create_view(&Default::default());
    let target = post.as_ref().map_or(&output_view, |h| &h.scene);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gallery HDR world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &color_group, &[]);
        pass.set_bind_group(1, &domain, &[]);
        pass.set_bind_group(2, &light_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..6, 0..1);
    }
    if let Some(post) = post {
        post.encode(&mut encoder, &output_view);
    }
    let stride = (size[0] * 4).div_ceil(256) * 256;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: stride as u64 * size[1] as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size[1]),
            },
        },
        output.size(),
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait)
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = staging.slice(..).get_mapped_range();
    let pixels = data
        .chunks(stride as usize)
        .flat_map(|row| row[..size[0] as usize * 4].iter().copied())
        .collect();
    drop(data);
    staging.unmap();
    if let Some(e) = pollster::block_on(device.pop_error_scope()) {
        return Err(e.to_string());
    }
    Ok(pixels)
}
pub fn run(path: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(path)?;
    let s = scene(0)?;
    let q = Quality {
        samples_per_update: 8,
        bounces: 3,
        max_steps: 256,
        max_samples: 64,
    };
    let mode = if std::env::args().any(|a| a == "--rtx") {
        Mode::Rtx
    } else {
        Mode::Path
    };
    let q = if mode == Mode::Rtx {
        Quality::for_mode(mode)
    } else {
        q
    };
    let (mut gpu, name) =
        pollster::block_on(GpuTrace::headless(s.input.geometry, mode, Platform::Pc, q))?;
    println!("Gallery adapter: {name}; {mode:?}");
    let device = gpu.device();
    let raw = device.raw_device();
    let queue = device.raw_queue();
    let size = if std::env::args().any(|a| a == "--1080p") {
        [1920, 1080]
    } else {
        [960, 640]
    };
    let static_only = std::env::args().any(|a| a == "--static-only");
    let noise_check = std::env::args().any(|a| a == "--noise-check");
    for stage in 0..(if noise_check { 0 } else { 3 }) {
        gpu.reset();
        let mut input = s.clone();
        input.surface_quality = SurfaceQuality::for_mode(mode);
        if stage == 0 {
            input.materials.iter_mut().for_each(|m| {
                m.specular = 0.;
                m.dielectric = false;
                m.emission = [0.; 3];
            });
        }
        if stage == 1 {
            input.materials.iter_mut().for_each(|m| {
                m.specular = 0.;
                m.dielectric = false;
            });
        }
        for _ in 0..q.max_samples.div_ceil(q.samples_per_update) {
            gpu.render(&input)?;
        }
        let texture = gpu.texture();
        let pixels = capture(
            raw,
            queue,
            texture,
            &albedo(&s),
            charon_lighting_render::HDR_SHADER,
            true,
            size,
        )?;
        image::save_buffer(
            path.join(format!("stage-{stage}.png")),
            &pixels,
            size[0],
            size[1],
            image::ColorType::Rgba8,
        )?;
    }
    if static_only {
        println!("Static gallery: {:?}", gpu.metrics());
        return Ok(());
    }
    if noise_check {
        gpu.reset();
        let input = scene(0)?;
        let mut previous: Option<Vec<[f32; 3]>> = None;
        for frame in 0..2 {
            gpu.render(&input)?;
            let field = gpu.readback_hdr()?;
            if let Some(previous) = &previous {
                let difference = field
                    .specular
                    .iter()
                    .flatten()
                    .zip(previous.iter().flatten())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0_f32, f32::max);
                println!("Static reflection/transmission temporal max difference: {difference}");
                if difference > 1e-5 {
                    return Err("static specular Monte Carlo flicker returned".into());
                }
            }
            previous = Some(field.specular);
            let pixels = capture(
                raw,
                queue,
                gpu.texture(),
                &albedo(&input),
                charon_lighting_render::HDR_SHADER,
                true,
                size,
            )?;
            image::save_buffer(
                path.join(format!("low-samples-{frame}.png")),
                &pixels,
                size[0],
                size[1],
                image::ColorType::Rgba8,
            )?;
        }
        return Ok(());
    }
    gpu.reset();
    let mut maximum_specular = 0_f32;
    for frame in 0..24 {
        let mut input = scene(frame)?;
        input.surface_quality = SurfaceQuality::for_mode(mode);
        gpu.render(&input)?;
        let hdr = gpu.readback_hdr()?;
        maximum_specular =
            maximum_specular.max(hdr.specular.iter().flatten().copied().fold(0., f32::max));
        let pixels = capture(
            raw,
            queue,
            gpu.texture(),
            &albedo(&input),
            charon_lighting_render::HDR_SHADER,
            true,
            size,
        )?;
        image::save_buffer(
            path.join(format!("motion-{frame:03}.png")),
            &pixels,
            size[0],
            size[1],
            image::ColorType::Rgba8,
        )?;
    }
    println!(
        "Gallery specular max {maximum_specular}; {:?}",
        gpu.metrics()
    );
    if maximum_specular <= 0. {
        return Err("reflection contribution was zero".into());
    }
    Ok(())
}
