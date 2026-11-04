//! 固定交叉面与GPU逐视图动态公告板的无窗口生命周期fixture。
//! 【功能设计】CPU只保存作者Transform；材质类型决定动态节点是否进入GPU路径，拓扑变化才刷新诊断。

use bevy::camera::primitives::Aabb; // 显式保守边界允许动态平面继续参加CPU视锥剔除。
use bevy::camera::visibility::{NoFrustumCulling, VisibilityRange}; // 缺少保守边界的动态平面必须关闭作者姿态剔除。
use bevy::prelude::{Children, Handle, Mesh, Quat, StandardMaterial, Transform, Vec3}; // 无窗口ECS世界验证材质契约、静态姿态和层级释放。

use super::*; // 复用真实布局、原型注册、生命周期与稳定根查询fixture。

/// 构造一个共享空Mesh并显式选择标准或GPU公告板材质的部件。
fn billboard_part(
    part_id: u32,                       // 稳定部件身份供层级查询。
    local_transform: Transform,         // 作者平移、缩放与局部旋转。
    billboard: VegetationMeshBillboard, // 固定、绕Y轴或全向模式。
    use_gpu_material: bool,             // 动态模式只有专用材质才能在顶点阶段转向。
    culling_aabb: Option<Aabb>,         // 可选边界必须覆盖GPU可能生成的全部朝向。
) -> VegetationMeshPart {
    let material = if use_gpu_material {
        Handle::<VegetationMeshBillboardMaterial>::default().into() // 类型身份足够让无RenderApp fixture验证GPU接线。
    } else {
        Handle::<StandardMaterial>::default().into() // 标准材质用于固定模式或故意制造动态回退。
    }; // 完成测试材质绑定选择。
    VegetationMeshPart {
        part_id,                               // 保存调用方稳定部件编号。
        mesh: Handle::<Mesh>::default(),       // 无窗口fixture无需真实GPU网格。
        material,                              // 保存具有真实具体类型的材质绑定。
        local_transform,                       // 保存作者局部姿态，CPU系统不得逐帧覆盖。
        culling_aabb,                          // 保存调用方是否提供显式全朝向保守边界。
        casts_shadow: false,                   // 最远档按正式预算退出阴影视图。
        billboard: Some(billboard),            // 显式插入统一公告板节点组件。
        wind: None,                            // 公告板保持静止且不携带风参数。
        batch_vertex_mode: Default::default(), // 本fixture仍验证原逐实例公告板装配。
    } // 返回可与其他模式共存于同一LOD的部件。
} // 完成公告板部件构造。

/// 构造一个单档任意公告板部件原型。
fn billboard_prototype(parts: Vec<VegetationMeshPart>) -> VegetationMeshPrototype {
    VegetationMeshPrototype {
        id: TEST_PROTOTYPE, // 复用测试稳定原型身份。
        lods: vec![VegetationMeshLod {
            level: VegetationMeshLodLevel(3), // 使用第四档编号表达典型最远公告板。
            visibility_range: VisibilityRange::default(), // 距离范围不影响GPU材质契约。
            parts,                            // 调用方可混合固定、圆柱和全向部件。
        }], // 单档足够验证公告板适配。
    } // 返回可登记到真实生命周期的原型。
} // 完成公告板原型构造。

/// 从根Children中按稳定part_id取得公告板子节点。
fn child_with_part(app: &App, root: Entity, part_id: u32) -> Entity {
    app.world()
        .entity(root)
        .get::<Children>()
        .expect("公告板根必须拥有部件子节点")
        .iter()
        .find(|child| {
            app.world()
                .entity(**child)
                .get::<VegetationMeshLodNode>()
                .is_some_and(|node| node.part_id == part_id) // 技术身份替代名称解析。
        })
        .copied()
        .expect("公告板原型必须生成指定part_id") // 返回真实子节点身份。
} // 完成公告板子节点查询。

