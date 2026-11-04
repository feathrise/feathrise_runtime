//! 标准PBR与任意扩展Material绑定的无窗口生命周期fixture。
//! 【功能设计】同一原型可混合不同具体材质组件，并保持AlphaMode、阴影、保守AABB与注销释放正交。

use bevy::asset::Asset; // 测试扩展材质必须拥有真实Bevy资产类型身份。
use bevy::camera::primitives::Aabb; // GPU顶点变形材质使用显式保守剔除边界。
use bevy::light::NotShadowCaster; // 阴影开关必须独立于具体材质类型。
use bevy::pbr::{ExtendedMaterial, MaterialExtension}; // 构造与树木风材质同形的StandardMaterial扩展。
use bevy::prelude::{
    AlphaMode, Assets, Children, Handle, MeshMaterial3d, StandardMaterial, Transform, Vec3,
}; // 无窗口世界读取两种具体材质组件、层级和PBR设置。
use bevy::reflect::TypePath; // 测试扩展必须提供Asset要求的静态类型路径。
use bevy::render::render_resource::AsBindGroup; // 测试扩展满足Bevy MaterialExtension绑定契约。

use super::*; // 复用真实布局、注册表、显式候选和根查询fixture。

/// 模拟项目侧GPU风扩展；公共植被crate不需要知道其业务类型或Shader。
#[derive(Asset, AsBindGroup, Clone, Debug, Default, TypePath)]
struct TestWindExtension {
    #[uniform(100)]
    strength: f32, // 单个Uniform足以让ExtendedMaterial获得独立具体材质类型。
} // 完成测试扩展材质数据声明。

impl MaterialExtension for TestWindExtension {} // 默认Shader入口足以验证类型化ECS绑定，不启动RenderApp。

type TestWindMaterial = ExtendedMaterial<StandardMaterial, TestWindExtension>; // 与正式树木风材质保持相同扩展结构。

