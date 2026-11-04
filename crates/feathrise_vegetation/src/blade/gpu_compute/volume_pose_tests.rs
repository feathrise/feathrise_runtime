//! 真实绘制辅助函数的姿态回归：水平体素草与室外一致，斜面只投影世界形变。
use bevy::prelude::*;
use std::{borrow::Cow, sync::mpsc, time::Duration};
use wgpu::util::DeviceExt;

const DRAW: &str = include_str!("../shaders/gpu_grass_minimal.wgsl");
const QUERY: &str = r#"
struct PoseResult {
    x:vec4<f32>, y:vec4<f32>, z:vec4<f32>,
    current:vec4<f32>, previous:vec4<f32>, force:vec4<f32>,
}
@group(0) @binding(0) var<storage,read> test_normals:array<vec4<f32>>;
@group(0) @binding(1) var<storage,read_write> poses:array<PoseResult>;
@compute @workgroup_size(64)
fn validate_volume_pose(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&test_normals) {return;}
    let packed=select(pack4x8snorm(vec4<f32>(test_normals[id.x].xyz,1.0)),0u,id.x==0u);
    let basis=volume_basis(packed);
    let root=vec3<f32>(12.0,3.0,-8.0);
    let tip=world_tip_to_surface(basis,vec2<f32>(0.12,-0.24));
    let previous_tip=world_tip_to_surface(basis,vec2<f32>(-0.17,0.08));
    let current=grass_surface_position(root,root+vec3<f32>(0.15+tip.x*0.65,0.73-0.02*0.65,-0.07+tip.y*0.65),basis);
    let previous=grass_surface_position(root,root+vec3<f32>(0.15+previous_tip.x*0.65,0.73-0.006*0.65,-0.07+previous_tip.y*0.65),basis);
    poses[id.x]=PoseResult(vec4<f32>(basis[0],0),vec4<f32>(basis[1],0),vec4<f32>(basis[2],0),
        vec4<f32>(current,1),vec4<f32>(previous,1),vec4<f32>(basis*vec3<f32>(tip.x,0,tip.y),0));
}
"#;

fn source() -> String {
    // 抽取生产函数原文，不复制基底算法；Bevy导入与材质宏不参与这个固定小型诊断。
    let start = DRAW.find("fn volume_basis(").unwrap();
    let end = DRAW[start..].find("struct GrassVertex {").unwrap() + start;
    format!("{}\n{QUERY}", &DRAW[start..end])
}

#[test]
fn volume_pose_helpers_are_valid_wgsl() {
    let module = wgpu::naga::front::wgsl::parse_str(&source()).unwrap();
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

pub(super) fn validate(device: &wgpu::Device, queue: &wgpu::Queue) {
    let normals = [
        Vec3::Y,
        Vec3::Y, // 零编码室外草与实际编码水平体素草必须逐项相同。
        Vec3::new(0.30, 1.0, 0.10),
        Vec3::new(0.32, 1.0, 0.10), // 跨越旧abs(n.y)>0.95切轴处。
        Vec3::X,
        Vec3::Z,
        -Vec3::Y,
        Vec3::new(0.03, -1.0, 0.04),
        -Vec3::X,
        -Vec3::Z,
    ];
    let inputs = normals.map(|n| n.normalize().extend(0.0).to_array());
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("production_volume_blade_pose"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(source())),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("validate_volume_pose"),
        layout: None,
        module: &shader,
        entry_point: Some("validate_volume_pose"),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("pose_normals"),
        contents: bytemuck::cast_slice(&inputs),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let size = (normals.len() * 96) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pose_results"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pose_readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = mpsc::sync_channel(1);
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = sender.send(r);
    });
    // 同步等待只存在于测试，正式帧路径不引入回读或CPU草叶变换。
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
    let results: &[[f32; 24]] = bytemuck::cast_slice(&bytes);
    let root = Vec3::new(12.0, 3.0, -8.0);
    for (i, (input, result)) in inputs.iter().zip(results).enumerate() {
        let vector = |offset| Vec3::from_slice(&result[offset..offset + 3]);
        let basis = Mat3::from_cols(vector(0), vector(4), vector(8));
        // 按snorm8恢复实际GPU输入，再以正交性、最短旋转和世界切向投影作为独立几何合同。
        let normal = Vec3::from_slice(input)
            .map(|v| (v * 127.0).round() / 127.0)
            .normalize();
        assert!(
            (basis.determinant() - 1.0).abs() < 0.001,
            "{i}: 基底必须保持右手正交"
        );
        assert!(basis.y_axis.abs_diff_eq(normal, 0.0001));
        if i <= 1 {
            assert!(
                basis.abs_diff_eq(Mat3::IDENTITY, 0.00001),
                "水平体素草被额外旋转：{basis:?}"
            );
        }
        let project = |v: Vec3| v - normal * v.dot(normal);
        assert!(
            vector(20).abs_diff_eq(project(Vec3::new(0.12, 0.0, -0.24)), 0.0001),
            "{i}: 世界风和踩踏被旋转"
        );
        if i != 6 {
            let geometry = Quat::from_rotation_arc(Vec3::Y, normal) * Vec3::new(0.15, 0.73, -0.07);
            for (offset, wind, drop) in [
                (12, Vec3::new(0.12, 0.0, -0.24), 0.02),
                (16, Vec3::new(-0.17, 0.0, 0.08), 0.006),
            ] {
                let expected = root + geometry + project(wind) * 0.65 - normal * drop * 0.65;
                assert!(
                    vector(offset).abs_diff_eq(expected, 0.0005),
                    "{i}: 当前或历史姿态不连续，actual={:?}, expected={expected:?}",
                    vector(offset)
                );
            }
        }
    }
    let read_basis = |i: usize| {
        Mat3::from_cols(
            Vec3::from_slice(&results[i][0..3]),
            Vec3::from_slice(&results[i][4..7]),
            Vec3::from_slice(&results[i][8..11]),
        )
    };
    assert!(
        read_basis(2).abs_diff_eq(read_basis(3), 0.04),
        "小幅坡度变化不能触发切轴翻转"
    );
}
