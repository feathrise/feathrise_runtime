use std::marker::PhantomData;

use bevy::asset::RenderAssetUsages; // 测试Mesh保留主世界属性供最终安全门读取。
use bevy::asset::uuid::Uuid;
use bevy::mesh::{PrimitiveTopology, VertexAttributeValues}; // 构造并显式编码最小静态与蒙皮顶点布局。
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use feathrise_scene_content::{SceneId, ScenePlacementAssetTarget, ScenePlacementHeightMode, ScenePlacementRecord};

use super::*;
use crate::shared::placement_data::resources::{PendingPlacementPlan, PendingPlacementPrimitive};

#[derive(Resource, Default)]
struct SpawnedPrimitiveRoot(Option<Entity>);

#[derive(Resource, Default)] // 捕获生产生成函数返回的蒙皮拒绝原因。
struct RejectedSkinnedPrimitiveError(Option<String>); // 测试不依赖日志文本或GPU崩溃判断结果。

#[test]
fn primitive_plan_spawns_exact_mesh_and_material_without_world_scene_root() {
    let expected_material = Handle::<StandardMaterial>::Uuid(Uuid::from_u128(202), PhantomData);
    let expected_material_id = expected_material.id();
    let mut app = App::new();
    app.init_resource::<Assets<Mesh>>(); // 生产生成函数必须读取已采用的真实Mesh资产。
    let expected_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::new(
            // 静态primitive应继续允许直接生成。
            PrimitiveTopology::TriangleList, // 拓扑不影响当前安全门。
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 与正式GLTF Mesh用途一致。
        )); // Assets分配稳定句柄供实体断言。
    let expected_mesh_id = expected_mesh.id(); // 在句柄移入测试系统前冻结期望身份。
    app.init_resource::<SpawnedPrimitiveRoot>();
    app.add_systems(
        Update,
        move |mut commands: Commands, meshes: Res<Assets<Mesh>>| {
            let scene_id = SceneId::parse("MAP-primitive-candidate").expect("测试场景ID合法");
            let staging_root = commands
                .spawn((PlacementStagingRoot, Visibility::Hidden))
                .id();
            let record = ScenePlacementRecord {
                mesh_lod: None,
                terrain_binding: None,
                editor: Default::default(),
                terrain_patch: None,
                id: "plant-part-1".to_owned(),
                asset_path: "gltf/plant.glb".to_owned(),
                asset_target: ScenePlacementAssetTarget::Primitive {
                    mesh: 6,
                    primitive: 0,
                    material: Some(2),
                    collision: false,
                },
                building_preset_id: None, // 直接 primitive 测试仍是普通资产。
                courtyard: None,          // 独立摆放记录不属于任何院落副本。
                translation: [3.0, 4.0, 5.0],
                height_mode: ScenePlacementHeightMode::FixedHeight,
                terrain_offset: 0.0,
                yaw_degrees: 20.0,
                scale: [1.0, 2.0, 1.0],
                water_surfaces: Vec::new(),
            };
            let plan = PendingPlacementPlan {
                record,
                scene: Handle::<WorldAsset>::default(), // 同源Scene0只负责规划加载锚点，实体不得实例化它。
                primitive: Some(PendingPlacementPrimitive {
                    mesh: expected_mesh.clone(),
                    material: expected_material.clone(),
                }),
                transform: Transform::from_xyz(3.0, 4.0, 5.0),
            };
            let (entity, immediately_ready) = spawn_ready_placement(
                &mut commands,
                &meshes, // 静态Mesh通过最终蒙皮属性安全门。
                &scene_id,
                &plan,
                staging_root,
                &PlacementInstanceGate::new(false),
                7,
                9,
            )
            .expect("静态primitive应允许直接生成"); // 正常建筑、道具和武器路径不得回归失败。
            assert!(immediately_ready); // Mesh与材质已在调用前通过完整加载门，无需WorldReady观察器。
            commands.insert_resource(SpawnedPrimitiveRoot(Some(entity)));
        },
    );

    app.update();

    let root = app
        .world()
        .resource::<SpawnedPrimitiveRoot>()
        .0
        .expect("测试系统应发布primitive根");
    assert!(app.world().get::<WorldAssetRoot>(root).is_none());
    assert!(app.world().get::<DynamicWaterMeshOverrides>(root).is_none()); // 直接primitive没有Scene0层级与整网格水事务。
    assert!(
        app.world()
            .get::<ScenePlacementInstance>(root)
            .expect("根应保留完整placement身份")
            .is_pending()
    );
    let children = app
        .world()
        .get::<Children>(root)
        .expect("根应有唯一可视子网格");
    assert_eq!(children.len(), 1);
    let child = children[0];
    assert!(
        app.world()
            .get::<ScenePlacementPrimitiveVisual>(child)
            .is_some()
    ); // 直接子资产必须提供窄视口命中标记，不能依赖不存在的Scene0网格名。
    assert_eq!(
        app.world()
            .get::<Mesh3d>(child)
            .expect("子实体应引用精确primitive")
            .0
            .id(),
        expected_mesh_id,
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(child)
            .expect("子实体应保留原关联材质")
            .0
            .id(),
        expected_material_id,
    );
}

