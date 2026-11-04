//! 通用Mesh手动装配、原型登记与revision生命周期程序化回归。
//! 【功能设计】全部fixture只使用内存Mesh/Material句柄和显式落点，不启动窗口、AssetServer或RenderApp。

use bevy::camera::visibility::VisibilityRange; // 构造与正式原型相同的Bevy距离LOD值。
use bevy::ecs::system::RunSystemOnce; // 捕获测试原型的一次性系统不能进入全局系统缓存。
use bevy::prelude::{
    App, Assets, Children, Commands, Entity, Handle, IVec2, Mesh, Rect, StandardMaterial,
    Transform, Vec2, With, World,
}; // 无窗口ECS世界即可验证多部件实例层级与自动生命周期。

use crate::{
    ExplicitVegetationPlacement, MarkVegetationDirty, VegetationCandidateSource,
    VegetationClaimSampling, VegetationCompetition, VegetationCompetitionGroup,
    VegetationCompetitionGroupId, VegetationCompetitionMode, VegetationDensityField,
    VegetationFootprint, VegetationFootprintScale, VegetationFootprintShape, VegetationLayerConfig,
    VegetationLayerId, VegetationLayoutState, VegetationPlugin, VegetationPrototypeId,
    VegetationRealization, VegetationSourceField, VegetationWeight,
}; // fixture使用完整公共配置与真实布局原子提交系统。

use super::*; // 直接构造通用原型并验证私有生命周期结果。

const TEST_LAYER: VegetationLayerId = VegetationLayerId(3); // 固定层ID便于状态和所有者断言。
const TEST_GROUP: VegetationCompetitionGroupId = VegetationCompetitionGroupId(5); // 单层Additive组不改变密度。
const TEST_PROTOTYPE: VegetationPrototypeId = VegetationPrototypeId(9); // 原型没有树或岩石枚举。
const TEST_INSTANCE: u64 = 77; // 稳定实例ID跨revision保持不变。

/// 构造一个只有一档一个标准PBR部件的测试原型。
fn prototype(part_id: u32) -> VegetationMeshPrototype {
    VegetationMeshPrototype {
        id: TEST_PROTOTYPE, // 所有热替换版本共享同一稳定原型ID。
        lods: vec![VegetationMeshLod {
            level: VegetationMeshLodLevel(0), // 单档足够覆盖结构与生命周期测试。
            visibility_range: VisibilityRange::default(), // 默认范围不影响ECS层级。
            parts: vec![VegetationMeshPart {
                part_id,                 // 调用者改变部件ID证明同原型热替换重建了子节点。
                mesh: Handle::default(), // 程序fixture无需真实GPU资产。
                material: Handle::<StandardMaterial>::default().into(), // 标准材质默认句柄进入类型安全绑定。
                local_transform: Transform::IDENTITY,                   // 子部件不引入额外变换。
                culling_aabb: None, // 普通测试部件使用Bevy自动网格边界。
                casts_shadow: true, // 默认参与阴影且不插入NotShadowCaster。
                billboard: None,    // 基础原型不是公告板，专项fixture在独立文件覆盖朝向适配。
                wind: None,         // 本小阶段不测试扩展风材质。
                batch_vertex_mode: Default::default(), // 最小fixture使用固定批量顶点路径。
            }],
        }],
    } // 返回可登记或手动生成的最小通用原型。
} // 完成测试原型构造。

/// 创建只安装布局核心与通用Mesh后端的无窗口应用。
fn lifecycle_app() -> App {
    let mut app = App::new(); // 不安装DefaultPlugins、窗口或RenderApp。
    app.init_resource::<Assets<Mesh>>() // 手动层级测试保持Mesh资产资源存在。
        .init_resource::<Assets<StandardMaterial>>() // 标准材质资产资源只保存测试空句柄。
        .add_plugins((VegetationPlugin, VegetationMeshPlugin)); // Update提交布局，PostUpdate同步Mesh。
    app // 返回可直接update的程序化fixture。
} // 完成无窗口应用构造。

