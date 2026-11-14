//! 自定义 LOD 的真实 GPU 回归；测试入口只调用生产算法，不维护选择逻辑的 CPU 镜像。

use crate::{
    components::{GpuBladePatch, GpuBladeView},
    types::{GpuBladeUltraLodBudget, GpuBladeViewLodConfig},
};
use bevy::prelude::*;
use bevy::render::render_resource::encase;
use std::{borrow::Cow, sync::mpsc, time::Duration};
use wgpu::util::DeviceExt;

const QUERY: &str = r#"
@group(1) @binding(0) var<storage, read_write> lod_results: array<u32>;
@compute @workgroup_size(64) fn validate_lods(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&lod_results)) { return; }
    let fine_cell = vec2<i32>(i32(id.x % 16u) - 8, i32(id.x / 16u) - 8);
    var mask = 0u;
    for (var level = 0u; level < 8u; level = level + 1u) {
        if (screen_space_lod_selects(fine_cell, level, 1.0)) { mask = mask | (1u << level); }
    }
    lod_results[id.x] = mask | (select(0u, 1u, ultra_budget_selects(fine_cell)) << 31u);
}
"#;

#[test]
fn custom_lod_gpu_selects_one_existing_level_without_overlap() {
    // 默认测试仍可在无显卡 CI 运行；显式后端验收必须成功取得对应硬件。
    let Ok(backend) = std::env::var("BLADE_LOD_GPU_BACKEND") else {
        return;
    };
    let (backends, expected) = match backend.as_str() {
        "dx12" => (wgpu::Backends::DX12, wgpu::Backend::Dx12),
        "vulkan" => (wgpu::Backends::VULKAN, wgpu::Backend::Vulkan),
        _ => panic!("BLADE_LOD_GPU_BACKEND 必须为 dx12 或 vulkan"),
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::VALIDATION,
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("指定 GPU 后端必须可用");
    assert_eq!(adapter.get_info().backend, expected);
    let (device, queue) =
        bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production_custom_lod"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(format!("{}\n{QUERY}",crate::shaders::compute_shader_source()))),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("validate_lods"),
        layout: None,
        module: &shader,
        entry_point: Some("validate_lods"),
        compilation_options: Default::default(),
        cache: None,
    });
    for count in 1..=8 {
        let pixels = (0..count - 1)
            .map(|i| {
                let start = 256.0 / 2.0_f32.powi(i as i32);
                Vec2::new(start, start * 1.25)
            })
            .collect::<Vec<_>>();
        let config = GpuBladeViewLodConfig::from_slice(
            &pixels,
            Vec2::new(0.2, 0.6),
            0.7,
            1.0,
            &vec![10_000.0; count],
            0.0,
            &vec![0.0; count - 1],
        )
        .unwrap();
        for local in [false, true] {
            // 全局八档用于无稀疏预算的公开视图；内容窗口另测带预算标志的真实作者路径。
            let mut patch = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 1);
            if local || count == 4 {
                patch = patch.with_ultra_lod_budget(GpuBladeUltraLodBudget::High);
            }
            if local {
                patch.set_view_lod_config(config);
            }
            for level in 0..count {
                let projected = if level == count - 1 {
                    0.0
                } else if level == 0 {
                    512.0
                } else {
                    192.0 / 2.0_f32.powi(level as i32 - 1)
                };
                let results = query(
                    &device,
                    &queue,
                    &pipeline,
                    patch,
                    config,
                    projected.max(0.0001),
                );
                for result in results {
                    let budget_keep = result >> 31 != 0;
                    let actual = result & 0xFF;
                    let budgeted = (local || count == 4) && count >= 4 && level == count - 1;
                    let expected_mask = if budgeted && !budget_keep {
                        0
                    } else {
                        1 << level
                    };
                    assert_eq!(
                        actual, expected_mask,
                        "{backend}: count={count}, local={local}, level={level}"
                    );
                }
            }
            // 遍历全部交接区间，正负世界格都至多由一个存在的档位承担。
            for pair in &pixels {
                for result in query(
                    &device,
                    &queue,
                    &pipeline,
                    patch,
                    config,
                    (pair.x + pair.y) * 0.5,
                ) {
                    let mask = result & 0xFF;
                    assert!(mask.count_ones() <= 1, "交接不能重复绘制父子档");
                    assert_eq!(mask >> count, 0, "不能选中不存在的档位");
                    if !local && count != 4 || count <= 3 {
                        assert_eq!(mask.count_ones(), 1, "无预算裁剪的层级不能出现空档");
                    }
                }
            }
            let mut bounded = config;
            for level in 0..count {
                bounded.residency_radii[level] = 4.0 * 2.0_f32.powi(level as i32);
            }
            bounded.residency_transition_widths.fill(1.0);
            let bounded_patch = if local {
                patch.with_view_lod_config(bounded)
            } else {
                patch
            };
            for result in query(&device, &queue, &pipeline, bounded_patch, bounded, 512.0) {
                assert_eq!(
                    (result & 0xFF).count_ones(),
                    1,
                    "子级驻留边界必须完整交还祖先"
                );
            }
            bounded.residency_radii.fill(0.01);
            bounded.residency_margin = 0.02;
            let absent_patch = if local {
                patch.with_view_lod_config(bounded)
            } else {
                patch
            };
            for result in query(&device, &queue, &pipeline, absent_patch, bounded, 512.0) {
                let budgeted = (local || count == 4) && count >= 4;
                let expected_mask = if budgeted && result >> 31 == 0 {
                    0
                } else {
                    1 << (count - 1)
                };
                assert_eq!(
                    result & 0xFF,
                    expected_mask,
                    "所有子级未驻留时只能保留真实最粗档"
                );
            }
        }
    }
    // 正交缩放决定投影大小；相机因场景包围体后移到67m或更远，不能把近处种植强制降为最粗档。
    let orthographic_config = GpuBladeViewLodConfig::new(
        Vec2::new(12.0, 20.0), Vec2::new(40.0, 56.0), Vec2::new(0.2, 0.6),
        0.7, 1.0, Vec3::splat(10_000.0), 0.0, Vec2::ZERO,
    );
    let orthographic_patch = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 1)
        .with_view_lod_config(orthographic_config);
    for (pixel_scale, expected_level) in [(100.0, 0u32), (40.0, 1), (10.0, 2)] {
        for depth in [1.0, 67.0, 10_000.0] {
            for result in query_view(&device, &queue, &pipeline, orthographic_patch,
                orthographic_config, Vec4::new(0.0, -depth, 0.0, 3.0),
                Vec4::new(0.0, 1.0, 0.0, pixel_scale))
            {
                assert_eq!(result & 0xFF, 1u32 << expected_level,
                    "{backend}: 正交 pixel_scale={pixel_scale}, depth={depth} 应选择同一档");
            }
        }
    }
    eprintln!("{backend}: 1～8 档生产 LOD 交接及正交缩放/深度独立 GPU 读回通过");
}