#[test] // 蒙皮primitive必须在任何场景实体生成前失败，旧JSON也不能触发GPU校验崩溃。
fn skinned_primitive_plan_is_rejected_before_direct_mesh_spawn() {
    // 覆盖正式运行时候选的最终安全门。
    let mut app = App::new(); // 纯ECS测试不启动窗口或渲染器。
    app.init_resource::<Assets<Mesh>>() // 提供生产函数读取的主世界Mesh容器。
        .init_resource::<RejectedSkinnedPrimitiveError>(); // 保存一次明确拒绝结果。
    let mut skinned_mesh = Mesh::new(
        // 构造与Mixamo身体相同的关键顶点布局。
        PrimitiveTopology::TriangleList, // 三角拓扑不是拒绝原因。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // 保留属性供主世界安全门检查。
    ); // 初始网格没有骨骼属性。
    skinned_mesh.insert_attribute(
        // 加入关节索引输入。
        Mesh::ATTRIBUTE_JOINT_INDEX, // PBR特化会读取此属性。
        VertexAttributeValues::Uint16x4(vec![[0_u16, 0_u16, 0_u16, 0_u16]]), // 显式匹配关节索引格式。
    ); // 不需要实际渲染数据。
    skinned_mesh.insert_attribute(
        // 加入关节权重输入。
        Mesh::ATTRIBUTE_JOINT_WEIGHT, // 与索引齐全后管线要求Skin绑定组。
        VertexAttributeValues::Float32x4(vec![[1.0_f32, 0.0_f32, 0.0_f32, 0.0_f32]]), // 显式匹配权重格式。
    ); // 完成蒙皮分类条件。
    let skinned_handle = app // 将Mesh登记到生产函数使用的同一Assets资源。
        .world_mut() // 测试主世界持有资产。
        .resource_mut::<Assets<Mesh>>() // 取得唯一Mesh容器。
        .add(skinned_mesh); // 返回稳定强句柄。
    app.add_systems(
        // 一帧内执行实际生成入口。
        Update, // 与生产候选相同调度阶段。
        move |mut commands: Commands,    // 仅允许通过延迟命令生成实体。
              meshes: Res<Assets<Mesh>>, // 读取当前真实蒙皮布局。
              mut rejected: ResMut<RejectedSkinnedPrimitiveError>| {
            // 发布明确错误供帧后断言。
            let scene_id = SceneId::parse("MAP-skinned-primitive").expect("测试场景ID合法"); // 建立稳定场景身份。
            let staging_root = commands // 候选容器本身仍可安全创建。
                .spawn((PlacementStagingRoot, Visibility::Hidden)) // 模拟正式隐藏事务根。
                .id(); // 冻结父实体身份。
            let record = ScenePlacementRecord {
                mesh_lod: None,
                terrain_binding: None,
                editor: Default::default(),
                terrain_patch: None,
                // 模拟旧版本可能已经保存的危险记录。
                id: "skinned-body-1".to_owned(), // 错误必须回显此作者身份。
                asset_path: "gltf/character.glb".to_owned(), // 角色GLB源路径。
                asset_target: ScenePlacementAssetTarget::Primitive {
                    // 旧JSON直接指向蒙皮身体。
                    mesh: 0,           // 身体Mesh0。
                    primitive: 0,      // 身体Primitive0。
                    material: Some(0), // 原材质不能补齐Skin绑定。
                    collision: false,
                }, // 完成直接primitive目标。
                building_preset_id: None,        // 危险旧记录不伪造建筑目录身份。
                courtyard: None,                 // 独立摆放记录不属于任何院落副本。
                translation: [0.0, 0.0, 0.0],    // 变换与拒绝无关。
                height_mode: ScenePlacementHeightMode::FixedHeight, // 不读取地形高度。
                terrain_offset: 0.0,             // 没有额外贴地偏移。
                yaw_degrees: 0.0,                // 保持默认朝向。
                scale: [1.0, 1.0, 1.0],          // 保持原始比例。
                water_surfaces: Vec::new(),      // 子资产本就不携带整网格水绑定。
            }; // 完成最小正式记录。
            let plan = PendingPlacementPlan {
                // 使用候选生成函数的真实计划结构。
                record,                                 // 保留作者目标和诊断身份。
                scene: Handle::<WorldAsset>::default(), // 同源Scene0锚点不应被生成。
                primitive: Some(PendingPlacementPrimitive {
                    // 提供已经Ready的直接子资产句柄。
                    mesh: skinned_handle.clone(), // 指向当前主世界蒙皮Mesh。
                    material: Handle::<StandardMaterial>::Uuid(Uuid::from_u128(303), PhantomData), // 材质句柄不参与最终蒙皮门。
                }), // 完成直接primitive依赖。
                transform: Transform::IDENTITY,         // 生成前不应采用任何变换。
            }; // 完成单项候选计划。
            let error = spawn_ready_placement(
                // 直接调用生产最终生成边界。
                &mut commands, // 若安全门失败，不得登记placement根命令。
                &meshes,       // 使用当前真实Mesh属性。
                &scene_id,       // 保持完整场景身份。
                &plan,         // 传入蒙皮目标计划。
                staging_root,  // 模拟正式候选父根。
                &PlacementInstanceGate::new(false), // 候选作者门保持关闭。
                1,             // 测试场景代次。
                1,             // 测试文档修订。
            ) // 返回值必须是明确可报告失败。
            .expect_err("蒙皮primitive不能生成直接Mesh3d"); // 若回归为成功，测试立即失败。
            rejected.0 = Some(error); // 帧后检查错误内容与无实体副作用。
        }, // 完成单次测试系统。
    ); // 注册生产入口夹具。

    app.update(); // 执行安全门并应用允许的候选容器命令。

    let error = app // 读取公开测试结果。
        .world() // 使用只读主世界。
        .resource::<RejectedSkinnedPrimitiveError>() // 取得生产错误。
        .0 // 访问可选错误文本。
        .as_deref() // 不复制诊断字符串。
        .expect("生产入口应返回蒙皮拒绝原因"); // 必须有确定结果。
    assert!(error.contains("skinned-body-1")); // 错误定位具体作者记录。
    assert!(error.contains("完整Scene0")); // 错误给出安全替代路径。
    let placement_count = {
        // 查询任何可能被错误登记的placement根。
        let world = app.world_mut(); // QueryState创建需要主世界可变访问。
        let mut query = world.query::<&ScenePlacementInstance>(); // 只匹配正式或候选placement根。
        query.iter(world).count() // 安全门前不得产生任何匹配实体。
    }; // 释放查询借用。
    assert_eq!(placement_count, 0); // 关键回归：RenderApp永远看不到裸蒙皮Mesh3d。
} // 测试完成后只剩无可视内容的隐藏候选容器。