#[test] // 固定、圆柱和全向模式必须共存，动态模式也不得产生CPU Transform写回。
fn gpu_billboard_modes_preserve_author_transforms_without_a_cpu_camera() {
    let mut app = lifecycle_app(); // 不安装窗口、AssetServer、RenderApp或任何相机。
    let fixed_rest = Quat::from_rotation_y(0.37); // 固定交叉面作者姿态必须逐值保留。
    let cylindrical_rest = Quat::from_rotation_y(-0.41); // 圆柱模式的CPU局部姿态同样保持静态。
    let spherical_rest = Quat::from_rotation_y(0.73); // 全向模式只在GPU构造逐视图世界矩阵。
    let prototype = billboard_prototype(vec![
        billboard_part(
            1,                                                             // 固定交叉impostor部件。
            Transform::from_xyz(-1.0, 4.0, 0.0).with_rotation(fixed_rest), // 作者姿态直接参与原材质渲染。
            VegetationMeshBillboard::Fixed, // 固定模式不需要专用材质。
            false,                          // 保持既有StandardMaterial路径。
            None,                           // 固定模式无需覆盖GPU旋转边界。
        ),
        billboard_part(
            2,                                                                  // 绕Y轴动态部件。
            Transform::from_xyz(0.0, 4.5, 0.0).with_rotation(cylindrical_rest), // 作者姿态只作为GPU退化回退。
            VegetationMeshBillboard::Cylindrical {
                front_axis: VegetationMeshBillboardFrontAxis::PositiveX, // 验证非默认资产正面轴身份。
            },
            true, // 显式绑定GPU公告板扩展材质。
            None, // 缺少保守边界时必须自动关闭CPU视锥剔除。
        ),
        billboard_part(
            3,                                                                // 完整三维动态部件。
            Transform::from_xyz(1.0, 5.0, 0.5).with_rotation(spherical_rest), // 使用独立作者姿态验证无交叉覆盖。
            VegetationMeshBillboard::Spherical {
                front_axis: VegetationMeshBillboardFrontAxis::PositiveZ, // 验证正Z资产平面身份。
            },
            true, // 显式绑定GPU公告板扩展材质。
            Some(Aabb::from_min_max(Vec3::splat(-4.0), Vec3::splat(4.0))), // 显式覆盖全向旋转的测试边界。
        ),
    ]); // 完成三模式混合原型。
    let instance_transform = Transform::from_xyz(3.0, 1.0, 4.0)
        .with_rotation(Quat::from_rotation_y(0.61))
        .with_scale(Vec3::splat(0.45)); // 模拟正式实例根的随机朝向与统一视觉缩放。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块满密度布局。
    configure(
        app.world_mut(),                     // 写入真实布局核心。
        explicit_source(instance_transform), // 唯一稳定公告板实例。
        bounds,                              // 源字段覆盖单块。
        65,                                  // 横向端点样本。
        65,                                  // 纵向端点样本。
        1,                                   // 单帧完成提交。
    ); // 完成公告板测试布局配置。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype); // 登记后通用生命周期才生成公告板层级。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首个活动布局。
    app.update(); // 生命周期生成节点，状态系统只聚合材质就绪信息。
    app.update(); // 再跑一个稳定帧，确认没有CPU朝向系统覆盖作者姿态。
    let root = managed_roots(app.world_mut())[0]; // 保存唯一托管实例根。
    let fixed = child_with_part(&app, root, 1); // 取得固定交叉面节点。
    let cylindrical = child_with_part(&app, root, 2); // 取得绕Y轴节点。
    let spherical = child_with_part(&app, root, 3); // 取得全向节点。
    for (entity, expected_rotation) in [
        (fixed, fixed_rest),             // 固定模式保持作者旋转。
        (cylindrical, cylindrical_rest), // 圆柱模式不再由CPU写旋转。
        (spherical, spherical_rest),     // 全向模式不再由CPU写旋转。
    ] {
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<Transform>()
                .expect("每个公告板子节点必须拥有Transform")
                .rotation,
            expected_rotation
        ); // 两次Update后局部旋转仍必须逐值等于作者输入。
    } // 完成三种模式CPU姿态核对。
    let fixed_node = *app
        .world()
        .entity(fixed)
        .get::<VegetationMeshBillboardNode>()
        .expect("固定部件也必须拥有统一公告板诊断组件"); // 复制固定节点快照避免延长世界借用。
    let cylindrical_node = *app
        .world()
        .entity(cylindrical)
        .get::<VegetationMeshBillboardNode>()
        .expect("圆柱部件必须拥有GPU公告板诊断组件"); // 复制圆柱节点快照。
    let spherical_node = *app
        .world()
        .entity(spherical)
        .get::<VegetationMeshBillboardNode>()
        .expect("全向部件必须拥有GPU公告板诊断组件"); // 复制全向节点快照。
    assert!(fixed_node.gpu_material_ready); // Fixed不依赖扩展材质，按定义始终可用。
    assert_eq!(fixed_node.rest_rotation, fixed_rest); // 固定作者姿态原样保留。
    assert!(cylindrical_node.gpu_material_ready); // 圆柱节点已绑定正确具体材质类型。
    assert_eq!(cylindrical_node.rest_rotation, cylindrical_rest); // 圆柱回退姿态原样保留。
    assert!(spherical_node.gpu_material_ready); // 全向节点已绑定正确具体材质类型。
    assert_eq!(spherical_node.rest_rotation, spherical_rest); // 全向回退姿态原样保留。
    assert!(!app.world().entity(fixed).contains::<NoFrustumCulling>()); // 固定平面仍可使用作者姿态的正常CPU视锥剔除。
    assert!(
        app.world()
            .entity(cylindrical)
            .contains::<NoFrustumCulling>()
    ); // 圆柱GPU旋转后的边界不能使用静态薄平面AABB。
    assert!(!app.world().entity(spherical).contains::<NoFrustumCulling>()); // 显式全朝向AABB保留正常CPU视锥剔除。
    let status = app.world().resource::<VegetationMeshBillboardStatus>(); // 读取完整材质就绪快照。
    assert_eq!(status.phase, VegetationMeshBillboardPhase::Active); // 两个动态节点都已进入GPU路径。
    assert_eq!(status.camera, None); // GPU逐视图模式不绑定单一CPU相机。
    assert_eq!(status.active_marked_cameras, 0); // 状态同步不再扫描旧相机标记。
    assert_eq!(status.total_nodes, 3); // 固定和动态节点共同统计。
    assert_eq!(status.fixed_nodes, 1); // 一个固定交叉面节点。
    assert_eq!(status.camera_facing_nodes, 2); // 圆柱和全向各一个动态节点。
    assert_eq!(status.oriented_nodes, 2); // 两个动态节点均具有正确GPU材质。
    assert_eq!(status.fallback_nodes, 0); // 正确接线没有材质回退。
} // 完成三模式GPU公告板生命周期回归。

