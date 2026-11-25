//! 内置参数化几何的有界弱缓存和唯一可视子节点；正式候选与F12作者预览共用此入口。

use std::collections::{HashMap, VecDeque};

use avian3d::prelude::{Collider, ColliderDisabled, RigidBodyDisabled};
use bevy::{asset::{AssetId, RenderAssetUsages}, camera::primitives::{Aabb, MeshAabb}, mesh::{Indices, MeshVertexAttribute, PrimitiveTopology}, prelude::*, render::render_resource::VertexFormat};
use feathrise_scene_content::{EditableMeshGeometry, ScenePlacementAssetTarget, EDITABLE_CUBOID_TRIANGLES};

use super::{ScenePlacementInstance, ScenePlacementPrimitiveVisual};
use super::components::{BuiltinPlacementVisual, BuiltinPlacementVisualRoot};

mod preparation;
#[cfg(test)]
mod preparation_tests;
pub use preparation::BuiltinVisualPrepared;
pub(super) use preparation::install_preparation;

/// 声明UV1承载作者真实边编码；私有固定ID排在标准属性之后，不改变原属性偏移或正式Shader输入。
/// 额外Float32使完整顶点步长增加4字节，避免与普通UV1网格共用源管线/多重绘制批次；无需逐实体提取标记。
pub const ATTRIBUTE_AUTHORING_WIRE: MeshVertexAttribute = MeshVertexAttribute::new(
    "AuthoringWire", 0x4155_5448_5749_5245, VertexFormat::Float32,
);

const BUILTIN_CACHE_ENTRIES: usize = 256; // 缓存只保留资产ID；尺寸连续拖动不会无限强持有旧网格或材质。

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum BuiltinGeometryKey {
    Cuboid([u32; 3]),
    Editable([[i32; 3]; 8]),
    Mesh(EditableMeshGeometry),
}

#[derive(Resource, Default)]
struct BuiltinPrimitiveCache {
    meshes: HashMap<BuiltinGeometryKey, AssetId<Mesh>>,
    mesh_order: VecDeque<BuiltinGeometryKey>,
    materials: HashMap<([u8; 3], u16, bool), AssetId<StandardMaterial>>,
    material_order: VecDeque<([u8; 3], u16, bool)>,
}

/// 正式显示与编辑幽灵共用十二三角及逐三角法线，非平面四边面不会假装成单一平面。
/// 输入必须先通过目标validate；固定36顶点和36索引的构造预算与场景规模无关。
pub fn editable_cuboid_mesh(vertices_half_mm: [[i32; 3]; 8]) -> Mesh {
    editable_mesh_mesh(EditableMeshGeometry::from_cuboid_vertices(vertices_half_mm))
}

