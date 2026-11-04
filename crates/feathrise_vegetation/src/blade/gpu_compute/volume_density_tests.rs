//! 相同水平承托面上的两种草运行完整生产Compute，逐株核对密度与LOD外观。
use super::culling::GpuBladeViewCullingUniform;
use crate::blade::{
    components::{GpuBladePatch, GpuBladeVolumeSource},
    types::GpuBladeViewLodConfig,
};
use bevy::{
    prelude::*,
    render::render_resource::{ShaderType, encase},
};
use std::{borrow::Cow, sync::mpsc, time::Duration};
use wgpu::util::DeviceExt;

const CAPACITY: u32 = 4096;

struct Harness {
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}
impl Harness {
    fn new(device: &wgpu::Device) -> Self {
        let entries = (0..13)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: match binding {
                    0 | 5 | 9 => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    3 | 4 | 6 | 7 | 8 => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    _ => wgpu::BindingType::Texture {
                        sample_type: if binding == 11 {
                            wgpu::TextureSampleType::Uint
                        } else {
                            wgpu::TextureSampleType::Float { filterable: false }
                        },
                        view_dimension: if binding == 10 {
                            wgpu::TextureViewDimension::D2Array
                        } else {
                            wgpu::TextureViewDimension::D2
                        },
                        multisampled: false,
                    },
                },
            })
            .collect::<Vec<_>>();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("density_parity"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production_density_parity"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(crate::shaders::compute_shader_source())),
        });
        let pipelines = [
            "cull_tiles",
            "finalize_tiles",
            "classify_and_compact",
            "finalize_indirect",
        ]
        .into_iter()
        .map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        })
        .collect();
        Self { layout, pipelines }
    }

    fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        patch: GpuBladePatch,
        view: &GpuBladeViewCullingUniform,
        density: &[u8],
        image: &Image,
    ) -> Vec<[u32; 4]> {
        self.run_with_clipmap(
            device,
            queue,
            patch,
            view,
            density,
            image,
            Default::default(),
        )
    }

    fn run_with_clipmap(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        patch: GpuBladePatch,
        view: &GpuBladeViewCullingUniform,
        density: &[u8],
        image: &Image,
        clipmap: super::clipmap_resources::ClipmapUniform,
    ) -> Vec<[u32; 4]> {
        let buffer = |data: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: data,
                usage,
            })
        };
        let uniform = buffer(&encode(&patch), wgpu::BufferUsages::UNIFORM);
        let view = buffer(&encode(view), wgpu::BufferUsages::UNIFORM);
        let clipmap = buffer(&encode(&clipmap), wgpu::BufferUsages::UNIFORM);
        let compact = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(CAPACITY) * 20,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let side = patch.grid.x;
        let tiles = side.div_ceil(16).pow(2);
        let mut state = [0u32; 20];
        state[0] = 3;
        state[10] = CAPACITY;
        state[11] = patch.instance_count();
        state[13] = patch.lod_level();
        state[15] = tiles;
        let state = buffer(
            bytemuck::cast_slice(&state),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let active = buffer(&vec![0; tiles as usize * 4], wgpu::BufferUsages::STORAGE);
        let dispatch = buffer(
            &[0; 16],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
        );
        let bytes = if patch.uses_volume_source() {
            super::helpers::encode_volume_depth_bucket_draws(3, 0, 0, CAPACITY)
        } else {
            super::helpers::encode_depth_bucket_draws(3, 0, 0, CAPACITY)
        };
        let buckets = buffer(&bytes, wgpu::BufferUsages::STORAGE);
        let height = texture(device, wgpu::TextureFormat::Rgba32Float, 1, 1);
        let probability = texture(device, wgpu::TextureFormat::R8Unorm, 17, 17);
        upload(queue, &probability, density, 17);
        let table = texture(device, wgpu::TextureFormat::Rgba32Uint, 1, 1);
        upload(
            queue,
            &table,
            bytemuck::cast_slice(&[1u32, 2f32.to_bits(), 0, 0]),
            16,
        );
        let volume = texture(
            device,
            wgpu::TextureFormat::Rgba32Float,
            image.width(),
            image.height(),
        );
        upload(
            queue,
            &volume,
            image.data.as_ref().unwrap(),
            image.width() * 16,
        );
        let height_view = height.create_view(&Default::default());
        let atlas_view = height.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let probability_view = probability.create_view(&Default::default());
        let table_view = table.create_view(&Default::default());
        let volume_view = volume.create_view(&Default::default());
        let make_group = |dispatch: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&height_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&probability_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: compact.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: state.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: view.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: active.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: dispatch.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: buckets.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: clipmap.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::TextureView(&atlas_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::TextureView(&table_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: wgpu::BindingResource::TextureView(&volume_view),
                    },
                ],
            })
        };
        let group = make_group(&dispatch);
        // 正式路径同样隔离binding7，防止间接参数在一次派发同时成为可写Storage。
        let classify_dispatch = buffer(&[0; 16], wgpu::BufferUsages::STORAGE);
        let classify_group = make_group(&classify_dispatch);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(CAPACITY) * 20 + 80,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        // 与正式RenderGraph相同的四阶段；CPU不提供期望根或密度算法镜像。
        for (index, pipeline) in self.pipelines.iter().enumerate() {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, if index < 2 { &group } else { &classify_group }, &[]);
            if index == 2 {
                pass.dispatch_workgroups_indirect(&dispatch, 0);
            } else {
                pass.dispatch_workgroups(if index == 0 { tiles.div_ceil(64) } else { 1 }, 1, 1);
            }
        }
        encoder.copy_buffer_to_buffer(&state, 0, &readback, 0, 80);
        encoder.copy_buffer_to_buffer(&compact, 0, &readback, 80, u64::from(CAPACITY) * 20);
        let submission = queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::sync_channel(1);
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender.send(r);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(30)),
            })
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let bytes = readback.slice(..).get_mapped_range();
        let state: &[u32] = bytemuck::cast_slice(&bytes[..80]);
        assert_eq!(state[6], 0, "不能把容量溢出误判为密度一致");
        let mut output = bytemuck::cast_slice::<u8, [u32; 5]>(&bytes[80..])
            .iter()
            // Clipmap重心插值的平面高度允许最后一位舍入；XYZ的层归属仍须全部为Y=2。
            .filter(|v| (f32::from_bits(v[1]) - 2.).abs() < 0.00001)
            .map(|v| [v[0], 2f32.to_bits(), v[2], v[3]])
            .collect::<Vec<_>>();
        assert_eq!(output.len(), state[1] as usize);
        output.sort_unstable();
        assert!(
            output.windows(2).all(|w| w[0] != w[1]),
            "同一父格不能重复生成草簇"
        );
        output
    }
}