fn query(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    patch: GpuBladePatch,
    config: GpuBladeViewLodConfig,
    pixels: f32,
) -> Vec<u32> {
    query_view(device, queue, pipeline, patch, config,
        Vec4::new(0.0, -100.0, 0.0, 2.0),
        Vec4::new(0.0, 1.0, 0.0, pixels * 100.35 / 0.7))
}

fn query_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    patch: GpuBladePatch,
    config: GpuBladeViewLodConfig,
    camera_position_enabled: Vec4,
    camera_forward_pixel_scale: Vec4,
) -> Vec<u32> {
    let mut encoded = encase::UniformBuffer::new(Vec::new());
    encoded.write(&patch).unwrap();
    let patch_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &encoded.into_inner(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    // 复用生产组件和 ShaderType 编码，投影沿 Y 轴保证所有父格像素尺寸相同。
    let view = GpuBladeView::screen_space_lod_with(config);
    let uniform = super::culling::GpuBladeViewCullingUniform {
        frustum_planes: [Vec4::ZERO; 6],
        camera_position_enabled,
        camera_forward_pixel_scale,
        lod_pixel_thresholds: view.lod_pixel_thresholds(),
        lod_ultra_pixel_thresholds: view.lod_ultra_pixel_thresholds(),
        camera_fade_lod_params: view.camera_fade_lod_params(),
        lod_residency_radii: view.lod_residency_radii(),
        lod_residency_transition_widths: view.lod_residency_transition_widths(),
        quality_multipliers: Vec4::ONE,
        lod_pixel_thresholds_ext: view.lod_pixel_thresholds_ext(),
        lod_residency_radii_ext: view.lod_residency_radii_ext(),
        lod_residency_transitions_ext: view.lod_residency_transitions_ext(),
        lod_metadata: view.lod_metadata(),
    };
    let mut encoded_view = encase::UniformBuffer::new(Vec::new());
    encoded_view.write(&uniform).unwrap();
    let view_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &encoded_view.into_inner(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let params = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: patch_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: view_buffer.as_entire_binding(),
            },
        ],
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 1024,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 1024,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &params, &[]);
        pass.set_bind_group(1, &group, &[]);
        pass.dispatch_workgroups(4, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 1024);
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = mpsc::sync_channel(1);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .expect("LOD GPU 查询超时");
    receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    bytemuck::cast_slice(&readback.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效")).to_vec()
}
