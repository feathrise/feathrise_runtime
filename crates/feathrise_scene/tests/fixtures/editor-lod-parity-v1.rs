//! 两套生产LOD状态机共用的800三角夹具；仅用于测试，不进入游戏或编辑器产物。
//! 相同顶点/索引/法线/UV、978×288像素、透视相机与16px误差；报告实际句柄内容而非只比较设置。
use bevy::{asset::RenderAssetUsages, camera::{ComputedCameraValues, RenderTargetInfo, primitives::Aabb},
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues}, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD);
    let mut positions = Vec::new(); let mut uvs = Vec::new(); let mut indices = Vec::new();
    for z in 0..=20 { for x in 0..=20 {
        let px = x as f32 * 0.2 - 2.0; let pz = z as f32 * 0.2 - 2.0;
        positions.push([px, 0.06 + (px * 0.7).sin() * (pz * 0.7).cos() * 0.04, pz]);
        uvs.push([x as f32 / 20.0, z as f32 / 20.0]);
    }}
    for z in 0..20 { for x in 0..20 {
        let a = z * 21 + x; let b = a + 21;
        indices.extend([a, b, a + 1, a + 1, b, b + 1]);
    }}
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 441]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices)); mesh
}

pub fn bounds() -> Aabb { Aabb::from_min_max(Vec3::new(-2.0, 0.01, -2.0), Vec3::new(2.0, 0.11, 2.0)) }

pub fn camera(world: &mut World) -> Entity {
    let pose = Transform::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::new(0.0, 0.06, 0.0), Vec3::Y);
    world.spawn((Camera3d::default(), Camera { computed: ComputedCameraValues {
        target_info: Some(RenderTargetInfo { physical_size: UVec2::new(978, 288), scale_factor: 1.0 }),
        ..default() }, ..default() }, Projection::Perspective(PerspectiveProjection::default()),
        pose, GlobalTransform::from(pose))).id()
}

fn digest(bytes: &[u8]) -> String { Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect() }

fn geometry_sha(mesh: &Mesh) -> String {
    let mut bytes = Vec::new();
    for attr in [Mesh::ATTRIBUTE_POSITION, Mesh::ATTRIBUTE_NORMAL, Mesh::ATTRIBUTE_UV_0] {
        match mesh.attribute(attr).unwrap() {
            VertexAttributeValues::Float32x3(values) => for v in values { for f in v { bytes.extend(f.to_le_bytes()); } },
            VertexAttributeValues::Float32x2(values) => for v in values { for f in v { bytes.extend(f.to_le_bytes()); } },
            _ => panic!("共同LOD夹具属性类型不符"),
        }
    }
    for index in mesh.indices().unwrap().iter() { bytes.extend((index as u32).to_le_bytes()); }
    digest(&bytes)
}

pub fn sample(world: &World, entity: Entity, original: &Handle<Mesh>, phase: &str) -> Value {
    let displayed = &world.get::<Mesh3d>(entity).unwrap().0;
    let meshes = world.resource::<Assets<Mesh>>();
    let source = meshes.get(original).unwrap(); let shown = meshes.get(displayed).unwrap();
    json!({"phase":phase,"original_handle": displayed == original,
        "source_triangles":source.indices().unwrap().len()/3,"displayed_triangles":shown.indices().unwrap().len()/3,
        "source_geometry_sha256":geometry_sha(source),"displayed_geometry_sha256":geometry_sha(shown)})
}

pub fn report(candidate: &str, rows: Vec<Value>) {
    let report = json!({"schema":1,"candidate":candidate,"fixture_sha256":digest(include_bytes!("editor-lod-parity-v1.rs")),
        "scope":"生产LOD主世界状态机；相同网格/相机/误差。作者模式走各自正式开关，不包括窗口输入、GPU或Present耗时。",
        "viewport":[978,288],"max_error_pixels":16.0,"hysteresis_ratio":0.15,"phases":rows});
    if let Some(path) = std::env::var_os("EDITOR_LOD_PARITY_REPORT") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).unwrap();
        file.write_all(serde_json::to_string_pretty(&report).unwrap().as_bytes()).unwrap();
    }
}