/// 构造一层完整合法且只由Mesh表达的测试配置。
fn layer(source: VegetationCandidateSource, bounds: Rect) -> VegetationLayerConfig {
    VegetationLayerConfig {
        id: TEST_LAYER,            // 固定测试层身份。
        prototype: TEST_PROTOTYPE, // 引用注册表中的稳定原型。
        source,                    // 调用者选择显式落点或用于拒绝测试的程序网格。
        density: VegetationDensityField {
            world_bounds: bounds, // 与CPU源字段使用相同有限范围。
            gain: 1.0,            // 不改变归一化源密度。
        },
        competition: VegetationCompetition {
            group: TEST_GROUP,                              // 引用唯一Additive测试组。
            priority: 0,                                    // 单层不需要优先级差异。
            weight: VegetationWeight::default(),            // 单层等权。
            claim_sampling: VegetationClaimSampling::Point, // 默认点采样足够覆盖显式落点。
            excludes_groups: Vec::new(),                    // fixture不测试跨组排斥。
        },
        footprint: VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 0.5 }, // 合法小轮廓。
            scale: VegetationFootprintScale::InheritTransform,       // 保持公共默认缩放语义。
            exclusion_margin: 0.0,                                   // 不增加额外净空。
            recovery_width: 0.0,                                     // 不增加软恢复带。
        },
        realization: VegetationRealization::Entities, // 全部已选候选由Mesh生命周期表达。
    } // 返回完整测试层配置。
} // 完成测试层构造。

/// 把一份层配置与满密度端点源写入布局状态。
fn configure(
    world: &mut World,                 // 修改真实布局核心资源。
    source: VegetationCandidateSource, // 当前候选源。
    bounds: Rect,                      // 当前源与层共享世界范围。
    width: u32,                        // 端点字段横向样本数。
    height: u32,                       // 端点字段纵向样本数。
    tile_budget_per_frame: usize,      // 控制单帧或多帧原子提交。
) {
    let mut state = world.resource_mut::<VegetationLayoutState>(); // 取得当前待生效配置与源。
    state.tile_budget_per_frame = tile_budget_per_frame; // fixture显式控制每帧块预算。
    state
        .set_configuration(
            vec![VegetationCompetitionGroup {
                id: TEST_GROUP,                            // 唯一测试组身份。
                mode: VegetationCompetitionMode::Additive, // 单层需求不受容量切分。
                selection_cell_size: 1.0,                  // 合法固定世界选择格。
                selection_seed: 11,                        // 固定种子保证跨运行确定性。
            }],
            vec![layer(source, bounds)], // 当前应用只有一个Mesh实现层。
        )
        .expect("测试Mesh层配置必须合法"); // 非法fixture应直接失败而非污染生命周期结论。
    state.set_source_field(
        TEST_LAYER,
        VegetationSourceField::new(
            bounds,                                      // 端点源覆盖完整测试范围。
            width,                                       // 横向样本数决定一米布局精度。
            height,                                      // 纵向样本数决定一米布局精度。
            vec![1.0; width as usize * height as usize], // 满密度稳定选择全部显式候选。
        )
        .expect("测试满密度源必须合法"),
    ); // 配置与源字段共同推进新generation。
} // 完成测试布局输入配置。

/// 构造一个显式稳定落点源。
fn explicit_source(transform: Transform) -> VegetationCandidateSource {
    VegetationCandidateSource::ExplicitPlacements(vec![ExplicitVegetationPlacement {
        instance_id: TEST_INSTANCE, // 稳定ID跨布局和原型换版不变。
        transform,                  // 调用者控制最终世界变换。
        density: 1.0,               // 单点需求保持满密度。
    }]) // 返回HN树木/景观迁移使用的同类显式源。
} // 完成显式测试源构造。

/// 请求一个闭区间布局块范围重新计算。
fn send_dirty(world: &mut World, min_tile: IVec2, max_tile: IVec2) {
    world
        .resource_mut::<bevy::ecs::message::Messages<MarkVegetationDirty>>()
        .write(MarkVegetationDirty {
            min_tile, // 本批次最小闭区间块。
            max_tile, // 本批次最大闭区间块。
        }); // 使用公共消息进入真实Update布局链。
} // 完成脏块请求发送。

/// 返回当前全部自动托管根实体。
fn managed_roots(world: &mut World) -> Vec<Entity> {
    let mut query = world.query_filtered::<Entity, With<VegetationMeshManagedRoot>>(); // 只查询自动生命周期所有者。
    query.iter(world).collect() // 稳定fixture规模很小，直接收集用于断言。
} // 完成托管根查询。