/// 显示与碰撞消费同一份已经验证并缓存的作者三角。大网格调用方在后台构造。
pub fn editable_mesh_mesh(geometry: EditableMeshGeometry) -> Mesh {
    let vertices = geometry.vertices().iter().map(|point| Vec3::from_array(point.map(|value| value as f32 * 0.0005))).collect::<Vec<_>>();
    let triangles = geometry.triangles_with_faces();
    let mut positions = Vec::with_capacity(triangles.len() * 3);
    let mut normals = Vec::with_capacity(triangles.len() * 3);
    let mut uv = Vec::with_capacity(triangles.len() * 3);
    let mut authoring_wire = Vec::with_capacity(triangles.len() * 3);
    for (face_index, triangle) in triangles {
        let [a, b, c] = triangle.map(|index| vertices[index]);
        let normal = (b - a).cross(c - a).normalize_or_zero().to_array();
        positions.extend([a.to_array(), b.to_array(), c.to_array()]);
        normals.extend([normal; 3]);
        uv.extend([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        // UV1只供作者线框使用：bit i对应顶点i的对边；必须查所属面的边界，不能把三角化对角线当真边。
        // 同一三角三点保存相同mask*4偏移，余下xy是重心坐标，z由1-x-y还原；原UV0保持不变。
        let face = geometry.faces()[*face_index].indices();
        let mut mask = 0_u8;
        for corner in 0..3 {
            let edge = [triangle[(corner + 1) % 3], triangle[(corner + 2) % 3]];
            if (0..face.len()).any(|side| {
                let boundary = [face[side] as usize, face[(side + 1) % face.len()] as usize];
                boundary == edge || boundary == [edge[1], edge[0]]
            }) {
                mask |= 1 << corner;
            }
        }
        let packed_mask = f32::from(mask) * 4.0;
        authoring_wire.extend([[packed_mask + 1.0, 0.0], [packed_mask, 1.0], [packed_mask, 0.0]]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, authoring_wire);
    mesh.insert_attribute(ATTRIBUTE_AUTHORING_WIRE, vec![0.0_f32; triangles.len() * 3]); // 只占布局身份；实际边界数据仍在UV1。
    mesh.insert_indices(Indices::U32((0..(triangles.len() * 3) as u32).collect()));
    // 使用共享作者顶点预计算范围，避免Bevy新旧显示句柄交接时再扫描每个重复三角顶点。
    let (min, max) = geometry.bounds_half_mm();
    let [min, max] = [min, max].map(|point| Vec3::from_array(point.map(|value| value as f32 * 0.0005)));
    mesh.final_aabb = Some(Aabb::from_min_max(min, max).into());
    mesh
}

/// 参数立方体改用作者三角源后，仍保留Bevy Cuboid六个面的原UV0朝向；不改变已编辑网格的历史UV0。
fn parameter_cuboid_uv0(geometry: &EditableMeshGeometry) -> Vec<[f32; 2]> {
    geometry.triangles_with_faces().iter().flat_map(|&(face, triangle)| {
        triangle.map(|vertex| {
            let [x, y, z] = [(vertex & 1) as f32, ((vertex >> 1) & 1) as f32, ((vertex >> 2) & 1) as f32];
            match face {
                0 => [1.0 - y, 1.0 - z], // -X
                1 => [y, z],             // +X
                2 => [1.0 - x, 1.0 - z], // -Y
                3 => [x, z],             // +Y
                4 => [1.0 - x, 1.0 - y], // -Z
                5 => [x, y],             // +Z
                _ => unreachable!("参数立方体只有六个固定作者面"),
            }
        })
    }).collect()
}

/// 为内置目标创建或更新唯一可视子节点；返回false表示调用方仍需处理外部GLB目标。
///
/// 根必须先持有匹配的ScenePlacementInstance。正式候选的所有内置碰撞归隐藏容器的禁用静态刚体，
/// F12独立预览则由调用方在当前根安装Static刚体；本入口绝不擅自激活玩法碰撞。
pub fn spawn_builtin_placement_visual(
    commands: &mut Commands,
    root: Entity,
    target: ScenePlacementAssetTarget,
) -> bool {
    if let Some((rgb, candela, range, cone)) = target.local_light_parameters() {
        let color = Color::linear_rgb(rgb[0], rgb[1], rgb[2]);
        // 与 Bevy glTF 加载器相同，将坎德拉换算成流明；没有声明范围的 glTF 默认照射 20 米。
        let intensity = candela * std::f32::consts::PI * 4.0;
        let range = range.unwrap_or(20.0);
        let mut light = commands.entity(root);
        if let Some([inner_angle, outer_angle]) = cone {
            light.remove::<PointLight>().insert(SpotLight { color, intensity, range, inner_angle, outer_angle, ..default() });
        } else {
            light.remove::<SpotLight>().insert(PointLight { color, intensity, range, ..default() });
        }
        return true;
    }
    let (geometry, color_srgb, roughness_milli, collision) = match &target {
        ScenePlacementAssetTarget::Cuboid { size_mm, color_srgb, roughness_milli, collision } =>
            (BuiltinGeometryKey::Cuboid(*size_mm), *color_srgb, *roughness_milli, *collision),
        ScenePlacementAssetTarget::EditableCuboid { vertices_half_mm, color_srgb, roughness_milli, collision } =>
            (BuiltinGeometryKey::Editable(*vertices_half_mm), *color_srgb, *roughness_milli, *collision),
        ScenePlacementAssetTarget::EditableMesh { geometry, color_srgb, roughness_milli, collision } =>
            (BuiltinGeometryKey::Mesh(geometry.clone()), *color_srgb, *roughness_milli, *collision),
        _ => return false,
    };
    let open_surface = matches!(&geometry, BuiltinGeometryKey::Mesh(geometry) if geometry.is_open());
    commands.queue(move |world: &mut World| {
        preparation::request(world, root, preparation::Request {
            target, geometry, material: (color_srgb, roughness_milli, open_surface), collision,
        });
    });
    true
}

#[derive(Component)]
struct BuiltinVisualSource(ScenePlacementAssetTarget);

/// 完成的子节点必须匹配当前作者快照；旧画面可继续显示，但不会给新的保存或预览交接授信。
pub fn builtin_placement_visual_ready(world: &World, root: Entity) -> bool {
    let Some(instance) = world.get::<ScenePlacementInstance>(root) else { return false; };
    if let Some((_, _, _, cone)) = instance.asset_target().local_light_parameters() {
        return (instance.is_active() || instance.is_pending()) && if cone.is_some() {
            world.get::<SpotLight>(root).is_some()
        } else { world.get::<PointLight>(root).is_some() };
    }
    let Some(visual) = builtin_visual_for_root(world, root) else { return false; };
    let target = instance.asset_target();
    let collision = match &target {
        ScenePlacementAssetTarget::Cuboid { collision, .. }
        | ScenePlacementAssetTarget::EditableCuboid { collision, .. }
        | ScenePlacementAssetTarget::EditableMesh { collision, .. } => *collision,
        _ => return false,
    };
    // 作者编辑交接要认领当前摆放自己的碰撞，运动轨道使用独立的盒。
    let collision_ready = world.get::<super::motion::MotionPlatform>(root).map_or_else(
        || !collision || world.get::<Collider>(visual).is_some(),
        |platform| platform.collider_ready(world, root),
    );
    (instance.is_active() || instance.is_pending())
        && world.get::<BuiltinVisualSource>(visual).is_some_and(|source| source.0 == target)
        && world.get::<Aabb>(visual).is_some()
        && world.get::<Mesh3d>(visual).is_some()
        && world.get::<MeshMaterial3d<StandardMaterial>>(visual).is_some()
        && collision_ready
}

/// 单节点地址仅表达查找，不授予他人子树写入权；重挂、删除或去标记后立即失败关闭。
fn builtin_visual_for_root(world: &World, root: Entity) -> Option<Entity> {
    let visual = world.get::<BuiltinPlacementVisualRoot>(root)?.0;
    (world.get::<BuiltinPlacementVisual>(visual).is_some()
        && world.get::<ChildOf>(visual).is_some_and(|parent| parent.parent() == root)).then_some(visual)
}

/// 编辑器临时隐藏、删除与恢复只切换内置根的直接盒碰撞，不影响正式批次共享刚体。
pub fn set_builtin_placement_collision_enabled(commands: &mut Commands, root: Entity, enabled: bool) {
    commands.queue(move |world: &mut World| {
        set_builtin_placement_collision_enabled_world(world, root, enabled);
    });
}

/// 正式原子交接可在撤销根身份前同步停用唯一子碰撞；不依赖根自身拥有RigidBody。
/// 调用后再移除ScenePlacementInstance，迟到的启用命令会因缺少当前身份而被拒绝。
pub fn set_builtin_placement_collision_enabled_world(world: &mut World, root: Entity, enabled: bool) {
    if world.get::<ScenePlacementInstance>(root).is_none_or(|instance| !instance.asset_target().is_builtin()) {
        return;
    }
    let Some(visual) = builtin_visual_for_root(world, root) else { return; };
    // 遮蔽可能在调用方排队后才取得责任；旧启用意图必须服从执行时的最终显隐。
    if enabled && world.get::<Visibility>(root) != Some(&Visibility::Hidden) { world.entity_mut(visual).remove::<ColliderDisabled>(); }
    else { world.entity_mut(visual).insert(ColliderDisabled); }
}

/// 显式候选或编辑器整组预览共用同一个物理/角色探测开关，父容器无需伪装成摆放实例。
pub fn set_builtin_placement_batch_enabled(commands: &mut Commands, root: Entity, enabled: bool) {
    commands.queue(move |world: &mut World| {
        set_builtin_placement_batch_enabled_world(world, root, enabled);
    });
}

/// 原子交接在释放正式根前同步停用作者自己的刚体及角色承托；不借父层级关闭共享批次。
pub fn set_builtin_placement_batch_enabled_world(world: &mut World, root: Entity, enabled: bool) {
    let Ok(mut root) = world.get_entity_mut(root) else { return; }; // 跨帧删除后的旧地址不能使整次清理panic。
    if enabled {
        root.remove::<(RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)>();
    } else {
        root.insert((RigidBodyDisabled, bevy_tnua::TnuaNotPlatform));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use avian3d::prelude::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::mesh::VertexAttributeValues;
    use feathrise_scene_content::{SceneId, ScenePlacementRecord, MeshFace};

    fn record(target: ScenePlacementAssetTarget) -> ScenePlacementRecord {
        serde_json::from_value(serde_json::json!({
            "id":"builtin-test", "asset_path":"", "asset_target":target,
            "translation":[0,0,0], "yaw_degrees":0, "scale":[1,1,1]
        })).unwrap()
    }

    fn install(world: &mut World, root: Entity, target: ScenePlacementAssetTarget) {
        world.run_system_once(move |mut commands: Commands| {
            assert!(spawn_builtin_placement_visual(&mut commands, root, target.clone()));
        }).unwrap();
    }

    fn spawn_root(world: &mut World, target: ScenePlacementAssetTarget) -> Entity {
        let record = record(target.clone());
        world.spawn((ScenePlacementInstance::new(&SceneId::parse("MAP-builtin").unwrap(), &record),
            Transform::IDENTITY, Visibility::Inherited)).id()
    }

    #[test]
    fn local_lights_use_gltf_photometry_on_the_transformed_placement_without_mesh_or_collider() {
        let mut world = World::new();
        for cone in [None, Some([0.2, 0.7])] {
            let target = ScenePlacementAssetTarget::from_local_light([0.8, 0.6, 0.4], 550.0, Some(15.0), cone).unwrap();
            let root = spawn_root(&mut world, target.clone());
            install(&mut world, root, target);
            let (color, intensity, range) = if let Some([inner, outer]) = cone {
                let spot = world.get::<SpotLight>(root).unwrap();
                assert_eq!([spot.inner_angle, spot.outer_angle], [inner, outer]);
                (spot.color, spot.intensity, spot.range)
            } else {
                let point = world.get::<PointLight>(root).unwrap();
                (point.color, point.intensity, point.range)
            };
            assert_eq!(color, Color::linear_rgb(0.8, 0.6, 0.4));
            assert!((intensity - 550.0 * 4.0 * std::f32::consts::PI).abs() < 0.01);
            assert_eq!(range, 15.0);
            assert!(builtin_placement_visual_ready(&world, root));
            assert!(world.get::<Collider>(root).is_none() && world.get::<Mesh3d>(root).is_none());
        }
    }

    fn active_ground_hit(query: SpatialQuery, active: feathrise_grounding::GroundSurfaceFilter) -> Option<Entity> {
        query.cast_ray_predicate(Vec3::new(0.0, 10.0, 0.0), Dir3::NEG_Y, 20.0, true,
            &SpatialQueryFilter::default(), &|entity| active.is_active(entity)).map(|hit| hit.entity)
    }

    fn ordered_edge([a, b]: [usize; 2]) -> [usize; 2] {
        if a < b { [a, b] } else { [b, a] }
    }

    /// 按Shader契约解码UV1，而非通过生产侧mask计算函数推断结果。
    fn decoded_wire_edges_by_face(geometry: EditableMeshGeometry, mesh: &Mesh) -> Vec<BTreeSet<[usize; 2]>> {
        let VertexAttributeValues::Float32x2(packed) = mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap() else { panic!("作者线框需要UV1") };
        let triangles = geometry.triangles_with_faces();
        assert_eq!(packed.len(), triangles.len() * 3);
        let mut edges = vec![BTreeSet::new(); geometry.faces().len()];
        for (index, (face, triangle)) in triangles.iter().copied().enumerate() {
            let mask = (packed[index * 3][0] / 4.0).floor() as u8;
            assert!(mask <= 7);
            let offset = f32::from(mask) * 4.0;
            assert_eq!(&packed[index * 3..index * 3 + 3], &[[offset + 1.0, 0.0], [offset, 1.0], [offset, 0.0]]);
            for corner in 0..3 {
                if mask & (1 << corner) != 0 {
                    edges[face].insert(ordered_edge([triangle[(corner + 1) % 3], triangle[(corner + 2) % 3]]));
                }
            }
        }
        edges
    }

    fn assert_only_face_boundary_edges(geometry: EditableMeshGeometry, mesh: &Mesh) -> BTreeSet<[usize; 2]> {
        let decoded = decoded_wire_edges_by_face(geometry.clone(), mesh);
        for (face, actual) in geometry.faces().iter().zip(&decoded) {
            let boundary = face.indices();
            let expected = boundary.iter().zip(boundary.iter().cycle().skip(1)).take(boundary.len())
                .map(|(a, b)| ordered_edge([*a as usize, *b as usize])).collect::<BTreeSet<_>>();
            assert_eq!(*actual, expected, "只能暴露本作者面的边界，不能显示其耳切或四边面对角线");
        }
        decoded.into_iter().flatten().collect()
    }

    #[test]
    fn editable_wire_metadata_keeps_twelve_cuboid_edges_and_hides_quad_diagonals() {
        let geometry = ScenePlacementAssetTarget::DEFAULT_CUBOID.editable_geometry().unwrap();
        let mesh = editable_mesh_mesh(geometry.clone());
        assert_eq!(geometry.vertices().len(), 8);
        let edges = assert_only_face_boundary_edges(geometry.clone(), &mesh);
        assert_eq!(edges.len(), 12);
        assert_eq!(edges, geometry.edges().iter().copied().map(ordered_edge).collect::<BTreeSet<_>>());
        let VertexAttributeValues::Float32x2(uv1) = mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap() else { unreachable!() };
        for pair in uv1.chunks_exact(6) {
            assert_eq!((pair[0][0] / 4.0).floor() as u8, 0b101, "首三角顶点1的对边是0到2对角线");
            assert_eq!((pair[3][0] / 4.0).floor() as u8, 0b011, "次三角顶点2的对边是同一对角线");
        }
        let VertexAttributeValues::Float32x2(uv0) = mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() else { unreachable!() };
        assert!(uv0.chunks_exact(3).all(|triangle| triangle == [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]));
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_none());
    }

    #[test]
    fn editable_wire_metadata_uses_concave_polygon_boundaries_instead_of_ear_diagonals() {
        let boundary = [[-2_000, -2_000], [-2_000, 2_000], [2_000, 2_000], [2_000, 0], [0, 0], [0, -2_000]];
        let mut vertices = Vec::new();
        for y in [-2_000, 2_000] {
            vertices.extend(boundary.map(|[x, z]| [x, y, z]));
        }
        let mut faces = vec![MeshFace::new(&[5, 4, 3, 2, 1, 0]).unwrap(), MeshFace::new(&[6, 7, 8, 9, 10, 11]).unwrap()];
        for side in 0..6_u32 {
            let next = (side + 1) % 6;
            faces.push(MeshFace::new(&[side, next, next + 6, side + 6]).unwrap());
        }
        let geometry = EditableMeshGeometry::from_parts(&vertices, &faces).unwrap();
        assert_eq!(geometry.triangles().len(), 20);
        let edges = assert_only_face_boundary_edges(geometry.clone(), &editable_mesh_mesh(geometry.clone()));
        assert_eq!(edges.len(), 18);
        assert_eq!(edges, geometry.edges().iter().copied().map(ordered_edge).collect::<BTreeSet<_>>());
    }

    #[test]
    fn builtin_wire_mesh_keeps_parameter_uv0_material_and_box_collision() {
        let target = ScenePlacementAssetTarget::Cuboid { size_mm: [1_000, 2_000, 3_000], color_srgb: [10, 20, 30], roughness_milli: 400, collision: true };
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let root = spawn_root(&mut world, target.clone());
        install(&mut world, root, target.clone());
        assert_eq!(world.get::<Children>(root).unwrap().len(), 1, "线框不能另建边实体");
        let visual = world.get::<Children>(root).unwrap()[0];
        let mesh = world.resource::<Assets<Mesh>>().get(&world.get::<Mesh3d>(visual).unwrap().0).unwrap();
        assert!(mesh.attribute(ATTRIBUTE_AUTHORING_WIRE).is_some());
        let original: Mesh = Cuboid::from_size(Vec3::new(1.0, 2.0, 3.0)).into();
        let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { unreachable!() };
        let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() else { unreachable!() };
        let VertexAttributeValues::Float32x2(uv0) = mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() else { unreachable!() };
        let VertexAttributeValues::Float32x3(old_positions) = original.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { unreachable!() };
        let VertexAttributeValues::Float32x3(old_normals) = original.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() else { unreachable!() };
        let VertexAttributeValues::Float32x2(old_uv0) = original.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() else { unreachable!() };
        assert_eq!(positions.len(), 36);
        for (index, position) in positions.iter().enumerate() {
            let old = old_positions.iter().enumerate().position(|(old, point)| {
                Vec3::from_array(*point).distance(Vec3::from_array(*position)) < 1e-6 && old_normals[old] == normals[index]
            }).unwrap(); // 半毫米转米与Bevy全尺寸除二的f32舍入可以相差一个ULP，不能误判为不同顶点。
            assert_eq!(uv0[index], old_uv0[old], "原参数配方每个面的UV朝向必须保留");
        }
        assert_eq!(assert_only_face_boundary_edges(target.editable_geometry().unwrap(), mesh).len(), 12);
        assert_eq!(world.get::<Collider>(visual).unwrap().shape().as_cuboid().unwrap().half_extents.to_array(), [0.5, 1.0, 1.5]);
        let material = world.resource::<Assets<StandardMaterial>>().get(&world.get::<MeshMaterial3d<StandardMaterial>>(visual).unwrap().0).unwrap();
        assert_eq!(material.base_color, Color::srgb_u8(10, 20, 30));
        assert_eq!(material.perceptual_roughness, 0.4);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_none());
    }

    #[test]
    fn builtin_wire_cache_distinguishes_author_face_topology_with_identical_triangles() {
        let quads = ScenePlacementAssetTarget::DEFAULT_CUBOID.editable_geometry().unwrap();
        let triangles = quads.triangles().into_iter().map(|triangle| MeshFace::new(&triangle.map(|vertex| vertex as u32)).unwrap()).collect::<Vec<_>>();
        let split = EditableMeshGeometry::from_parts(quads.vertices(), &triangles).unwrap();
        assert_eq!(quads.triangles(), split.triangles());
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let mut handles = Vec::new();
        for geometry in [quads.clone(), quads.clone(), split.clone(), split.clone()] {
            let target = ScenePlacementAssetTarget::EditableMesh { geometry, color_srgb: [160; 3], roughness_milli: 700, collision: false };
            let root = spawn_root(&mut world, target.clone());
            install(&mut world, root, target.clone());
            let visual = world.get::<Children>(root).unwrap()[0];
            handles.push(world.get::<Mesh3d>(visual).unwrap().0.clone());
        }
        assert_eq!(handles[0], handles[1]);
        assert_eq!(handles[2], handles[3]);
        assert_ne!(handles[0], handles[2], "相同坐标和三角但作者面不同，不能错用另一套真实边mask");
        let meshes = world.resource::<Assets<Mesh>>();
        let (quad_mesh, split_mesh) = (meshes.get(&handles[0]).unwrap(), meshes.get(&handles[2]).unwrap());
        for attribute in [Mesh::ATTRIBUTE_POSITION, Mesh::ATTRIBUTE_NORMAL, Mesh::ATTRIBUTE_UV_0] {
            assert_eq!(quad_mesh.attribute(attribute), split_mesh.attribute(attribute));
        }
        assert_eq!(assert_only_face_boundary_edges(quads, quad_mesh).len(), 12);
        assert_eq!(assert_only_face_boundary_edges(split, split_mesh).len(), 18, "显式拆为作者三角面后，对角线才成为真边");
        assert_eq!(world.resource::<BuiltinPrimitiveCache>().meshes.len(), 2);
        assert_eq!(world.resource::<BuiltinPrimitiveCache>().materials.len(), 1);
    }

    #[test]
    fn authoring_wire_layout_stays_distinct_from_plain_uv1_meshes_with_or_without_colors() {
        let geometry = ScenePlacementAssetTarget::DEFAULT_CUBOID.editable_geometry().unwrap();
        for with_colors in [false, true] {
            let mut authoring = editable_mesh_mesh(geometry.clone());
            if with_colors {
                authoring.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0_f32; 4]; 36]);
            } // 幽灵可额外带原顶点颜色，仍需与相同标准属性集合的普通网格隔离。
            let VertexAttributeValues::Float32(marker) = authoring.attribute(ATTRIBUTE_AUTHORING_WIRE).unwrap() else { panic!("布局标记必须是单精度标量") };
            assert_eq!(marker.as_slice(), &[0.0; 36]);
            let mut plain = authoring.clone();
            plain.remove_attribute(ATTRIBUTE_AUTHORING_WIRE).unwrap();
            let mut layouts = bevy::mesh::MeshVertexBufferLayouts::default();
            let authoring_layout = authoring.get_mesh_vertex_buffer_layout(&mut layouts);
            let plain_layout = plain.get_mesh_vertex_buffer_layout(&mut layouts);
            assert!(authoring_layout.0.contains(ATTRIBUTE_AUTHORING_WIRE));
            assert!(!plain_layout.0.contains(ATTRIBUTE_AUTHORING_WIRE));
            let mut inputs = vec![Mesh::ATTRIBUTE_POSITION.at_shader_location(0), Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
                Mesh::ATTRIBUTE_UV_0.at_shader_location(2), Mesh::ATTRIBUTE_UV_1.at_shader_location(3)];
            if with_colors { inputs.push(Mesh::ATTRIBUTE_COLOR.at_shader_location(4)); }
            let authoring_pipeline_layout = authoring_layout.0.get_layout(&inputs).unwrap();
            let plain_pipeline_layout = plain_layout.0.get_layout(&inputs).unwrap();
            assert_eq!(authoring_pipeline_layout.attributes, plain_pipeline_layout.attributes, "原属性偏移和Shader输入必须保留");
            assert_eq!(authoring_pipeline_layout.array_stride, plain_pipeline_layout.array_stride + 4);
            assert_eq!(authoring_pipeline_layout.array_stride, if with_colors { 60 } else { 44 });
            assert_ne!(authoring_pipeline_layout, plain_pipeline_layout,
                "Bevy会按最终VertexBufferLayout去重，不能仅靠未被Shader读取的属性ID隔离多重绘制批次");
        }
    }

    #[test]
    fn builtin_visual_reuses_assets_updates_in_place_and_honors_collision_toggle() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        let first = spawn_root(&mut world, target.clone());
        let second = spawn_root(&mut world, target.clone());
        install(&mut world, first, target.clone());
        install(&mut world, second, target.clone());
        let first_visual = world.get::<Children>(first).unwrap()[0];
        let second_visual = world.get::<Children>(second).unwrap()[0];
        assert_eq!(world.get::<Mesh3d>(first_visual).unwrap().0, world.get::<Mesh3d>(second_visual).unwrap().0);
        assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(first_visual).unwrap().0, world.get::<MeshMaterial3d<StandardMaterial>>(second_visual).unwrap().0);
        assert!(world.get::<Collider>(first_visual).is_some());
        assert!(world.get::<ScenePlacementPrimitiveVisual>(first_visual).is_some());
        world.run_system_once(move |mut commands: Commands| set_builtin_placement_collision_enabled(&mut commands, first, false)).unwrap();
        assert!(world.get::<ColliderDisabled>(first_visual).is_some());
        world.run_system_once(move |mut commands: Commands| set_builtin_placement_collision_enabled(&mut commands, first, true)).unwrap();
        assert!(world.get::<ColliderDisabled>(first_visual).is_none());

        let changed = ScenePlacementAssetTarget::Cuboid { size_mm: [1_000, 2_000, 3_000], color_srgb: [10, 20, 30], roughness_milli: 400, collision: false };
        let record = record(changed.clone());
        world.entity_mut(first).insert(ScenePlacementInstance::new(&SceneId::parse("MAP-builtin").unwrap(), &record));
        install(&mut world, first, changed.clone());
        assert_eq!(world.get::<Children>(first).unwrap().len(), 1);
        assert!(world.get_entity(first_visual).is_err());
        let changed_visual = world.get::<Children>(first).unwrap()[0];
        assert!(world.get::<Collider>(changed_visual).is_none());
        assert_ne!(world.get::<Mesh3d>(changed_visual).unwrap().0, world.get::<Mesh3d>(second_visual).unwrap().0);
    }

    #[test]
    fn builtin_visual_rejects_stale_target_and_keeps_hidden_roots_non_colliding() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        let root = spawn_root(&mut world, target.clone());
        let stale = ScenePlacementAssetTarget::Cuboid { size_mm: [1_000; 3], color_srgb: [160; 3], roughness_milli: 700, collision: true };
        install(&mut world, root, stale.clone());
        assert!(world.get::<Children>(root).is_none());
        world.entity_mut(root).insert(Visibility::Hidden);
        install(&mut world, root, target.clone());
        let visual = world.get::<Children>(root).unwrap()[0];
        assert!(world.get::<ColliderDisabled>(visual).is_some());
    }

    #[test]
    fn builtin_visual_replacement_preserves_wide_author_children_and_only_switches_its_registered_collider() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        let root = spawn_root(&mut world, target.clone());
        install(&mut world, root, target.clone());
        let old_visual = world.get::<BuiltinPlacementVisualRoot>(root).unwrap().0;
        let author_children = (0..1_024).map(|_| world.spawn(ChildOf(root)).id()).collect::<Vec<_>>();
        let author_collider = world.spawn((ChildOf(root), Collider::sphere(0.25))).id();
        let author_leaf = world.spawn(ChildOf(author_children[0])).id();
        let changed = ScenePlacementAssetTarget::Cuboid { size_mm: [1_000, 2_000, 3_000],
            color_srgb: [10, 20, 30], roughness_milli: 400, collision: true };
        world.entity_mut(root).insert(ScenePlacementInstance::new(&SceneId::parse("MAP-builtin").unwrap(), &record(changed.clone())));
        install(&mut world, root, changed.clone());
        let visual = world.get::<BuiltinPlacementVisualRoot>(root).unwrap().0;
        assert_ne!(visual, old_visual);
        assert!(world.get_entity(old_visual).is_err(), "尺寸变化只替换登记的旧唯一网格");
        assert_eq!(world.get::<Children>(root).unwrap().len(), author_children.len() + 2);
        for child in author_children {
            assert_eq!(world.get::<ChildOf>(child).map(ChildOf::parent), Some(root));
        }
        assert!(world.get_entity(author_leaf).is_ok());
        set_builtin_placement_collision_enabled_world(&mut world, root, false);
        assert!(world.get::<ColliderDisabled>(visual).is_some());
        assert!(world.get::<ColliderDisabled>(author_collider).is_none(),
            "附加作者子树和它的碰撞不属于内置唯一网格，不得因同父关系被批量切换");
    }

    #[test]
    fn builtin_world_collision_switch_is_synchronous_for_shared_body_preview_and_stays_off_after_identity_removed() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        let batch = world.spawn((Transform::IDENTITY, RigidBody::Static)).id();
        let root = spawn_root(&mut world, target.clone());
        let sibling = spawn_root(&mut world, target.clone());
        world.entity_mut(root).insert(ChildOf(batch));
        world.entity_mut(sibling).insert(ChildOf(batch));
        install(&mut world, root, target.clone());
        install(&mut world, sibling, target.clone());
        let visual = world.get::<BuiltinPlacementVisualRoot>(root).unwrap().0;
        let sibling_visual = world.get::<BuiltinPlacementVisualRoot>(sibling).unwrap().0;
        assert!(world.get::<RigidBody>(root).is_none(), "粘贴预览组的子根可共用上层刚体");
        set_builtin_placement_collision_enabled_world(&mut world, root, false);
        set_builtin_placement_batch_enabled_world(&mut world, root, false);
        assert!(world.get::<ColliderDisabled>(visual).is_some(), "返回前就必须停用子碰撞，不能等待下一次命令刷新");
        assert!(world.get::<bevy_tnua::TnuaNotPlatform>(root).is_some());
        assert!(world.get::<ColliderDisabled>(sibling_visual).is_none());
        assert!(world.get::<RigidBodyDisabled>(batch).is_none(), "单根回收不能关闭共享刚体上的其他作者对象");
        assert!(world.get::<bevy_tnua::TnuaNotPlatform>(batch).is_none());
        world.entity_mut(root).remove::<ScenePlacementInstance>();
        set_builtin_placement_collision_enabled_world(&mut world, root, true);
        world.run_system_once(move |mut commands: Commands| {
            set_builtin_placement_collision_enabled(&mut commands, root, true);
        }).unwrap();
        assert!(world.get::<ColliderDisabled>(visual).is_some(), "撤销根身份后的迟到启用不能复活跨帧清理中的旧碰撞");
    }

    #[test]
    fn queued_builtin_collision_enable_obeys_later_root_mask_without_switching_the_shared_body() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        let batch = world.spawn((Transform::IDENTITY, RigidBody::Static)).id();
        let root = spawn_root(&mut world, target.clone());
        let sibling = spawn_root(&mut world, target.clone());
        world.entity_mut(root).insert(ChildOf(batch));
        world.entity_mut(sibling).insert(ChildOf(batch));
        install(&mut world, root, target.clone());
        install(&mut world, sibling, target.clone());
        let visual = world.get::<BuiltinPlacementVisualRoot>(root).unwrap().0;
        let sibling_visual = world.get::<BuiltinPlacementVisualRoot>(sibling).unwrap().0;
        set_builtin_placement_collision_enabled_world(&mut world, root, false);
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            set_builtin_placement_collision_enabled(&mut commands, root, true);
        }
        // 排队时可见、执行前才被删除/作者覆盖；旧启用命令必须重新消费最终显隐。
        world.entity_mut(root).insert(Visibility::Hidden);
        queue.apply(&mut world);
        assert!(world.get::<ColliderDisabled>(visual).is_some());
        assert!(world.get::<ColliderDisabled>(sibling_visual).is_none());
        assert!(world.get::<RigidBodyDisabled>(batch).is_none());
        assert!(world.get::<bevy_tnua::TnuaNotPlatform>(batch).is_none());
        assert_eq!(world.get::<ChildOf>(sibling).unwrap().parent(), batch);
        world.entity_mut(root).insert(Visibility::Inherited);
        set_builtin_placement_collision_enabled_world(&mut world, root, true);
        assert!(world.get::<ColliderDisabled>(visual).is_none(), "正式解除覆盖后新启用意图仍可正常生效");
    }

    #[test]
    fn builtin_world_batch_switch_is_synchronous_keeps_other_bodies_active_and_ignores_removed_entities() {
        let mut world = World::new();
        let parent = world.spawn(RigidBody::Static).id();
        let root = world.spawn((RigidBody::Static, ChildOf(parent))).id();
        let sibling = world.spawn((RigidBody::Static, ChildOf(parent))).id();
        set_builtin_placement_batch_enabled_world(&mut world, root, false);
        assert!(world.get::<RigidBodyDisabled>(root).is_some());
        assert!(world.get::<bevy_tnua::TnuaNotPlatform>(root).is_some(),
            "角色承托门必须与物理门在返回前同时生效，不能等延迟命令");
        for other in [parent, sibling] {
            assert!(world.get::<RigidBodyDisabled>(other).is_none());
            assert!(world.get::<bevy_tnua::TnuaNotPlatform>(other).is_none());
        }
        set_builtin_placement_batch_enabled_world(&mut world, root, true);
        assert!(world.get::<RigidBodyDisabled>(root).is_none());
        assert!(world.get::<bevy_tnua::TnuaNotPlatform>(root).is_none());
        world.despawn(root);
        set_builtin_placement_batch_enabled_world(&mut world, root, false);
        world.run_system_once(move |mut commands: Commands| {
            set_builtin_placement_batch_enabled(&mut commands, root, true);
        }).unwrap(); // 旧延迟地址与同步地址都安静拒绝，不波及仍活动的父和兄弟。
        assert!(world.get::<RigidBodyDisabled>(parent).is_none());
        assert!(world.get::<RigidBodyDisabled>(sibling).is_none());
    }

    #[test]
    fn stale_builtin_visual_address_cannot_modify_or_despawn_a_reparented_or_unmarked_child() {
        for reparented in [false, true] {
            let mut world = World::new();
            world.init_resource::<Assets<Mesh>>();
            world.init_resource::<Assets<StandardMaterial>>();
            let target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
            let root = spawn_root(&mut world, target.clone());
            install(&mut world, root, target.clone());
            let old_visual = world.get::<BuiltinPlacementVisualRoot>(root).unwrap().0;
            if reparented {
                let other = world.spawn_empty().id();
                world.entity_mut(old_visual).insert(ChildOf(other));
            } else {
                world.entity_mut(old_visual).remove::<BuiltinPlacementVisual>();
            }
            set_builtin_placement_collision_enabled_world(&mut world, root, false);
            assert!(world.get::<ColliderDisabled>(old_visual).is_none(), "地址没有当前标记及真实父的共同证明就无写入权");
            install(&mut world, root, target.clone());
            assert!(world.get_entity(old_visual).is_ok(), "替换不能销毁已被其他层级或作者接管的旧地址");
            let replacement = world.get::<BuiltinPlacementVisualRoot>(root).unwrap().0;
            assert_ne!(replacement, old_visual);
            assert_eq!(world.get::<ChildOf>(replacement).map(ChildOf::parent), Some(root));
            set_builtin_placement_collision_enabled_world(&mut world, root, false);
            assert!(world.get::<ColliderDisabled>(replacement).is_some());
            assert!(world.get::<ColliderDisabled>(old_visual).is_none());
        }
    }

    #[test]
    fn builtin_cache_stays_bounded_during_repeated_dimension_and_material_edits() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        for index in 0..BUILTIN_CACHE_ENTRIES + 8 {
            let target = ScenePlacementAssetTarget::Cuboid { size_mm: [index as u32 + 1; 3],
                color_srgb: [(index % 256) as u8, (index / 256) as u8, 0], roughness_milli: 700, collision: false };
            let root = spawn_root(&mut world, target.clone());
            install(&mut world, root, target.clone());
            world.despawn(root);
        }
        let cache = world.resource::<BuiltinPrimitiveCache>();
        assert_eq!(cache.meshes.len(), BUILTIN_CACHE_ENTRIES);
        assert_eq!(cache.materials.len(), BUILTIN_CACHE_ENTRIES);
        assert_eq!(cache.mesh_order.len(), BUILTIN_CACHE_ENTRIES);
        assert_eq!(cache.material_order.len(), BUILTIN_CACHE_ENTRIES);
    }

    #[test]
    fn builtin_candidate_shared_body_blocks_real_constraints_until_adoption_and_after_retirement() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin,
            bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(Gravity(Vec3::ZERO))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)));
        app.finish();
        let batch = app.world_mut().spawn((Transform::IDENTITY, RigidBody::Static, RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)).id();
        let root = spawn_root(app.world_mut(), ScenePlacementAssetTarget::DEFAULT_CUBOID);
        app.world_mut().entity_mut(root).insert(ChildOf(batch));
        install(app.world_mut(), root, ScenePlacementAssetTarget::DEFAULT_CUBOID);
        let visual = app.world().get::<Children>(root).unwrap()[0];
        let dynamic = app.world_mut().spawn((RigidBody::Dynamic, LockedAxes::ALL_LOCKED,
            Collider::sphere(0.5), Transform::from_xyz(0.0, 1.1, 0.0))).id();
        for _ in 0..4 { app.update(); }
        assert_eq!(app.world().get::<ColliderOf>(visual).unwrap().body, batch);
        assert_eq!(app.world_mut().run_system_once(active_ground_hit).unwrap(), Some(dynamic)); // 隐藏盒被正式地面过滤，只留下测试动态球。
        let constraints = |app: &App| app.world().resource::<ContactGraph>().contact_pairs_with(dynamic)
            .any(|pair| (pair.collider1 == visual || pair.collider2 == visual) && pair.generates_constraints());
        assert!(!constraints(&app), "隐藏候选不能给玩法刚体生成承托约束");
        app.world_mut().entity_mut(batch).remove::<(RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)>();
        let proxy_key = *app.world().get::<avian3d::collider_tree::ColliderTreeProxyKey>(visual).unwrap();
        assert!(!app.world().resource::<avian3d::collider_tree::ColliderTrees>().get_proxy(proxy_key).unwrap().flags
            .contains(avian3d::collider_tree::ColliderTreeProxyFlags::BODY_DISABLED),
            "移除组件时旧Has仍为真，但已有代理必须立即转为启用");
        assert!(app.world().resource::<avian3d::collider_tree::MovedProxies>().contains(proxy_key),
            "完全锁定的重叠刚体不会改变AABB，重新启用必须主动登记重检测");
        for _ in 0..4 { app.update(); }
        assert!(constraints(&app), "正式采用后解析盒必须参与真实接触求解");
        app.world_mut().entity_mut(batch).insert((RigidBodyDisabled, bevy_tnua::TnuaNotPlatform));
        assert!(app.world().resource::<avian3d::collider_tree::ColliderTrees>().get_proxy(proxy_key).unwrap().flags
            .contains(avian3d::collider_tree::ColliderTreeProxyFlags::BODY_DISABLED));
        // 重复写入标记也不得通过Discard的旧组件生命周期把代理重新启用。
        app.world_mut().entity_mut(batch).insert(RigidBodyDisabled);
        assert!(app.world().resource::<avian3d::collider_tree::ColliderTrees>().get_proxy(proxy_key).unwrap().flags
            .contains(avian3d::collider_tree::ColliderTreeProxyFlags::BODY_DISABLED));
        for _ in 0..4 { app.update(); }
        assert!(!constraints(&app), "退役批次即使尚未回收，也不能继续承托玩法刚体");
    }

    #[test]
    fn builtin_paste_ghost_reparenting_rebinds_real_collider_to_the_disabled_group_body() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin,
            bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)));
        app.finish();
        let group = app.world_mut().spawn((Transform::IDENTITY, RigidBody::Static, RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)).id();
        let root = spawn_root(app.world_mut(), ScenePlacementAssetTarget::DEFAULT_CUBOID);
        app.world_mut().entity_mut(root).insert(RigidBody::Static);
        install(app.world_mut(), root, ScenePlacementAssetTarget::DEFAULT_CUBOID);
        let visual = app.world().get::<Children>(root).unwrap()[0];
        // 模拟粘贴：先移除独立预览刚体，再重新写父关系，让Avian寻找公共容器。
        app.world_mut().entity_mut(root).remove::<RigidBody>().insert(ChildOf(group));
        for _ in 0..4 { app.update(); }
        assert_eq!(app.world().get::<ColliderOf>(visual).unwrap().body, group);
        assert!(app.world().get::<RigidBodyDisabled>(group).is_some());
        assert_eq!(app.world_mut().run_system_once(active_ground_hit).unwrap(), None);
    }

    fn sloped_target() -> ScenePlacementAssetTarget {
        let mut target = ScenePlacementAssetTarget::DEFAULT_CUBOID.into_editable().unwrap();
        let ScenePlacementAssetTarget::EditableCuboid { ref mut vertices_half_mm, .. } = target else { unreachable!() };
        for index in [3, 7] { vertices_half_mm[index][1] = 4_000; }
        assert!(target.validate().is_ok());
        target
    }

    #[test]
    fn editable_visual_normals_and_real_triangle_collider_share_the_same_surface() {
        use bevy::mesh::VertexAttributeValues;
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let target = sloped_target();
        let root = spawn_root(&mut world, target.clone());
        let second = spawn_root(&mut world, target.clone());
        install(&mut world, root, target.clone());
        install(&mut world, second, target.clone());
        let visual = world.get::<Children>(root).unwrap()[0];
        let other = world.get::<Children>(second).unwrap()[0];
        let mesh_handle = &world.get::<Mesh3d>(visual).unwrap().0;
        assert_eq!(mesh_handle, &world.get::<Mesh3d>(other).unwrap().0);
        let mesh = world.resource::<Assets<Mesh>>().get(mesh_handle).unwrap();
        let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!("需要三维坐标") };
        let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() else { panic!("需要逐三角法线") };
        assert_eq!(positions.len(), 36);
        assert_eq!(mesh.indices().unwrap().len(), 36);
        let points = target.editable_vertices_half_mm().unwrap().map(|point| Vec3::from_array(point.map(|value| value as f32 * 0.0005)));
        let collider = world.get::<Collider>(visual).unwrap();
        let triangles = collider.shape().as_trimesh().expect("变形网格必须安装真实三角碰撞");
        assert_eq!(triangles.indices().len(), 12);
        for (index, triangle) in EDITABLE_CUBOID_TRIANGLES.into_iter().enumerate() {
            let [a, b, c] = triangle.map(|vertex| points[vertex]);
            let normal = (b-a).cross(c-a).normalize();
            for corner in 0..3 {
                assert_eq!(positions[index*3+corner], points[triangle[corner]].to_array());
                assert!(Vec3::from_array(normals[index*3+corner]).distance(normal) < 1e-6);
            }
        }
        let ray = avian3d::parry::query::Ray::new(Vec3::new(-0.5, 10.0, 0.0), Vec3::NEG_Y);
        let distance = collider.shape().cast_local_ray(&ray, 20.0, false).unwrap();
        assert!((distance - 8.75).abs() < 1e-5, "真实斜面高度应为1.25米，不能用顶部2米的包围盒冒充");
        let outside = avian3d::parry::query::Ray::new(Vec3::new(1.5, 10.0, 0.0), Vec3::NEG_Y);
        assert!(collider.shape().cast_local_ray(&outside, 20.0, false).is_none());
    }

    #[test]
    fn editable_triangle_support_obeys_the_existing_candidate_body_gate() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin,
            bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(Gravity(Vec3::ZERO))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)));
        app.finish();
        let batch = app.world_mut().spawn((Transform::IDENTITY, RigidBody::Static, RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)).id();
        let target = sloped_target();
        let root = spawn_root(app.world_mut(), target.clone());
        app.world_mut().entity_mut(root).insert(ChildOf(batch));
        install(app.world_mut(), root, target.clone());
        let visual = app.world().get::<Children>(root).unwrap()[0];
        let dynamic = app.world_mut().spawn((RigidBody::Dynamic, LockedAxes::ALL_LOCKED,
            Collider::sphere(0.5), Transform::from_xyz(0.0, 1.8, 0.0))).id();
        let constraints = |app: &App| app.world().resource::<ContactGraph>().contact_pairs_with(dynamic)
            .any(|pair| (pair.collider1 == visual || pair.collider2 == visual) && pair.generates_constraints());
        for _ in 0..4 { app.update(); }
        assert_eq!(app.world().get::<ColliderOf>(visual).unwrap().body, batch);
        assert!(!constraints(&app));
        app.world_mut().entity_mut(batch).remove::<(RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)>();
        for _ in 0..4 { app.update(); }
        assert!(constraints(&app), "采用后真实斜面必须参与刚体接触求解");
        app.world_mut().entity_mut(batch).insert((RigidBodyDisabled, bevy_tnua::TnuaNotPlatform));
        for _ in 0..4 { app.update(); }
        assert!(!constraints(&app));
    }

    #[test]
    fn extruded_mesh_visual_and_collider_use_the_same_added_cap_and_sides() {
        use bevy::mesh::VertexAttributeValues;
        let before = record(ScenePlacementAssetTarget::DEFAULT_CUBOID);
        let after = feathrise_scene_content::extruded_placement_record(&before,3,0.5).unwrap();
        let geometry = after.asset_target.editable_geometry().unwrap();
        let mut world = World::new(); world.init_resource::<Assets<Mesh>>(); world.init_resource::<Assets<StandardMaterial>>();
        let root = spawn_root(&mut world,after.asset_target.clone()); install(&mut world,root,after.asset_target.clone());
        let other = spawn_root(&mut world,after.asset_target.clone()); install(&mut world,other,after.asset_target.clone());
        let visual = world.get::<Children>(root).unwrap()[0]; let other_visual = world.get::<Children>(other).unwrap()[0];
        assert_eq!(world.get::<Mesh3d>(visual).unwrap().0,world.get::<Mesh3d>(other_visual).unwrap().0);
        let mesh = world.resource::<Assets<Mesh>>().get(&world.get::<Mesh3d>(visual).unwrap().0).unwrap();
        let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!("需要三维坐标") };
        let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() else { panic!("需要三角法线") };
        let collider = world.get::<Collider>(visual).unwrap(); let trimesh = collider.shape().as_trimesh().unwrap();
        let triangles = geometry.triangles(); let vertices = geometry.vertices().iter().map(|point| Vec3::from_array(point.map(|value| value as f32*0.0005))).collect::<Vec<_>>();
        assert_eq!((geometry.vertex_count(),geometry.face_count()),(12,10)); assert_eq!(positions.len(),60);
        assert_eq!(trimesh.indices().len(),triangles.len());
        for (index,triangle) in triangles.iter().enumerate() {
            assert_eq!(trimesh.indices()[index],triangle.map(|vertex| vertex as u32));
            let [a,b,c] = triangle.map(|vertex| vertices[vertex]); let normal = (b-a).cross(c-a).normalize();
            for corner in 0..3 {
                assert_eq!(positions[index*3+corner],vertices[triangle[corner]].to_array());
                assert!(Vec3::from_array(normals[index*3+corner]).distance(normal)<1e-6);
            }
        }
        let ray = avian3d::parry::query::Ray::new(Vec3::new(0.0,10.0,0.0),Vec3::NEG_Y);
        assert!((collider.shape().cast_local_ray(&ray,20.0,false).unwrap()-8.5).abs()<1e-5);
        world.entity_mut(root).insert(Visibility::Hidden); install(&mut world,root,after.asset_target.clone());
        let hidden_visual = world.get::<Children>(root).unwrap()[0]; assert!(world.get::<ColliderDisabled>(hidden_visual).is_some());
    }
}