#[test] // 标准与扩展材质必须共存、保持原资产语义，并随托管根一起释放。
fn mixed_material_bindings_preserve_typed_components_and_render_contracts() {
    let mut app = lifecycle_app(); // 创建真实布局Update与Mesh PostUpdate生命周期。
    app.init_resource::<Assets<TestWindMaterial>>(); // 只登记测试资产容器，不安装窗口或RenderApp。
    let standard_material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            alpha_mode: AlphaMode::Opaque, // 标准部件保持不透明PBR。
            ..Default::default()           // 其余PBR字段不影响绑定契约。
        }); // 保存真实强标准材质句柄。
    let wind_material = app
        .world_mut()
        .resource_mut::<Assets<TestWindMaterial>>()
        .add(ExtendedMaterial {
            base: StandardMaterial {
                alpha_mode: AlphaMode::Mask(0.45), // 扩展树冠保留正式双面叶簇同类AlphaMask阈值。
                double_sided: true,                // 双面语义仍由基础PBR资产自身拥有。
                ..Default::default()               // 其余PBR字段保持默认。
            }, // 完成扩展材质基础PBR配置。
            extension: TestWindExtension { strength: 0.8 }, // 模拟项目GPU风Uniform。
        }); // 保存真实强扩展材质句柄。
    let standard_binding = VegetationMeshMaterialBinding::standard(standard_material.clone()); // 标准便利入口不改变句柄。
    let wind_binding = VegetationMeshMaterialBinding::new(wind_material.clone()); // 泛型入口擦除具体扩展类型。
    assert!(standard_binding.is_material::<StandardMaterial>()); // 标准绑定保留真实材质TypeId。
    assert!(wind_binding.is_material::<TestWindMaterial>()); // 扩展绑定保留完整ExtendedMaterial TypeId。
    assert_ne!(standard_binding.asset_id(), wind_binding.asset_id()); // 异构材质不能因内部索引相同而碰撞。
    let expanded_aabb = Aabb::from_min_max(Vec3::new(-2.0, -0.5, -2.0), Vec3::new(2.0, 8.0, 2.0)); // 覆盖测试GPU摆动最坏边界。
    let prototype = VegetationMeshPrototype {
        id: TEST_PROTOTYPE, // 同一内容无关原型混合标准与扩展材质。
        lods: vec![VegetationMeshLod {
            level: VegetationMeshLodLevel(0), // 单档即可验证异构部件装配。
            visibility_range: VisibilityRange::default(), // 距离范围不影响材质绑定。
            parts: vec![
                VegetationMeshPart {
                    part_id: 1,                            // 模拟不透明树干或岩石。
                    mesh: Handle::default(),               // 无窗口fixture无需真实GPU网格。
                    material: standard_binding.clone(),    // 插入MeshMaterial3d<StandardMaterial>。
                    local_transform: Transform::IDENTITY,  // 标准部件不偏移。
                    culling_aabb: None,                    // 静态标准PBR继续使用Bevy自动网格边界。
                    casts_shadow: true,                    // 标准部件保留阴影资格。
                    billboard: None,                       // 材质fixture不混入公告板朝向。
                    wind: None,                            // 标准部件没有扩展风参数。
                    batch_vertex_mode: Default::default(), // 材质测试使用固定批量顶点模式。
                }, // 完成标准PBR部件。
                VegetationMeshPart {
                    part_id: 2,                                          // 模拟AlphaMask GPU风树冠或灌木。
                    mesh: Handle::default(), // 无窗口fixture无需真实GPU网格。
                    material: wind_binding.clone(), // 插入MeshMaterial3d<TestWindMaterial>。
                    local_transform: Transform::from_xyz(0.0, 3.0, 0.0), // 模拟树冠局部高度。
                    culling_aabb: Some(expanded_aabb), // 显式保守边界覆盖GPU顶点摆动。
                    casts_shadow: false,     // 阴影策略独立于扩展材质类型。
                    billboard: None,         // 材质fixture不混入公告板朝向。
                    wind: Some(VegetationHeightWeightedWind {
                        root_height: 0.0, // 地表根部固定。
                        tip_height: 8.0,  // 树梢达到完整权重。
                        strength: 0.8,    // 与测试扩展强度保持同量级。
                        frequency: 1.0,   // 固定有限动画频率。
                    }), // 通用风元数据继续与实际扩展材质正交。
                    batch_vertex_mode: Default::default(),
                }, // 完成扩展材质部件。
            ], // 同一LOD包含两种不同具体Material组件。
        }], // 完成单档异构原型。
    }; // 完成测试原型构造。
    let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(64.0)); // 单块满密度布局。
    configure(
        app.world_mut(),                      // 写入真实布局核心。
        explicit_source(Transform::IDENTITY), // 唯一稳定显式落点。
        bounds,                               // 源字段覆盖单块。
        65,                                   // 横向端点样本。
        65,                                   // 纵向端点样本。
        1,                                    // 单帧完成提交。
    ); // 完成测试配置。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .register(prototype); // 登记后生命周期才允许生成异构材质根。
    send_dirty(app.world_mut(), IVec2::ZERO, IVec2::ZERO); // 请求首个活动布局。
    app.update(); // Update提交布局，PostUpdate生成根和两种材质子节点。
    let root = managed_roots(app.world_mut())[0]; // 保存唯一托管根供层级与注销断言。
    let children = app
        .world()
        .entity(root)
        .get::<Children>()
        .expect("异构原型根必须拥有两个部件子节点")
        .iter()
        .copied()
        .collect::<Vec<_>>(); // 复制实体身份后释放Children借用。
    assert_eq!(children.len(), 2); // 同一LOD完整生成标准与扩展两个部件。
    for child in &children {
        let entity = app.world().entity(*child); // 只读当前部件全部技术组件。
        let node = entity
            .get::<VegetationMeshLodNode>()
            .expect("每个部件必须拥有通用LOD身份"); // 使用显式part_id区分两种材质。
        let material_node = entity
            .get::<VegetationMeshMaterialNode>()
            .expect("每个部件必须拥有统一材质诊断身份"); // F3无需异构查询即可读取类型和资产。
        match node.part_id {
            1 => {
                let binding = entity
                    .get::<MeshMaterial3d<StandardMaterial>>()
                    .expect("标准部件必须恢复StandardMaterial组件"); // 核对具体组件类型。
                assert_eq!(binding.0.id(), standard_material.id()); // 绑定必须保留调用方原资产身份。
                assert!(entity.get::<MeshMaterial3d<TestWindMaterial>>().is_none()); // 单节点禁止第二种材质组件。
                assert_eq!(material_node.asset_id, standard_binding.asset_id()); // 统一身份与具体组件一致。
                assert!(entity.get::<NotShadowCaster>().is_none()); // 标准部件仍参与阴影视图。
                assert!(entity.get::<Aabb>().is_none()); // 静态部件不伪造GPU风边界。
            } // 完成标准部件断言。
            2 => {
                let binding = entity
                    .get::<MeshMaterial3d<TestWindMaterial>>()
                    .expect("扩展部件必须恢复完整ExtendedMaterial组件"); // 核对具体扩展类型。
                assert_eq!(binding.0.id(), wind_material.id()); // 绑定必须保留原扩展材质资产身份。
                assert!(entity.get::<MeshMaterial3d<StandardMaterial>>().is_none()); // 扩展节点禁止残留标准材质组件。
                assert_eq!(material_node.asset_id, wind_binding.asset_id()); // 统一身份与扩展组件一致。
                assert!(entity.contains::<NotShadowCaster>()); // 阴影关闭不依赖具体材质种类。
                assert_eq!(entity.get::<Aabb>(), Some(&expanded_aabb)); // 保守GPU风边界逐字保留。
            } // 完成扩展部件断言。
            part_id => panic!("测试原型出现未声明部件：{part_id}"), // 任何额外节点都表示装配漂移。
        } // 完成当前部件类型分派。
    } // 完成两个异构材质子节点检查。
    assert_eq!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&standard_material)
            .expect("标准材质资产必须仍由原型绑定持有")
            .alpha_mode,
        AlphaMode::Opaque
    ); // 公共绑定不克隆或改写标准PBR AlphaMode。
    let stored_wind = app
        .world()
        .resource::<Assets<TestWindMaterial>>()
        .get(&wind_material)
        .expect("扩展材质资产必须仍由原型绑定持有"); // 读取原始扩展资产。
    assert_eq!(stored_wind.base.alpha_mode, AlphaMode::Mask(0.45)); // AlphaMask阈值原样保留。
    assert!(stored_wind.base.double_sided); // 双面基础PBR语义原样保留。
    assert_eq!(stored_wind.extension.strength, 0.8); // 扩展Uniform不被公共层解释或改写。
    app.world_mut()
        .resource_mut::<VegetationMeshPrototypeRegistry>()
        .remove(TEST_PROTOTYPE); // 注销异构原型触发同一通用释放路径。
    app.update(); // PostUpdate回收根和全部不同材质类型子节点。
    assert!(app.world().get_entity(root).is_err()); // 根已经确定性释放。
    for child in children {
        assert!(app.world().get_entity(child).is_err()); // 两种材质子节点都随linked-spawn层级释放。
    } // 完成异构材质层级注销回归。
} // 完成标准与扩展材质绑定生命周期fixture。