/// 返回当前全部通用Mesh根身份实体。
fn all_mesh_roots(world: &mut World) -> Vec<Entity> {
    let mut query = world.query_filtered::<Entity, With<VegetationMeshInstance>>(); // 同时包含旧根与自动根。
    query.iter(world).collect() // 单一所有者测试比较完整根数量。
} // 完成全部Mesh根查询。

#[test] // 保留原手动入口的多部件表达能力。
fn one_prototype_can_express_multiple_parts_without_content_kind() {
    let mut app = App::new(); // 手动helper不要求安装布局或Mesh插件。
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>(); // 无窗口ECS世界即可验证多部件实例层级。
    let prototype = VegetationMeshPrototype {
        id: TEST_PROTOTYPE, // 原型没有树或岩石枚举。
        lods: vec![VegetationMeshLod {
            level: VegetationMeshLodLevel(0), // 单档足够覆盖结构测试。
            visibility_range: VisibilityRange::default(), // 默认范围不影响ECS层级。
            parts: vec![
                VegetationMeshPart {
                    part_id: 1, // 可由消费侧解释为树干。
                    mesh: Handle::default(),
                    material: Handle::<StandardMaterial>::default().into(), // 标准PBR走通用材质绑定。
                    local_transform: Transform::IDENTITY,
                    culling_aabb: None, // 无GPU顶点变形时不覆盖Bevy自动边界。
                    casts_shadow: true,
                    billboard: None, // 第一部件保持普通实体网格。
                    wind: Some(VegetationHeightWeightedWind {
                        root_height: 0.0,
                        tip_height: 10.0,
                        strength: 1.0,
                        frequency: 0.5,
                    }),
                    batch_vertex_mode: Default::default(),
                },
                VegetationMeshPart {
                    part_id: 2, // 可由消费侧解释为树冠。
                    mesh: Handle::default(),
                    material: Handle::<StandardMaterial>::default().into(), // 第二部件复用同类标准绑定。
                    local_transform: Transform::IDENTITY,
                    culling_aabb: None, // 无GPU顶点变形时不覆盖Bevy自动边界。
                    casts_shadow: true,
                    billboard: None, // 第二部件保持普通实体网格。
                    wind: None,
                    batch_vertex_mode: Default::default(),
                },
            ],
        }],
    };
    let root = app
        .world_mut()
        .run_system_once(move |mut commands: Commands| {
            spawn_vegetation_mesh_instance(
                &mut commands,
                TEST_LAYER,
                &prototype,
                TEST_INSTANCE,
                Transform::IDENTITY,
            )
        })
        .unwrap();
    let children = app
        .world()
        .entity(root)
        .get::<Children>()
        .expect("手动多部件根必须拥有渲染子节点")
        .iter()
        .copied()
        .collect::<Vec<_>>(); // 复制实体身份后释放Children借用。
    assert_eq!(children.len(), 2); // 一个原型可表达树干加树冠等多个PBR部件。
    let mut render_identities = children
        .iter()
        .map(|child| {
            *app.world()
                .entity(*child)
                .get::<VegetationMeshRenderIdentity>()
                .expect("每个通用Mesh子节点都必须携带可提取渲染身份") // RenderApp禁止只按共享Mesh句柄猜测归属。
        })
        .collect::<Vec<_>>(); // 收集两个部件的紧凑技术身份。
    render_identities.sort_unstable_by_key(|identity| identity.part_id); // 固定断言顺序而不依赖层级内部迭代约定。
    assert_eq!(
        render_identities,
        vec![
            VegetationMeshRenderIdentity {
                layer: TEST_LAYER,                // 两个子节点继承生成调用的布局层。
                prototype: TEST_PROTOTYPE,        // 两个子节点继承完整原型身份。
                level: VegetationMeshLodLevel(0), // 两个部件都属于fixture唯一LOD档。
                part_id: 1,                       // 第一部件可由消费侧解释为树干。
                billboard: false,                 // 普通Mesh部件不能被误标为公告板。
            },
            VegetationMeshRenderIdentity {
                layer: TEST_LAYER,                // 第二部件保持相同布局层。
                prototype: TEST_PROTOTYPE,        // 第二部件保持相同原型。
                level: VegetationMeshLodLevel(0), // 第二部件保持相同LOD档。
                part_id: 2,                       // 第二部件可由消费侧解释为树冠。
                billboard: false,                 // 普通Mesh部件同样不是公告板。
            },
        ]
    ); // 层、原型、LOD、部件和公告板位均可被RenderApp精确提取。
} // 完成原手动多部件入口回归。

