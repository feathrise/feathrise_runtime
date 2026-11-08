//! 无窗口GPU读回普通Blade的倾斜源姿态；生产打包与实际绘制基底须还原同一方向。
use super::*;

const QUERY: &str = r#"
@compute @workgroup_size(1) fn validate_source_frame(@builtin(global_invocation_id) id:vec3<u32>) {
    let seed=17u+id.x;
    let root=vec2<f32>(f32(id.x)+2.0,3.0);
    let world=(grass_patch.source_to_world*vec4<f32>(root.x,5.0,root.y,1.0)).xyz;
    let up=source_tilt_normal();
    let appearance=pack_tilted_source_appearance(seed,root,0u,1.0,up);
    compact_grass[id.x*3u]=CompactGrassInstance(world.x,world.y,world.z,appearance,up);
    let random=hash_unit(seed^0x4f1bbcdcu)*TWO_PI;
    let expected=normalize(grass_patch.source_to_world[0].xyz)*cos(random)+normalize(grass_patch.source_to_world[2].xyz)*sin(random);
    compact_grass[id.x*3u+1u]=CompactGrassInstance(expected.x,expected.y,expected.z,0u,1u);
    // 真实draw把局部X先转为(cos,0,sin)，再应用生产volume_basis；不在CPU复制基底算法。
    let yaw=vec2<f32>(f32(appearance&255u),f32((appearance>>8u)&255u))/255.0*2.0-vec2<f32>(1.0);
    let actual=volume_basis(up)*vec3<f32>(yaw.x,0.0,yaw.y);
    compact_grass[id.x*3u+2u]=CompactGrassInstance(actual.x,actual.y,actual.z,0u,1u);
}
"#;

pub(super) fn query() -> String {
    // 与现有体积姿态回归一样抽取真实绘制函数原文，专项不需要运行其他重型验证组。
    let draw=include_str!("../shaders/gpu_grass_minimal.wesl");
    let start=draw.find("fn volume_basis(").unwrap();
    let end=start+draw[start..].find("fn world_tip_to_surface(").unwrap();
    format!("{}\n{QUERY}",&draw[start..end])
}

pub(super) fn validate(device:&wgpu::Device,queue:&wgpu::Queue,shader:&wgpu::ShaderModule) {
    let compute=pipeline(device,shader,"validate_source_frame");
    // 纯yaw须与实际draw的(cos,0,sin)同号；带yaw的极小倾斜同时跨越零法线分支验证连续性。
    for rotation in [Quat::IDENTITY,Quat::from_rotation_y(0.6),Quat::from_rotation_y(-0.6),
        Quat::from_euler(EulerRot::YXZ,0.6,0.0001,0.0),Quat::from_euler(EulerRot::YXZ,-0.6,-0.0001,0.0001),
        Quat::from_rotation_x(0.4),Quat::from_rotation_z(-0.7),
        Quat::from_euler(EulerRot::YXZ,0.6,0.3,-0.2),Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        Quat::from_rotation_x(std::f32::consts::PI)] {
        let frame=Transform::from_xyz(12.,-8.,7.).with_rotation(rotation).with_scale(Vec3::new(2.,3.,0.5)).to_matrix();
        let mut uniform=[[0u32;4];26];
        uniform[18..22].copy_from_slice(&frame.to_cols_array_2d().map(|column|column.map(f32::to_bits)));
        uniform[22..26].copy_from_slice(&frame.inverse().to_cols_array_2d().map(|column|column.map(f32::to_bits)));
        let patch=device.create_buffer_init(&wgpu::util::BufferInitDescriptor {label:Some("tilted_blade_source"),
            contents:bytemuck::cast_slice(&uniform),usage:wgpu::BufferUsages::UNIFORM});
        let output=device.create_buffer(&wgpu::BufferDescriptor {label:Some("tilted_blade_output"),size:240,
            usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_SRC,mapped_at_creation:false});
        let read=device.create_buffer(&wgpu::BufferDescriptor {label:Some("tilted_blade_read"),size:240,
            usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
        let group=device.create_bind_group(&wgpu::BindGroupDescriptor {label:Some("tilted_blade_bind"),
            layout:&compute.get_bind_group_layout(0),entries:&[
                wgpu::BindGroupEntry {binding:0,resource:patch.as_entire_binding()},
                wgpu::BindGroupEntry {binding:3,resource:output.as_entire_binding()}]});
        let mut encoder=device.create_command_encoder(&default());
        {let mut pass=encoder.begin_compute_pass(&default());pass.set_pipeline(&compute);pass.set_bind_group(0,&group,&[]);pass.dispatch_workgroups(4,1,1);}
        encoder.copy_buffer_to_buffer(&output,0,&read,0,240);
        let submission=queue.submit([encoder.finish()]);
        let (tx,rx)=mpsc::sync_channel(1);
        read.slice(..).map_async(wgpu::MapMode::Read,move |result| {let _=tx.send(result);});
        device.poll(wgpu::PollType::Wait {submission_index:Some(submission),timeout:Some(Duration::from_secs(30))}).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let data=read.slice(..).get_mapped_range().expect("读回缓冲映射范围必须有效");
        let records:&[[u32;5]]=bytemuck::cast_slice(&data);
        for index in 0..4 {
            let record=records[index*3]; let expected=records[index*3+1]; let actual=records[index*3+2];
            let actual_root=Vec3::new(f32::from_bits(record[0]),f32::from_bits(record[1]),f32::from_bits(record[2]));
            assert!(actual_root.distance(frame.transform_point3(Vec3::new(index as f32+2.,5.,3.)))<0.0001);
            let up=if record[4]==0 {Vec3::Y} else {
                let bytes=record[4].to_le_bytes();Vec3::new(bytes[0] as i8 as f32,bytes[1] as i8 as f32,bytes[2] as i8 as f32).normalize()
            };
            assert!(up.dot(rotation*Vec3::Y)>0.9999,"普通草叶未跟随源上方向");
            let axis=Vec3::new(f32::from_bits(actual[0]),f32::from_bits(actual[1]),f32::from_bits(actual[2]));
            let expected=Vec3::new(f32::from_bits(expected[0]),f32::from_bits(expected[1]),f32::from_bits(expected[2]));
            assert!(axis.normalize().dot(expected)>0.9998,"普通草叶朝向未组合完整源旋转");
        }
        drop(data);read.unmap();
    }
}
