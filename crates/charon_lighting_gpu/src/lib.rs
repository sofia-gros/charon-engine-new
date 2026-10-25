//! GPU computation only: no window, renderer, tile, or particle dependency.
use charon_lighting::{Geometry, LightField, Prepared};
use std::{sync::mpsc, time::Instant};
use wgpu::util::DeviceExt;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OpticalGpu {
    transport: [f32; 4],
    emission: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    extent: [u32; 4],
    quality: [f32; 4],
    sky: [f32; 4],
    ambient: [f32; 4],
}
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub encoding_us: f64,
    pub gpu_us: Option<f64>,
    pub upload_bytes: u64,
    pub resident_bytes: u64,
    pub dispatches: u32,
}
pub struct GpuLighting {
    device: wgpu::Device,
    queue: wgpu::Queue,
    geometry: Geometry,
    optics: wgpu::Buffer,
    sources: wgpu::Buffer,
    params: wgpu::Buffer,
    _base: wgpu::Buffer,
    _ping: wgpu::Buffer,
    _pong: wgpu::Buffer,
    result: wgpu::Buffer,
    output: wgpu::Texture,
    groups: [wgpu::BindGroup; 2],
    seed: wgpu::ComputePipeline,
    propagate: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    query: Option<wgpu::QuerySet>,
    query_buffer: Option<wgpu::Buffer>,
    last_revision: Option<u64>,
    metrics: Metrics,
}
impl GpuLighting {
    pub async fn headless(geometry: Geometry) -> Result<(Self, String), String> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .map_err(|e| e.to_string())?;
        let name = adapter.get_info().name;
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok((Self::new(device, queue, geometry).await?, name))
    }
    pub async fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        geometry: Geometry,
    ) -> Result<Self, String> {
        geometry.validate()?;
        let n = geometry.len() as u64;
        let limits = device.limits();
        if geometry.width > limits.max_texture_dimension_2d
            || geometry.height > limits.max_texture_dimension_2d
            || n * 32 > u64::from(limits.max_storage_buffer_binding_size)
        {
            return Err("lighting domain exceeds GPU limits".into());
        }
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let optics = buffer("optical inputs", n * 32);
        let sources = buffer("point and material sources", n * 16);
        let base = buffer("fresh sky seeds", n * 16);
        let ping = buffer("light ping", n * 16);
        let pong = buffer("light pong", n * 16);
        let result = buffer("light result and AO", n * 16);
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lighting settings"),
            contents: &[0; 64],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("linear lightmap"),
            size: wgpu::Extent3d {
                width: geometry.width,
                height: geometry.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let mut entries: Vec<_> = (0..6)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: matches!(binding, 0 | 1 | 3),
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lighting compute layout"),
            entries: &entries,
        });
        let make_group = |previous: &wgpu::Buffer, next: &wgpu::Buffer| {
            let buffers = [&optics, &sources, &base, previous, next, &result];
            let mut entries: Vec<_> = buffers
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect();
            entries.push(wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 7,
                resource: params.as_entire_binding(),
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lighting ping pong"),
                layout: &layout,
                entries: &entries,
            })
        };
        let groups = [make_group(&ping, &pong), make_group(&pong, &ping)];
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lighting computation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lighting.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let seed = pipeline("seed");
        let propagate = pipeline("propagate");
        let finish = pipeline("finish");
        if let Some(error) = device.pop_error_scope().await {
            return Err(error.to_string());
        }
        let query = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("lighting timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                })
            });
        let query_buffer = query.as_ref().map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lighting timing result"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timing_bytes = if query_buffer.is_some() { 16 } else { 0 };
        Ok(Self {
            device,
            queue,
            geometry,
            optics,
            sources,
            params,
            _base: base,
            _ping: ping,
            _pong: pong,
            result,
            output,
            groups,
            seed,
            propagate,
            finish,
            query,
            query_buffer,
            last_revision: None,
            metrics: Metrics {
                resident_bytes: n * 116 + 64 + timing_bytes,
                ..Default::default()
            },
        })
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.output
    }
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }
    /// Submits compute work only. No readback or CPU wait in the live render path.
    pub fn solve(&mut self, input: &Prepared) -> Result<(), String> {
        input.validate()?;
        if input.geometry != self.geometry {
            return Err("GPU lighting geometry changed; recreate the backend".into());
        }
        let started = Instant::now();
        let optics: Vec<_> = input
            .optics
            .iter()
            .map(|o| {
                let e = o.emission();
                OpticalGpu {
                    transport: [
                        o.opacity,
                        o.transmission[0],
                        o.transmission[1],
                        o.transmission[2],
                    ],
                    emission: [e[0], e[1], e[2], 0.],
                }
            })
            .collect();
        let sources: Vec<_> = input
            .sources
            .iter()
            .map(|s| [s[0], s[1], s[2], 0.])
            .collect();
        let g = self.geometry;
        let steps = input.settings.steps(g)?;
        let params = Params {
            extent: [g.width, g.height, steps, 0],
            quality: [input.settings.decay(g), input.settings.ao_strength, 0., 0.],
            sky: [
                input.settings.sky[0],
                input.settings.sky[1],
                input.settings.sky[2],
                0.,
            ],
            ambient: [
                input.settings.ambient[0],
                input.settings.ambient[1],
                input.settings.ambient[2],
                0.,
            ],
        };
        self.queue
            .write_buffer(&self.optics, 0, bytemuck::cast_slice(&optics));
        self.queue
            .write_buffer(&self.sources, 0, bytemuck::cast_slice(&sources));
        self.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lighting compute, independent of drawing"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sky and fresh seeds"),
                timestamp_writes: self
                    .query
                    .as_ref()
                    .map(|q| wgpu::ComputePassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: None,
                    }),
            });
            pass.set_pipeline(&self.seed);
            pass.set_bind_group(0, &self.groups[1], &[]);
            pass.dispatch_workgroups(g.width.div_ceil(64), 1, 1);
        }
        for step in 0..steps {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("light propagation"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.propagate);
            pass.set_bind_group(0, &self.groups[(step % 2) as usize], &[]);
            pass.dispatch_workgroups((g.len() as u32).div_ceil(64), 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("AO and light result"),
                timestamp_writes: self
                    .query
                    .as_ref()
                    .map(|q| wgpu::ComputePassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(1),
                    }),
            });
            pass.set_pipeline(&self.finish);
            pass.set_bind_group(0, &self.groups[(steps % 2) as usize], &[]);
            pass.dispatch_workgroups((g.len() as u32).div_ceil(64), 1, 1);
        }
        if let (Some(query), Some(buffer)) = (&self.query, &self.query_buffer) {
            encoder.resolve_query_set(query, 0..2, buffer, 0);
        }
        self.queue.submit([encoder.finish()]);
        self.last_revision = Some(input.revision);
        self.metrics.encoding_us = started.elapsed().as_secs_f64() * 1e6;
        self.metrics.upload_bytes = g.len() as u64 * 48 + 64;
        self.metrics.dispatches = steps + 2;
        self.metrics.gpu_us = None;
        Ok(())
    }
    /// Diagnostic path: waits for completion and reads full-precision values. Live
    /// composition uses texture() directly without this transfer.
    pub fn readback(&mut self) -> Result<LightField, String> {
        let revision = self.last_revision.ok_or("lighting has not been computed")?;
        let bytes = self.geometry.len() as u64 * 16;
        let extra = if self.query_buffer.is_some() { 16 } else { 0 };
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("diagnostic lighting readback"),
            size: bytes + extra,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.result, 0, &staging, 0, bytes);
        if let Some(buffer) = &self.query_buffer {
            encoder.copy_buffer_to_buffer(buffer, 0, &staging, bytes, 16);
        }
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let view = staging.slice(..).get_mapped_range();
        let rgba = bytemuck::cast_slice::<u8, [f32; 4]>(&view[..bytes as usize]).to_vec();
        if self.query_buffer.is_some() {
            let stamps = bytemuck::cast_slice::<u8, u64>(&view[bytes as usize..]);
            self.metrics.gpu_us = Some(
                stamps[1].saturating_sub(stamps[0]) as f64
                    * self.queue.get_timestamp_period() as f64
                    / 1000.,
            );
        }
        drop(view);
        staging.unmap();
        Ok(LightField {
            geometry: self.geometry,
            rgba,
            revision,
        })
    }
}
