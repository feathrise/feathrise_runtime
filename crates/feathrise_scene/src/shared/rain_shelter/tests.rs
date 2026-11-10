//! 正式遮雨ECS桥回归：显式无碰撞extras、移动、删除、非法热更与切换场景。

use super::*;
use bevy::gltf::GltfExtras; // 使用真实glTF节点属性组件。
use bevy::prelude::*; // 最小App、变换和状态。
use feathrise_environment::rain_shelter::{RainShelterBox, RainShelterState}; // 实际公开作者/消费者接口。
use feathrise_scene_content::ActiveDynamicScene; // 场景完整身份。
use feathrise_scene_core::state::ScenePhase; // 当前场景状态。 // 使用正式插件与解析器。

fn app() -> App {
    // 不安装GPU或物理插件。
    let mut app = App::new(); // ECS宿主足以覆盖生产查询冲突。
    app.insert_resource(State::new(ScenePhase::Active)); // 明确活动场景。
    app.insert_resource(ActiveDynamicScene {
        scene_id: Some(feathrise_scene_content::SceneId::parse("MAP-shelter-first").unwrap()),
    }); // 玩法场景必须显式指定场景身份。
    app.add_plugins(RainShelterPlugin); // 正式Update+PostUpdate链。
    app // 返回完整小宿主。
} // 完成夹具。

fn source() -> RainShelterBox {
    // 可直接给无Mesh空节点添加。
    RainShelterBox {
        center: Vec3::ZERO,
        half_extents: Vec3::new(3.0, 0.2, 2.0),
    } // 三轴局部半径。
} // 完成小输入。

#[test] // extras无需Collider即可产生遮挡，并跟随世界变换热更。
fn extras_move_remove_and_invalid_recovery_share_production_state() {
    // 严格运行正式系统。
    let mut app = app(); // 无物理/渲染插件。
    let node = app
        .world_mut()
        .spawn((
            GltfExtras {
                value: r#"{"rain_shelter":{"center":[0,0,0],"half_extents":[3,0.2,2]}}"#.into(),
            },
            GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 0.0)),
        ))
        .id(); // 不含Mesh、Collider或Name，证明显式代理独立性。
    app.update(); // extras解析、延迟组件应用、完整发布。
    assert_eq!(
        app.world()
            .resource::<RainShelterState>()
            .exposure(Vec3::ZERO),
        0.0
    ); // 屋檐下挡雨。
    app.world_mut()
        .entity_mut(node)
        .insert(GlobalTransform::from(Transform::from_xyz(10.0, 4.0, 0.0))); // 模拟父变换传播后的移动。
    app.update(); // 新逆矩阵同次发布。
    assert_eq!(
        app.world()
            .resource::<RainShelterState>()
            .exposure(Vec3::ZERO),
        1.0
    ); // 旧位置不得留下幽灵遮挡。
    assert_eq!(
        app.world()
            .resource::<RainShelterState>()
            .exposure(Vec3::new(10.0, 0.0, 0.0)),
        0.0
    ); // 新位置正确遮挡。
    app.world_mut().entity_mut(node).insert(GltfExtras {
        value: r#"{"rain_shelter":{"center":[0,0,0],"half_extents":[0,0.2,2]}}"#.into(),
    }); // 坏热更不能当作合法缺省。
    app.update(); // 整批拒绝而非留下旧盒。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 无效作者明确关闭。
    app.world_mut().entity_mut(node).insert(GltfExtras {
        value: r#"{"rain_shelter":{"center":[0,0,0],"half_extents":[3,0.2,2]}}"#.into(),
    }); // 作者修复同一个节点。
    app.update(); // 重新完整接管。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 1.0); // 修复可恢复。
    app.world_mut().entity_mut(node).remove::<GltfExtras>(); // 删除作者属性必须取消代理。
    app.update(); // 按所有权标记移除派生组件。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 没有幽灵遮雨。
} // 完成extras生命周期。

