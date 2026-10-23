//! Distributed PCG with GPU reductions and ping-pong directions. Batches avoid
//! one CPU round trip per Krylov iteration. Small domains keep the cooperative
//! kernel; only converged pressures cross back, not the Krylov work arrays.
use super::{Error, GpuContext, Work};
const INITIALIZE: usize = 0;
const START: usize = 1;
const PRODUCT: usize = 2;
const ALPHA: usize = 3;
const UPDATE: usize = 4;
const BETA: usize = 5;
const COLLECT: usize = 6;
const BATCH: u32 = 16;
pub(super) struct TiledPressure {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 7],
}
impl TiledPressure {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let entries: Vec<_> = (0..4)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: i == 0 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tiled pressure"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tiled pressure"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tiled pressure"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../pressure-tiled.wgsl").into()),
        });
        let pipelines = [
            "initialize",
            "start",
            "product",
            "alpha",
            "update",
            "beta",
            "collect",
        ]
        .map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        Self { layout, pipelines }
    }
    fn bind(&self, device: &wgpu::Device, buffers: &[(&wgpu::Buffer, u64); 4]) -> wgpu::BindGroup {
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, (buffer, size))| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(*size),
                }),
            })
            .collect();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tiled pressure"),
            layout: &self.layout,
            entries: &entries,
        })
    }
}
impl GpuContext {
    pub(super) fn solve_tiled(
        &self,
        data: &[u8],
        n: usize,
    ) -> Result<(Vec<f64>, usize, Work), Error> {
        let groups = (n as u32).div_ceil(128);
        let mut cache = self.scratch.lock().map_err(|_| Error::Backend)?;
        let rows = self.buffer(
            &mut cache,
            3,
            "pressure rows",
            data.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let work = self.buffer(
            &mut cache,
            4,
            "distributed Krylov work",
            (n * 24) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        // Reductions use a short binding; after CG, the same allocation packs
        // four pressures per entry. Stale capacity never changes arrayLength.
        let partial_bytes = u64::from(groups) * 16;
        let packed_bytes = n.next_multiple_of(4) as u64 * 4;
        let partials = self.buffer(
            &mut cache,
            6,
            "pressure reductions/packed output",
            partial_bytes.max(packed_bytes),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let state = self.buffer(
            &mut cache,
            7,
            "pressure control",
            32,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let readback = self.buffer(
            &mut cache,
            5,
            "fluid readback",
            self.readback_size((n as u64 * 4).max(32)),
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        self.queue.write_buffer(&rows, 0, data);
        self.queue.write_buffer(&state, 0, &[0; 32]);
        let bindings = [
            (&rows, data.len() as u64),
            (&work, (n * 24) as u64),
            (&partials, partial_bytes),
            (&state, 32),
        ];
        let bind = self.tiled.bind(&self.device, &bindings);
        let mut total = self.empty_work();
        total.upload_bytes = data.len() as u64 + 32;
        let mut iterations = 0;
        for batch in 0..800 / BATCH {
            let (result, work) = self.dispatch_read(
                &state,
                32,
                &readback,
                (4 * BATCH + if batch == 0 { 2 } else { 0 }, groups),
                |pass| {
                    pass.set_bind_group(0, &bind, &[]);
                    if batch == 0 {
                        pass.set_pipeline(&self.tiled.pipelines[INITIALIZE]);
                        pass.dispatch_workgroups(groups, 1, 1);
                        pass.set_pipeline(&self.tiled.pipelines[START]);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                    for _ in 0..BATCH {
                        for (pipeline, count) in
                            [(PRODUCT, groups), (ALPHA, 1), (UPDATE, groups), (BETA, 1)]
                        {
                            pass.set_pipeline(&self.tiled.pipelines[pipeline]);
                            pass.dispatch_workgroups(count, 1, 1);
                        }
                    }
                },
            )?;
            total.add(work);
            iterations = u32::from_le_bytes(result[16..20].try_into().unwrap()) as usize;
            let status = u32::from_le_bytes(result[20..24].try_into().unwrap());
            if status == 2 {
                return Err(Error::Pressure);
            }
            if status == 1 || iterations >= 800 {
                break;
            }
        }
        let packed = self.tiled.bind(
            &self.device,
            &[
                bindings[0],
                bindings[1],
                (&partials, packed_bytes),
                bindings[3],
            ],
        );
        let (result, work) = self.dispatch_read(
            &partials,
            n as u64 * 4,
            &readback,
            (1, (n as u32).div_ceil(512)),
            |pass| {
                pass.set_bind_group(0, &packed, &[]);
                pass.set_pipeline(&self.tiled.pipelines[COLLECT]);
                pass.dispatch_workgroups((n as u32).div_ceil(512), 1, 1);
            },
        )?;
        total.add(work);
        let values: Vec<_> = result
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f64::from(f32::from_le_bytes(*bytes)))
            .collect();
        if values.iter().any(|v| !v.is_finite()) {
            return Err(Error::Pressure);
        }
        Ok((values, iterations, total))
    }
}