#[test] // 固定公告板必须保持原材质并完全忽略兼容相机标记。
fn fixed_cross_plane_reports_fixed_only_and_ignores_legacy_camera_markers() {
    let mut app = lifecycle_app(); // 不安装窗口、AssetServer或RenderApp。
    let rest_rotation = Quat::from_rotation_y(-0.52); // 固定交叉面作者旋转必须始终不变。
    let prototype = billboard_prototype(vec![billboard_part(
        5,                                                               // 唯一固定公告板部件。
        Transform::from_xyz(0.0, 3.0, 0.0).with_rotation(rest_rotation), // 保存作者交叉面姿态。
        VegetationMeshBillboard::Fixed, // 明确选择零动态转向成本的模式。
        false,                          // 保持既有StandardMaterial路径。
        None,                           // 固定模式无需覆盖GPU旋转边界。
    )]); // 完成固定公告板原型。
    app.world_mut().spawn(VegetationMeshBillboardCamera); // 保留一个旧标记验证兼容类型已成为无操作组件。
    app.world_mut().spawn(VegetationMeshBillboardCamera); // 多个旧标记也不能制造歧义或逐帧查询。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块满密度布局。
    configure(
        app.world_mut(),                      // 写入真实布局核心。
        explicit_source(Transform::IDENTITY), // 根保持单位Transform。
        bounds,                               // 源字段覆盖单块。
        65,                                   // 横向端点样本。
        65,                                   // 纵向端点样本。
        1,                                    // 单帧完成提交。
    ); // 完成固定公告板布局配置。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype); // 登记固定公告板原型。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首个活动布局。
    app.update(); // 生成固定节点并发布无需GPU动态转向的状态。
    let root = managed_roots(app.world_mut())[0]; // 保存唯一托管根。
    let child = child_with_part(&app, root, 5); // 取得固定交叉面节点。
    let status = app.world().resource::<VegetationMeshBillboardStatus>(); // 读取固定模式状态。
    assert_eq!(status.phase, VegetationMeshBillboardPhase::FixedOnly); // 不误报GPU活动或相机异常。
    assert_eq!(status.camera, None); // 固定模式不选择或绑定任何相机。
    assert_eq!(status.active_marked_cameras, 0); // 兼容标记不再进入稳定帧扫描。
    assert_eq!(status.total_nodes, 1); // 唯一固定节点进入总数。
    assert_eq!(status.fixed_nodes, 1); // 固定节点分项正确。
    assert_eq!(status.camera_facing_nodes, 0); // 没有动态GPU节点。
    assert_eq!(
        app.world()
            .entity(child)
            .get::<Transform>()
            .expect("固定公告板必须拥有Transform")
            .rotation,
        rest_rotation
    ); // 旧相机标记存在时作者旋转仍逐值保留。
    assert!(!app.world().entity(child).contains::<NoFrustumCulling>()); // 固定作者姿态继续使用Bevy正常视锥剔除。
} // 完成固定交叉impostor零相机依赖回归。