#[test] // 原型登记采用当前布局，注销后确定性回收根和LOD子节点。
fn registry_adopts_explicit_layer_and_unregister_reclaims_owned_hierarchy() {
    let mut app = lifecycle_app(); // 创建真实Update/PostUpdate无窗口链。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单个64米布局块。
    configure(
        app.world_mut(),
        explicit_source(Transform::from_xyz(8.0, 0.0, 9.0)), // 一个稳定显式落点。
        bounds,
        65,
        65,
        1,
    ); // 一次update完成唯一脏块。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype(1)); // 原型登记是自动接管的显式前置。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首个活动布局。
    app.update(); // Update提交revision一，PostUpdate生成托管根。
    let roots = managed_roots(app.world_mut()); // 查询本插件实际所有者。
    assert_eq!(roots.len(), 1); // 满密度单落点只生成一个根。
    let root = roots[0]; // 保存实体用于注销回收断言。
    assert_eq!(
        app.world()
            .entity(root)
            .get::<Children>()
            .expect("托管根必须拥有LOD子节点")
            .len(),
        1
    ); // 最小原型生成恰好一个LOD部件。
    let status = app.world().resource::<VegetationMeshAdoptionStatus>();
    assert_eq!(
        status.phase,
        VegetationMeshAdoptionPhase::CurrentLayoutAdopted
    ); // 同代布局且无旧根时完整采用。
    assert_eq!(status.managed_instances, 1); // 全局托管分子与真实根一致。
    assert_eq!(status.layers[0].selected_instances, 1); // 当前布局选择唯一候选。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .remove(TEST_PROTOTYPE); // 地图离场可用原型注销作为显式释放信号。
    app.update(); // 注册表generation变化触发PostUpdate回收。
    assert!(app.world().get_entity(root).is_err()); // 根与linked-spawn子节点已经一并销毁。
    assert!(managed_roots(app.world_mut()).is_empty()); // 私有活动所有权表同步归零。
    let status = app.world().resource::<VegetationMeshAdoptionStatus>();
    assert_eq!(status.managed_instances, 0); // 只读状态不保留过期根。
    assert_eq!(
        status.layers[0].blocker,
        Some(VegetationMeshAdoptionBlocker::MissingPrototype)
    ); // 注销后明确显示未登记而非伪报采用。
} // 完成登记与回收生命周期回归。

#[test] // 同ID原型热替换只重建一次根，并保持活动布局revision不变。
fn prototype_replacement_rebuilds_root_without_consuming_layout_revision() {
    let mut app = lifecycle_app(); // 创建无窗口生命周期应用。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块满密度范围。
    configure(
        app.world_mut(),
        explicit_source(Transform::from_xyz(4.0, 0.0, 6.0)), // 固定变换排除布局变化。
        bounds,
        65,
        65,
        1,
    ); // 配置唯一显式层。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype(1)); // 登记第一版部件身份。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 建立首个活动布局。
    app.update(); // 生成第一版根。
    let first_root = managed_roots(app.world_mut())[0]; // 保存第一版实体。
    let layout_revision = app
        .world()
        .resource::<crate::ActiveVegetationLayout>()
        .revision; // 原型替换不应推进布局revision。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype(2)); // 同ID登记第二版并推进原型revision。
    app.update(); // 只由注册表generation触发生命周期重建。
    let second_roots = managed_roots(app.world_mut()); // 查询替换后的唯一托管根。
    assert_eq!(second_roots.len(), 1); // 热替换不产生双根。
    assert_ne!(second_roots[0], first_root); // 原型revision变化要求完整重建层级。
    assert!(app.world().get_entity(first_root).is_err()); // 第一版根及子节点已经回收。
    assert_eq!(
        app.world()
            .resource::<crate::ActiveVegetationLayout>()
            .revision,
        layout_revision
    ); // 原型资源换版与空间布局revision保持正交。
    let mut lod_nodes = app.world_mut().query::<&VegetationMeshLodNode>(); // 读取当前全部LOD技术身份。
    assert_eq!(
        lod_nodes
            .iter(app.world())
            .map(|node| node.part_id)
            .collect::<Vec<_>>(),
        vec![2]
    ); // 新层级只包含第二版部件身份。
} // 完成同ID原型热替换回归。

