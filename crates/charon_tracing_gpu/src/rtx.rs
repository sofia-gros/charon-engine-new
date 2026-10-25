use super::*;
use wgpu::util::DeviceExt;
pub struct Acceleration {
    pub group: wgpu::BindGroup,
    _blas: wgpu::Blas,
    _tlas: wgpu::Tlas,
    _vertices: wgpu::Buffer,
    _faces: wgpu::Buffer,
}
/// Extract only air/solid boundaries, extruded through z=0. Dynamic edits rebuild
/// these boundaries. Changing only sources/reflectance does not rebuild the BVH.
type BoundaryMesh = (Vec<[f32; 3]>, Vec<[u32; 4]>);
pub fn mesh(scene: &TraceScene) -> Result<BoundaryMesh, String> {
    let g = scene.input.geometry;
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    for y in 0..g.height {
        for x in 0..g.width {
            let i = y * g.width + x;
            if scene.input.optics[i as usize].opacity < 0.999 {
                continue;
            }
            for (axis, sign) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let (nx, ny) = if axis == 0 {
                    (x as i32 + if sign == 0 { -1 } else { 1 }, y as i32)
                } else {
                    (x as i32, y as i32 + if sign == 0 { -1 } else { 1 })
                };
                if nx >= 0
                    && ny >= 0
                    && nx < g.width as i32
                    && ny < g.height as i32
                    && scene.input.optics[(ny as u32 * g.width + nx as u32) as usize].opacity
                        >= 0.999
                {
                    continue;
                }
                if faces.len() >= 262144 {
                    return Err(
                        "RTX boundary mesh exceeds 262144 faces; reduce lighting resolution".into(),
                    );
                }
                let (a, b) = if axis == 0 {
                    (
                        [x as f32 + sign as f32, y as f32],
                        [x as f32 + sign as f32, y as f32 + 1.],
                    )
                } else {
                    (
                        [x as f32, y as f32 + sign as f32],
                        [x as f32 + 1., y as f32 + sign as f32],
                    )
                };
                vertices.extend_from_slice(&[
                    [a[0], a[1], -0.5],
                    [b[0], b[1], -0.5],
                    [b[0], b[1], 0.5],
                    [a[0], a[1], -0.5],
                    [b[0], b[1], 0.5],
                    [a[0], a[1], 0.5],
                ]);
                faces.push([i, axis, sign, 0]);
            }
        }
    }
    if vertices.is_empty() {
        vertices = vec![[-100000., -100000., -0.5]; 6];
        faces.push([0; 4]);
    }
    Ok((vertices, faces))
}
impl Acceleration {
    pub fn build(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &TraceScene,
        pipeline: &wgpu::ComputePipeline,
    ) -> Result<Self, String> {
        let (vertices, faces) = mesh(scene)?;
        let limits = device.limits();
        if limits.max_blas_primitive_count < faces.len() as u32 * 2
            || limits.max_blas_geometry_count < 1
            || limits.max_tlas_instance_count < 1
            || limits.max_acceleration_structures_per_shader_stage < 1
        {
            return Err("RTX acceleration limits were not enabled when creating this device; recreate the device".into());
        }

        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("opaque boundary triangles"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let fb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("boundary optical cells and normals"),
            contents: bytemuck::cast_slice(&faces),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: vertices.len() as u32,
            index_format: None,
            index_count: None,
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("2D opaque geometry"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("2D tracing scene"),
            max_instances: 1,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        *tlas.get_mut_single(0).unwrap() = Some(wgpu::TlasInstance::new(
            &blas,
            [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
            0,
            255,
        ));
        let entry = wgpu::BlasBuildEntry {
            blas: &blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                size: &size,
                vertex_buffer: &vb,
                first_vertex: 0,
                vertex_stride: 12,
                index_buffer: None,
                first_index: None,
                transform_buffer: None,
                transform_buffer_offset: None,
            }]),
        };
        encoder.build_acceleration_structures([&entry], [&tlas]);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hardware ray queries"),
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: tlas.as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fb.as_entire_binding(),
                },
            ],
        });
        Ok(Self {
            group,
            _blas: blas,
            _tlas: tlas,
            _vertices: vb,
            _faces: fb,
        })
    }
}
/// Probe the actual query unit using a known wall, before advertising readiness.
/// No configuration or restart is performed here; the caller supplies the device.
pub async fn verify_device(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<bool, String> {
    use charon_tracing::charon_lighting::*;
    let g = Geometry {
        width: 3,
        height: 3,
        origin: [0.; 2],
        cell_size: 1.,
    };
    let mut scene = Scene::new(g, Settings::default())?;
    let mut grid = OpticalGrid::new(g)?;
    for y in 0..3 {
        grid.set(1, y, Optical::OPAQUE)?;
    }
    scene.set_layer(1, grid)?;
    let input = TraceScene::new(scene.prepare())?;
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let result = (|| -> Result<bool, String> {
        let source = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RTX startup probe"),
            source: wgpu::ShaderSource::Wgsl(shader(Mode::Rtx).into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("verify hardware ray query"),
            layout: None,
            module: &source,
            entry_point: Some("verify_rtx"),
            compilation_options: Default::default(),
            cache: None,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RTX probe result"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RTX probe readback"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RTX probe data"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 6,
                resource: output.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let accel = Acceleration::build(device, &mut encoder, &input, &pipeline)?;
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &accel.group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, 16);
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
        let mapped = staging.slice(..).get_mapped_range();
        let value = bytemuck::cast_slice::<u8, f32>(&mapped);
        let passed = value[0] == 1. && (value[1] - 0.5).abs() < 0.001 && value[2] == 4.;
        drop(mapped);
        staging.unmap();
        Ok(passed)
    })();
    let validation = device.pop_error_scope().await;
    if let Some(e) = validation {
        return Err(e.to_string());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adjacent_opaque_cells_share_no_internal_triangles() {
        use charon_tracing::charon_lighting::*;
        let mut scene = Scene::new(
            Geometry {
                width: 2,
                height: 1,
                origin: [0.; 2],
                cell_size: 1.,
            },
            Settings::default(),
        )
        .unwrap();
        let mut grid = OpticalGrid::new(scene.geometry()).unwrap();
        grid.set(0, 0, Optical::OPAQUE).unwrap();
        grid.set(1, 0, Optical::OPAQUE).unwrap();
        scene.set_layer(1, grid).unwrap();
        let (vertices, faces) = mesh(&TraceScene::new(scene.prepare()).unwrap()).unwrap();
        assert_eq!(faces.len(), 6);
        assert_eq!(vertices.len(), 36);
    }
}
