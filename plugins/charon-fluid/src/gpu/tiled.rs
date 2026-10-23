//! Distributed PCG with GPU reductions and ping-pong directions. Batches avoid
//! one CPU round trip per Krylov iteration. Small domains keep the cooperative
//! kernel; only converged pressures cross back, not the Krylov work arrays.
use super::multigrid::{Hierarchy, COARSE_SWEEPS};
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
    multigrid: [wgpu::ComputePipeline; 7],
}
impl TiledPressure {
    pub(super) fn new(device: &wgpu::Device, lanes: f64) -> Self {
        let entries: Vec<_> = (0..9)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if i == 7 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: matches!(i, 0 | 4 | 5 | 8),
                        }
                    },
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
        let pipeline = |shader: &wgpu::ShaderModule, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[("LANES", lanes)],
                    ..Default::default()
                },
                cache: None,
            })
        };
        let pipelines = [
            "initialize",
            "start",
            "product",
            "alpha",
            "update",
            "beta",
            "collect",
        ]
        .map(|entry| pipeline(&shader, entry));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aggregation multigrid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../multigrid.wgsl").into()),
        });
        let multigrid = [
            "rhs",
            "smooth_first",
            "smooth_second",
            "restrict_residual",
            "coarse_solve",
            "prolong_smooth",
            "finish",
        ]
        .map(|entry| pipeline(&shader, entry));
        Self {
            layout,
            pipelines,
            multigrid,
        }
    }
    fn bind(
        &self,
        device: &wgpu::Device,
        buffers: &[(&wgpu::Buffer, u64); 9],
        level: usize,
    ) -> wgpu::BindGroup {
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, (buffer, size))| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: if i == 7 { level as u64 * 256 } else { 0 },
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
    fn cycle(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        binds: &[wgpu::BindGroup],
        hierarchy: &Hierarchy,
    ) {
        let dispatch = |pass: &mut wgpu::ComputePass<'_>, stage: usize, level: usize| {
            pass.set_bind_group(0, &binds[level], &[]);
            pass.set_pipeline(&self.multigrid[stage]);
            pass.dispatch_workgroups((hierarchy.counts[level] as u32).div_ceil(128), 1, 1);
        };
        dispatch(pass, 0, 0);
        let last = hierarchy.counts.len() - 1;
        for level in 0..last {
            // RHS/restriction already computed the zero-start first sweep.
            dispatch(pass, 2, level);
            dispatch(pass, 3, level + 1);
        }
        if hierarchy.direct {
            dispatch(pass, 4, last);
        } else {
            // First sweep came with RHS/restriction. A one-level hierarchy
            // defers its final sweep to the PCG dot-product pass.
            dispatch(pass, 2, last);
            for sweep in (2..COARSE_SWEEPS).step_by(2) {
                dispatch(pass, 1, last);
                if last > 0 || sweep + 2 < COARSE_SWEEPS {
                    dispatch(pass, 2, last);
                }
            }
        }
        for level in (0..last).rev() {
            dispatch(pass, 5, level);
            if level > 0 {
                dispatch(pass, 2, level);
            }
        }
        dispatch(pass, 6, 0);
    }
}
impl GpuContext {
    pub(super) fn solve_tiled(
        &self,
        data: &[u8],
        right: &[u8],
        hierarchy: &Hierarchy,
        first: bool,
        cache: &mut super::Scratch,
    ) -> Result<(Vec<f64>, usize, Work), Error> {
        let n = right.len() / 4;
        let groups = (n as u32).div_ceil(128);
        let rows = self.buffer(
            cache,
            3,
            "pressure rows",
            data.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let work = self.buffer(
            cache,
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
            cache,
            6,
            "pressure reductions/packed output",
            partial_bytes.max(packed_bytes),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let state = self.buffer(
            cache,
            7,
            "pressure control",
            32,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let readback = self.buffer(
            cache,
            5,
            "fluid readback",
            self.readback_size((n as u64 * 4).max(32)),
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let nodes = self.buffer(
            cache,
            9,
            "multigrid nodes",
            hierarchy.nodes.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let coefficients = self.buffer(
            cache,
            10,
            "multigrid coefficients",
            hierarchy.data.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let value_bytes = hierarchy.counts.iter().sum::<usize>() as u64 * 16;
        let values = self.buffer(
            cache,
            11,
            "multigrid vectors",
            value_bytes,
            wgpu::BufferUsages::STORAGE,
        );
        let params = self.buffer(
            cache,
            12,
            "multigrid levels",
            hierarchy.params.len() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        if first {
            self.queue.write_buffer(&nodes, 0, &hierarchy.nodes);
            self.queue.write_buffer(&coefficients, 0, &hierarchy.data);
            self.queue.write_buffer(&params, 0, &hierarchy.params);
        }
        if first {
            self.queue.write_buffer(&rows, 0, data);
        }
        let right_buffer = self.buffer(
            cache,
            13,
            "pressure right hand side",
            right.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        self.queue.write_buffer(&right_buffer, 0, right);
        self.queue.write_buffer(&state, 0, &[0; 32]);
        let bindings = [
            (&rows, data.len() as u64),
            (&work, (n * 24) as u64),
            (&partials, partial_bytes),
            (&state, 32),
            (&nodes, hierarchy.nodes.len() as u64),
            (&coefficients, hierarchy.data.len() as u64),
            (&values, value_bytes),
            (&params, 32),
            (&right_buffer, right.len() as u64),
        ];
        let binds: Vec<_> = (0..hierarchy.counts.len())
            .map(|level| self.tiled.bind(&self.device, &bindings, level))
            .collect();
        let mut total = self.empty_work();
        total.upload_bytes = right.len() as u64 + 32;
        if first {
            total.upload_bytes += data.len() as u64;
        }
        if first {
            total.upload_bytes +=
                (hierarchy.nodes.len() + hierarchy.data.len() + hierarchy.params.len()) as u64;
        }
        let mut iterations = 0;
        for batch in 0..800 / BATCH {
            let (result, work) = self.dispatch_read(
                &state,
                32,
                &readback,
                (
                    (4 + hierarchy.dispatches()) * BATCH
                        + if batch == 0 {
                            2 + hierarchy.dispatches()
                        } else {
                            0
                        },
                    groups,
                ),
                |pass| {
                    pass.set_bind_group(0, &binds[0], &[]);
                    if batch == 0 {
                        pass.set_pipeline(&self.tiled.pipelines[INITIALIZE]);
                        pass.dispatch_workgroups(groups, 1, 1);
                        self.tiled.cycle(pass, &binds, hierarchy);
                        pass.set_bind_group(0, &binds[0], &[]);
                        pass.set_pipeline(&self.tiled.pipelines[START]);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                    for _ in 0..BATCH {
                        for (pipeline, count) in [(PRODUCT, groups), (ALPHA, 1), (UPDATE, groups)] {
                            pass.set_pipeline(&self.tiled.pipelines[pipeline]);
                            pass.dispatch_workgroups(count, 1, 1);
                        }
                        self.tiled.cycle(pass, &binds, hierarchy);
                        pass.set_bind_group(0, &binds[0], &[]);
                        pass.set_pipeline(&self.tiled.pipelines[BETA]);
                        pass.dispatch_workgroups(1, 1, 1);
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
                bindings[4],
                bindings[5],
                bindings[6],
                bindings[7],
                bindings[8],
            ],
            0,
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