#[test] // 多帧脏块处理中保留上一根，只有同generation提交后才换版。
fn pending_configuration_generation_keeps_previous_root_until_atomic_commit() {
    let mut app = lifecycle_app(); // 创建真实预算化布局与PostUpdate生命周期链。
    let first_bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 首版只有一个块。
    configure(
        app.world_mut(),
        explicit_source(Transform::from_xyz(5.0, 0.0, 7.0)), // 首版稳定落点。
        first_bounds,
        65,
        65,
        1,
    ); // 首版一次update完成。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype(1)); // 生命周期拥有可用原型。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首版布局。
    app.update(); // 生成首版托管根。
    let first_root = managed_roots(app.world_mut())[0]; // 保存上一有效根。
    let second_bounds = Rect::from_corners(Vec2::ZERO, Vec2::new(128.0, 64.0)); // 新版跨两个水平块。
    configure(
        app.world_mut(),
        explicit_source(Transform::from_xyz(25.0, 0.0, 7.0)), // 新代配置改变同ID最终变换。
        second_bounds,
        129,
        65,
        1,
    ); // 每帧只处理一个块，首帧不能原子提交。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::new(1, 0)); // 请求两个相邻块完整换版。
    app.update(); // 只完成第一块，活动布局仍属于上一generation。
    assert!(app.world().get_entity(first_root).is_ok()); // 等待期间上一根继续可见。
    assert_eq!(managed_roots(app.world_mut()), vec![first_root]); // 未完成批次不得部分替换根。
    assert_eq!(
        app.world().resource::<VegetationMeshAdoptionStatus>().phase,
        VegetationMeshAdoptionPhase::WaitingForLayout
    ); // 只读状态明确显示配置generation尚未提交。
    app.update(); // 第二块完成后活动布局与配置generation原子对齐。
    let second_roots = managed_roots(app.world_mut()); // 查询同代提交后的新根。
    assert_eq!(second_roots.len(), 1); // 同一稳定ID仍只有一个所有者。
    assert_ne!(second_roots[0], first_root); // 变换变化触发完整层级替换。
    assert!(app.world().get_entity(first_root).is_err()); // 上一根及Children已回收。
    assert_eq!(
        *app.world()
            .entity(second_roots[0])
            .get::<Transform>()
            .expect("新托管根必须保留显式最终变换"),
        Transform::from_xyz(25.0, 0.0, 7.0)
    ); // 新根只在同代活动布局完成后采用新变换。
} // 完成多帧原子换版回归。

#[test] // 不支持的新候选源整层拒绝，并保留上一有效托管根。
fn unsupported_candidate_source_rejects_new_generation_and_keeps_previous_root() {
    let mut app = lifecycle_app(); // 创建无窗口生命周期应用。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块测试范围。
    configure(
        app.world_mut(),
        explicit_source(Transform::from_xyz(6.0, 0.0, 8.0)), // 首版受支持显式源。
        bounds,
        65,
        65,
        1,
    ); // 首版一次提交。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype(1)); // 登记唯一原型。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首版布局。
    app.update(); // 生成首版托管根。
    let first_root = managed_roots(app.world_mut())[0]; // 保存上一有效集合。
    configure(
        app.world_mut(),
        VegetationCandidateSource::DenseGrid {
            spacing: 2.0, // 合法但没有有限驻留枚举边界的程序网格。
            seed: 13,     // 固定种子不影响本阶段拒绝原因。
        },
        bounds,
        65,
        65,
        1,
    ); // 新generation切换到本阶段不支持的候选源。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 让核心原子提交新空间配置。
    app.update(); // 同代布局完成后Mesh层结构化拒绝。
    assert_eq!(managed_roots(app.world_mut()), vec![first_root]); // 拒绝不能制造空洞或部分新根。
    let status = app.world().resource::<VegetationMeshAdoptionStatus>();
    assert_eq!(
        status.phase,
        VegetationMeshAdoptionPhase::RejectedKeepingFallback
    ); // 全局采用阶段保持最保守拒绝状态。
    assert_eq!(
        status.layers[0].blocker,
        Some(VegetationMeshAdoptionBlocker::UnsupportedCandidateSource)
    ); // 拒绝原因不依赖内容名称。
} // 完成不支持候选源原子拒绝回归。