#[test] // 动态模式误绑标准材质必须明确回退，注销后同帧清空状态。
fn dynamic_billboard_material_mismatch_falls_back_and_unregister_clears_status() {
    let mut app = lifecycle_app(); // 不安装窗口、AssetServer或RenderApp。
    let rest_rotation = Quat::from_rotation_y(0.29); // 材质错误时必须保留该作者旋转。
    let billboard = VegetationMeshBillboard::Cylindrical {
        front_axis: VegetationMeshBillboardFrontAxis::NegativeZ, // 使用Bevy常见局部负Z正面。
    }; // 完成动态模式配置。
    let prototype = billboard_prototype(vec![billboard_part(
        7,                                                               // 唯一动态公告板部件。
        Transform::from_xyz(0.0, 2.0, 0.0).with_rotation(rest_rotation), // 保存确定性作者回退姿态。
        billboard, // 节点仍声明圆柱GPU朝向需求。
        false,     // 故意误绑StandardMaterial验证安全诊断。
        None,      // 回退节点不会发生GPU旋转，无需扩张边界。
    )]); // 完成错误材质动态原型。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块满密度布局。
    configure(
        app.world_mut(),                      // 写入真实布局核心。
        explicit_source(Transform::IDENTITY), // 根保持单位Transform。
        bounds,                               // 源字段覆盖单块。
        65,                                   // 横向端点样本。
        65,                                   // 纵向端点样本。
        1,                                    // 单帧完成提交。
    ); // 完成材质回退布局配置。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype); // 登记唯一动态公告板原型。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首个活动布局。
    app.update(); // 节点生成后只发布材质缺失状态，不写CPU旋转。
    let root = managed_roots(app.world_mut())[0]; // 保存唯一托管根。
    let child = child_with_part(&app, root, 7); // 保存动态公告板节点。
    let node = *app
        .world()
        .entity(child)
        .get::<VegetationMeshBillboardNode>()
        .expect("动态公告板必须拥有诊断组件"); // 复制错误材质节点快照。
    assert_eq!(node.billboard, billboard); // 动态模式身份不能因材质错误而丢失。
    assert_eq!(node.rest_rotation, rest_rotation); // 作者回退姿态必须原样保存。
    assert!(!node.gpu_material_ready); // 标准材质不能冒充GPU公告板扩展材质。
    assert_eq!(
        app.world()
            .entity(child)
            .get::<Transform>()
            .expect("动态公告板必须拥有Transform")
            .rotation,
        rest_rotation
    ); // 材质错误时CPU也不启动逐帧旋转补偿。
    assert!(!app.world().entity(child).contains::<NoFrustumCulling>()); // 未发生GPU旋转时保留正常CPU视锥剔除。
    let fallback = app.world().resource::<VegetationMeshBillboardStatus>(); // 读取错误材质诊断快照。
    assert_eq!(
        fallback.phase,
        VegetationMeshBillboardPhase::ActiveWithFallback
    ); // 动态需求存在但专用材质缺失。
    assert_eq!(fallback.camera, None); // GPU契约错误与相机选择无关。
    assert_eq!(fallback.camera_facing_nodes, 1); // 唯一节点声明动态转向。
    assert_eq!(fallback.oriented_nodes, 0); // 错误材质没有进入GPU路径。
    assert_eq!(fallback.fallback_nodes, 1); // 唯一错误节点明确进入回退计数。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .remove(TEST_PROTOTYPE); // 注销原型触发通用根与公告板Children释放。
    app.update(); // 生命周期先删除层级，状态系统同帧发布空快照。
    assert!(app.world().get_entity(root).is_err()); // 托管根已经确定性释放。
    assert!(app.world().get_entity(child).is_err()); // 公告板节点随linked-spawn层级释放。
    let cleared = app.world().resource::<VegetationMeshBillboardStatus>(); // 读取注销后的诊断快照。
    assert_eq!(cleared.phase, VegetationMeshBillboardPhase::NoBillboards); // 状态不残留GPU活动或回退。
    assert_eq!(cleared.total_nodes, 0); // 全部公告板计数同帧归零。
    assert_eq!(cleared.oriented_nodes, 0); // GPU就绪计数同帧归零。
    assert_eq!(cleared.fallback_nodes, 0); // 回退计数同帧归零。
} // 完成GPU材质错误与生命周期释放回归。