fn encode<T: ShaderType + encase::internal::WriteInto>(value: &T) -> Vec<u8> {
    let mut buffer = encase::UniformBuffer::new(Vec::new());
    buffer.write(value).unwrap();
    buffer.into_inner()
}
fn texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}
fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, bytes: &[u8], stride: u32) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(texture.height()),
        },
        texture.size(),
    );
}

pub(super) fn validate(device: &wgpu::Device, queue: &wgpu::Queue) {
    let harness = Harness::new(device);
    let config = |spacing| {
        GpuBladeViewLodConfig::from_slice(
            &[Vec2::new(20., 24.), Vec2::new(8., 12.), Vec2::new(2., 4.)],
            Vec2::new(0.2, 0.6),
            0.8,
            spacing,
            &[5., 8., 12., 16.],
            0.1,
            &[1., 1., 1.],
        )
        .unwrap()
    };
    let mut compared = 0;
    for pattern in 0..5 {
        // 中央2m作者区，四周保留真实零概率承托面；两边都允许宽父簇落在刷印之外。
        let density = (0..17 * 17)
            .map(|i| {
                let x = i % 17 - 8i32;
                let z = i / 17 - 8i32;
                if x.abs() > 4 || z.abs() > 4 {
                    return 0;
                }
                match pattern {
                    0 => 255,
                    1 => 96,
                    2 => 0,
                    3 => (((x + 4) * 31 + (z + 4) * 17) % 256) as u8,
                    _ => {
                        if x == 1 && z == 1 {
                            96
                        } else {
                            0
                        }
                    } // 稀疏边缘的父格代理允许落在零概率承托格。
                }
            })
            .collect::<Vec<_>>();
        let cells = density
            .iter()
            .enumerate()
            .map(|(i, probability)| {
                let x = (i % 17) as f32 * 0.25 - 2.;
                let z = (i / 17) as f32 * 0.25 - 2.;
                [
                    [x, 2., z, f32::from(*probability) / 255.],
                    [0., 1., 0., 0.25],
                    [x, 2., z, 1.],
                ]
            })
            .collect::<Vec<_>>();
        let image = GpuBladeVolumeSource::image_from_cells(&cells, 8 * 1024 * 1024).unwrap();
        for spacing in [0.125_f32, 0.25, 0.1] {
            let config = config(spacing);
            for (pixels, quality, gain, camera_y, focus_x, radius) in [
                (64., 1., 1., 30., 0., 16.),
                (16., 1., 1., 30., 0., 16.),
                (6., 1., 1., 30., 0., 16.),
                (1., 1., 1., 30., 0., 16.),
                (22., 0.35, 2., 30., 0., 16.),
                (10., 0.35, 8., 30., 0., 16.),
                (3., 1., 1., 30., 0., 16.),
                (64., 0., 8., 30., 0., 16.),
                (64., 1., 1., 2.3, 0., 16.),
                (22., 1., 1., 30., 4.5, 16.),
                (64., 1., 1., 30., 0., 1.5),
            ] {
                let view = GpuBladeViewCullingUniform {
                    frustum_planes: [Vec4::new(0., 0., 0., 1.0e8); 6],
                    camera_position_enabled: Vec4::new(0., camera_y, 0., 2.),
                    camera_forward_pixel_scale: Vec4::new(
                        0.,
                        -1.,
                        0.,
                        pixels * (camera_y - 2.4).max(0.01) / 0.8,
                    ),
                    lod_pixel_thresholds: Vec4::ZERO,
                    lod_ultra_pixel_thresholds: Vec4::ZERO,
                    camera_fade_lod_params: Vec4::new(0.2, 0.6, 0.8, 0.125),
                    lod_residency_radii: Vec4::ZERO,
                    lod_residency_transition_widths: Vec4::ZERO,
                    quality_multipliers: Vec4::new(quality, 1., 1., 0.),
                    lod_pixel_thresholds_ext: [Vec4::ZERO; 2],
                    lod_residency_radii_ext: Vec4::ZERO,
                    lod_residency_transitions_ext: Vec4::ZERO,
                    lod_metadata: Vec4::ZERO,
                };
                for level in 0..4 {
                    let side = 1u32 << level;
                    let mut outdoor = GpuBladePatch::square(
                        Vec3::new(0., 2., 0.),
                        4.,
                        (4. / spacing).round() as u32 / side + 1,
                        17,
                    )
                    .with_lod_level(level)
                    .with_hierarchical_lod(true)
                    .with_view_lod_config(config)
                    .with_lod_range(0., radius, 0.01, 0.25)
                    .with_density_gain(gain)
                    .with_density_field(Vec2::splat(-2.), Vec2::splat(2.));
                    outdoor.set_focus(Vec3::new(focus_x, 2., 0.));
                    let volume = outdoor.with_volume_cell_source(cells.len() as u32, 0.25);
                    let expected = harness.run(device, queue, outdoor, &view, &density, &image);
                    let actual = harness.run(device, queue, volume, &view, &density, &image);
                    assert!(
                        actual == expected,
                        "两种草应逐株一致：spacing={spacing}, pattern={pattern}, level={level}, pixels={pixels}, quality={quality}, gain={gain}, focus={focus_x}, radius={radius}, counts={}/{}, extra={:?}, missing={:?}",
                        actual.len(),
                        expected.len(),
                        actual
                            .iter()
                            .filter(|v| expected.binary_search(v).is_err())
                            .take(3)
                            .collect::<Vec<_>>(),
                        expected
                            .iter()
                            .filter(|v| actual.binary_search(v).is_err())
                            .take(3)
                            .collect::<Vec<_>>()
                    );
                    if quality == 0. || pattern == 2 {
                        assert!(actual.is_empty());
                    }
                    if pattern == 0
                        && pixels == 64.
                        && level == 0
                        && quality == 1.
                        && focus_x == 0.
                        && radius == 16.
                        && camera_y == 30.
                    {
                        assert!(!actual.is_empty());
                    }
                    compared += 1;
                }
            }
        }
    }
    eprintln!("普通/体素草完整四阶段逐株一致：{compared}组密度、LOD、过渡、画质、近镜头与距离窗口");
    validate_seams(device, queue, &harness);
}