#[test] // 同层同ID根拓扑变化会自动让渡所有权，不要求伪造布局或原型换版。
fn root_topology_changes_preserve_single_ownership_without_revision_bump() {
    let mut app = lifecycle_app(); // 创建无窗口生命周期应用。
    let transform = Transform::from_xyz(9.0, 0.0, 11.0); // 旧根与公共候选使用相同最终变换。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块满密度范围。
    configure(
        app.world_mut(),
        explicit_source(transform), // 当前布局选择同一稳定ID。
        bounds,
        65,
        65,
        1,
    ); // 配置唯一显式层。
    let legacy_root = app
        .world_mut()
        .spawn((
            VegetationMeshInstance {
                layer: TEST_LAYER,          // 旧根已经登记通用层身份。
                prototype: TEST_PROTOTYPE,  // 旧根已经登记通用原型身份。
                instance_id: TEST_INSTANCE, // 旧根占用相同稳定ID。
            },
            transform, // 旧根继续承担可见变换。
        ))
        .id(); // 不插入ManagedRoot即可模拟当前HN旧实现。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype(1)); // 即使原型已登记也不能越过旧所有者。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求当前活动布局。
    app.update(); // PostUpdate发现旧根并让出自动所有权。
    assert_eq!(all_mesh_roots(app.world_mut()), vec![legacy_root]); // 世界中始终只有旧根一个所有者。
    assert!(managed_roots(app.world_mut()).is_empty()); // 通用后端没有偷生成第二个根。
    let status = app.world().resource::<VegetationMeshAdoptionStatus>();
    assert_eq!(status.phase, VegetationMeshAdoptionPhase::FallbackRetained); // 迁移状态明确保留旧实现。
    assert_eq!(
        status.layers[0].blocker,
        Some(VegetationMeshAdoptionBlocker::ExistingOwner)
    ); // 单一所有者门提供结构化原因。
    app.world_mut().entity_mut(legacy_root).despawn(); // 项目切换点显式移除旧根。
    app.update(); // 组件钩子推进根拓扑generation并触发安全接管。
    assert_eq!(all_mesh_roots(app.world_mut()).len(), 1); // 切换后仍只有一个根。
    let managed_root = managed_roots(app.world_mut())[0]; // 唯一根现在由通用生命周期托管。
    assert_eq!(
        app.world().resource::<VegetationMeshAdoptionStatus>().phase,
        VegetationMeshAdoptionPhase::CurrentLayoutAdopted
    ); // 旧根消失后才报告当前布局完整采用。
    let replacement_legacy_root = app
        .world_mut()
        .spawn((
            VegetationMeshInstance {
                layer: TEST_LAYER,          // 新旧适配器若晚到仍声明同一层。
                prototype: TEST_PROTOTYPE,  // 原型身份与当前托管根相同。
                instance_id: TEST_INSTANCE, // 稳定ID冲突要求自动根主动让出。
            },
            transform, // 晚到旧根继续保留公共最终变换。
        ))
        .id(); // 模拟地图系统在稳定revision中重新生成旧根。
    app.update(); // 根拓扑变化触发自动根回收，不等待registry或layout变化。
    assert!(app.world().get_entity(managed_root).is_err()); // 通用根及Children已经主动让出。
    assert_eq!(
        all_mesh_roots(app.world_mut()),
        vec![replacement_legacy_root]
    ); // PostUpdate结束时仍严格只有一个可见所有者。
    assert_eq!(
        app.world().resource::<VegetationMeshAdoptionStatus>().phase,
        VegetationMeshAdoptionPhase::FallbackRetained
    ); // 晚到旧根重新成为明确托底。
} // 完成单一所有者迁移门回归。

mod billboard; // 独立锁定固定/动态公告板、唯一相机、回退、诊断与注销释放契约。
mod material; // 独立锁定标准/扩展材质、AlphaMode、阴影、AABB与注销释放契约。