#[test] // 超预算必须原子关闭，减少回预算后完整恢复。
fn over_budget_rejects_whole_batch_and_scene_switch_clears() {
    // 实际系统不能静默挑32个源。
    let mut app = app(); // 正式桥夹具。
    let mut nodes = Vec::new(); // 仅测试存储节点身份。
    for _ in 0..33 {
        nodes.push(
            app.world_mut()
                .spawn((
                    source(),
                    GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 0.0)),
                ))
                .id(),
        );
    } // 作者33体越过明确契约。
    app.update(); // 完整批次预算门拒绝。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 不能发布32个半成品。
    app.world_mut().despawn(nodes.pop().unwrap()); // 作者删去多余源。
    app.update(); // 删除事件允许重建。
    assert_eq!(
        app.world().resource::<RainShelterState>().gpu.header.x,
        32.0
    ); // 剩余全体一次恢复。
    app.world_mut().resource_mut::<ActiveDynamicScene>()
        .select(feathrise_scene_content::SceneId::parse("MAP-shelter-second").unwrap()); // 模拟同枚举切换场景时旧实体尚未卸载。
    app.update(); // 场景身份门不允许旧源串用。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 旧场景盒不能遮挡新图。
    app.world_mut()
        .entity_mut(nodes[0])
        .insert(GlobalTransform::from(Transform::from_xyz(1.0, 4.0, 0.0))); // 切换场景期间旧根最后一次变换传播。
    app.update(); // 变换差分不能覆盖创建时场景身份。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 旧源移动后仍不能被新场景认领。
} // 完成超限和切换场景回归。

#[test] // 严格作者格式不把无关extras变成屋顶。
fn author_extras_reject_bad_dimensions_and_ignore_unrelated_fields() {
    // 不依赖渲染或物理。
    assert!(
        helpers::parse_extras(r#"{"note":"roof"}"#)
            .unwrap()
            .is_none()
    ); // 名称语义不能隐式启用。
    assert!(
        helpers::parse_extras(r#"{"rain_shelter":{"center":[0,0,0],"half_extents":[1,2]}}"#)
            .is_err()
    ); // 缺轴不推测。
    assert!(
        helpers::parse_extras(r#"{"rain_shelter":{"center":[0,0,0],"half_extents":[1,-2,3]}}"#)
            .is_err()
    ); // 负半径明确无效。
} // 完成作者输入回归。

#[test] // 加载期保留的旧场景根不能由首次新场景激活认领。
fn loading_root_identity_and_ancestor_reparent_are_respected() {
    // 检查祖先缓存真实失效链。
    let mut app = app(); // 无物理宿主。
    app.world_mut()
        .insert_resource(State::new(ScenePhase::Loading)); // 场景根可以先于玩法激活创建。
    let old_root = app
        .world_mut()
        .spawn(DespawnOnExit(ScenePhase::Idle))
        .id(); // 非玩法旧根不能被新场景认领。
    let middle = app.world_mut().spawn(ChildOf(old_root)).id(); // 代理父级之上的中间节点。
    let node = app
        .world_mut()
        .spawn((
            source(),
            GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 0.0)),
            ChildOf(middle),
        ))
        .id(); // 屋檐通过祖先归属旧图。
    app.update(); // 加载态缓存根但不激活。
    app.world_mut()
        .insert_resource(State::new(ScenePhase::Active)); // 下一活动场景不是该根所属场景。
    app.update(); // None创建context也不能绕过真实根。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 不认领加载期旧根。
    let new_root = app
        .world_mut()
        .spawn(DespawnOnExit(ScenePhase::Active))
        .id(); // 作者明确把尚未采用的节点挂入新图。
    app.world_mut().entity_mut(middle).insert(ChildOf(new_root)); // 只改祖先，代理自己的ChildOf没有变化。
    app.update(); // 反向祖先索引必须让该源重缓存。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 1.0); // 真实新根下允许完整激活。
    app.world_mut().entity_mut(node).remove::<GlobalTransform>(); // 失去世界几何证据。
    app.update(); // 变换移除事件撤销缓存和根索引。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 没有幽灵遮挡。
} // 完成加载身份与祖先重挂回归。

#[test] // 多个盒共用同一实际场景根也必须遵守32活动体预算。
fn one_scene_root_with_thirty_three_boxes_rejects_the_whole_batch() {
    // 与独立根数量预算分开验证。
    let mut app = app(); // 正式桥夹具。
    let root = app
        .world_mut()
        .spawn(DespawnOnExit(ScenePhase::Active))
        .id(); // 全部盒共享一个缓存根。
    let mut last = root; // 保存一个待移除盒身份。
    for _ in 0..33 {
        last = app
            .world_mut()
            .spawn((
                source(),
                GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 0.0)),
                ChildOf(root),
            ))
            .id();
    } // 一个根的33个活动体。
    app.update(); // 第33体使整批拒绝。
    assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // 不截断为32个。
    assert!(
        app.world()
            .resource::<resources::RainShelterRegistry>()
            .error
            .unwrap()
            .contains("活动代理")
    ); // 明确检查活动体门而非根数量门。
    app.world_mut().despawn(last); // 作者回到32体契约。
    app.update(); // 定向移除后完整恢复。
    assert_eq!(
        app.world().resource::<RainShelterState>().gpu.header.x,
        32.0
    ); // 同一次发布全部有效体。
} // 完成独立活动体预算回归。