/// 同一平面由两种后端各负责一半，联合草根必须与未切割平面逐株一致。
/// 不只比较两边各自的平面：父格中心和抖动后的根可以分别落在接缝两侧。
fn validate_seams(device: &wgpu::Device, queue: &wgpu::Queue, harness: &Harness) {
    use super::clipmap_resources::ClipmapUniform;
    let mut compared = 0;
    for seam in [-0.125_f32, 0., 0.07, 0.25] {
        for pattern in [0, 1, 2, 3, 4] {
            let phase = if pattern == 4 { 0.07 } else { 0. };
            let density = (0..17 * 17)
                .map(|i| {
                    let x = i % 17 - 8i32;
                    let z = i / 17 - 8i32;
                    if pattern == 4 {
                        return 96;
                    }
                    if x.abs() > 4 || z.abs() > 4 {
                        return 0;
                    }
                    match pattern {
                        0 => 255,
                        1 => 96,
                        2 => ((x + 4) * 23 + (z + 4) * 7) as u8,
                        _ => {
                            if x as f32 * 0.25 > seam {
                                255
                            } else {
                                96
                            }
                        }
                    }
                })
                .collect::<Vec<_>>();
            // 三维笔触已改变内侧密度，室外R8仍是旧值；必须双向读取正确所有者。
            let outdoor_density = density
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if pattern == 3 && (i % 17) as f32 * 0.25 - 2. > seam {
                        0
                    } else {
                        *p
                    }
                })
                .collect::<Vec<_>>();
            let cells = density
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    let x = (i % 17) as f32 * 0.25 - 2.;
                    let z = (i / 17) as f32 * 0.25 - 2.;
                    // 边界穿过作者格时保留其内侧半格；BVH承托点可以位于该格内部而非中心。
                    let x = x + phase;
                    let z = z + phase;
                    (x + 0.125 > seam).then_some([
                        [
                            x.max(seam + 0.00001),
                            2.,
                            z.clamp(-1.99999, 1.99999),
                            f32::from(*p) / 255.,
                        ],
                        [0., 1., 0., 0.25],
                        [x, 2., z, 1.],
                    ])
                })
                .collect::<Vec<_>>();
            let image = GpuBladeVolumeSource::image_from_cells(&cells, 8 * 1024 * 1024).unwrap();
            let mut topology = ClipmapUniform {
                map_bounds: Vec4::new(-2., -2., 2., 2.),
                grid: Vec4::new(0.25, 0.25, 16., 16.),
                options: Vec4::new(0., 1., 1., 0.),
                ..default()
            };
            topology.bands[0] = UVec4::new(0, 0, 16, 16);
            let mut cut = topology;
            cut.hole_info.x = 1.;
            cut.holes[0] = Vec4::new(seam, -2., 2., 2.);
            for spacing in [0.125_f32, 0.1] {
                let config = GpuBladeViewLodConfig::from_slice(
                    &[Vec2::new(20., 24.), Vec2::new(8., 12.), Vec2::new(2., 4.)],
                    Vec2::new(0.2, 0.6),
                    0.8,
                    spacing,
                    &[5., 8., 12., 16.],
                    0.1,
                    &[1., 1., 1.],
                )
                .unwrap();
                for pixels in [64., 10., 1.] {
                    let view = GpuBladeViewCullingUniform {
                        frustum_planes: [Vec4::new(0., 0., 0., 1.0e8); 6],
                        camera_position_enabled: Vec4::new(0., 30., 0., 2.),
                        camera_forward_pixel_scale: Vec4::new(0., -1., 0., pixels * 27.6 / 0.8),
                        quality_multipliers: Vec4::ONE,
                        lod_pixel_thresholds: Vec4::ZERO,
                        lod_ultra_pixel_thresholds: Vec4::ZERO,
                        camera_fade_lod_params: Vec4::new(0.2, 0.6, 0.8, spacing),
                        lod_residency_radii: Vec4::ZERO,
                        lod_residency_transition_widths: Vec4::ZERO,
                        lod_pixel_thresholds_ext: [Vec4::ZERO; 2],
                        lod_residency_radii_ext: Vec4::ZERO,
                        lod_residency_transitions_ext: Vec4::ZERO,
                        lod_metadata: Vec4::ZERO,
                    };
                    for level in 0..4 {
                        let outdoor = GpuBladePatch::square(
                            Vec3::new(0., 2., 0.),
                            4.,
                            (4. / spacing).round() as u32 / (1 << level) + 1,
                            17,
                        )
                        .with_lod_level(level)
                        .with_hierarchical_lod(true)
                        .with_view_lod_config(config)
                        .with_lod_range(0., 16., 0.01, 0.25)
                        .with_density_field(Vec2::splat(-2.), Vec2::splat(2.));
                        let expected = harness.run_with_clipmap(
                            device, queue, outdoor, &view, &density, &image, topology,
                        );
                        let mut actual = harness.run_with_clipmap(
                            device,
                            queue,
                            outdoor,
                            &view,
                            &outdoor_density,
                            &image,
                            cut,
                        );
                        actual.extend(harness.run_with_clipmap(
                            device,
                            queue,
                            outdoor.with_volume_cell_source(cells.len() as u32, 0.25),
                            &view,
                            &outdoor_density,
                            &image,
                            cut,
                        ));
                        actual.sort_unstable();
                        assert_eq!(
                            actual.len(),
                            actual
                                .iter()
                                .collect::<std::collections::BTreeSet<_>>()
                                .len(),
                            "交接不能产生重复草根"
                        );
                        assert!(
                            actual == expected,
                            "拼接漏草：seam={seam}, pattern={pattern}, spacing={spacing}, level={level}, pixels={pixels}, counts={}/{}, missing={:?}, extra={:?}",
                            actual.len(),
                            expected.len(),
                            expected
                                .iter()
                                .filter(|v| actual.binary_search(v).is_err())
                                .take(3)
                                .collect::<Vec<_>>(),
                            actual
                                .iter()
                                .filter(|v| expected.binary_search(v).is_err())
                                .take(3)
                                .collect::<Vec<_>>()
                        );
                        if seam == 0.
                            && pattern == 0
                            && spacing == 0.125
                            && level == 0
                            && pixels == 64.
                        {
                            let exterior_only = expected
                                .iter()
                                .copied()
                                .filter(|v| f32::from_bits(v[0]) <= seam)
                                .collect::<Vec<_>>();
                            for (y, normal) in [(-2., Vec3::Y), (8., Vec3::Y), (2., Vec3::NEG_Y)] {
                                let other_layer = cells
                                    .iter()
                                    .map(|cell| {
                                        let mut cell = *cell;
                                        cell[0][1] = y;
                                        cell[0][3] = 0.;
                                        cell[1][..3].copy_from_slice(&normal.to_array());
                                        cell[2][1] = y;
                                        cell
                                    })
                                    .collect::<Vec<_>>();
                                let other_image = GpuBladeVolumeSource::image_from_cells(
                                    &other_layer,
                                    8 * 1024 * 1024,
                                )
                                .unwrap();
                                assert_eq!(
                                    harness.run_with_clipmap(
                                        device,
                                        queue,
                                        outdoor,
                                        &view,
                                        &density,
                                        &other_image,
                                        cut
                                    ),
                                    exterior_only,
                                    "室外不能把同XZ另一层的零密度当成自身擦除"
                                );
                                assert!(
                                    harness
                                        .run_with_clipmap(
                                            device,
                                            queue,
                                            outdoor.with_volume_cell_source(
                                                other_layer.len() as u32,
                                                0.25
                                            ),
                                            &view,
                                            &density,
                                            &other_image,
                                            cut
                                        )
                                        .is_empty(),
                                    "空洞底或洞顶不能借室外R8生草"
                                );
                            }
                        }
                        compared += 1;
                    }
                }
            }
        }
    }
    eprintln!("混合地形边界联合草根无漏草、无重复：{compared}组");
}
