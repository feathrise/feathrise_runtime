//! 场景 GLB 热重建 ECS 系统：复用 Bevy 场景更新、玩家冻结、跨帧碰撞重建与恢复。

use avian3d::prelude::{
    // 导入场景碰撞完成事件和玩家刚体控制组件。
    AngularVelocity,              // 冻结玩家前清空旋转速度，避免恢复后继承旧冲量。
    Collider,                     // 第一阶段只移除旧场景碰撞体，让 Avian 正常清理接触图。
    ColliderConstructor,          // 清理可能尚未完成的单网格构造器，避免旧网格异步生成碰撞。
    ColliderConstructorHierarchy, // 清理旧场景时移除碰撞构造器，重生时重新插入。
    ColliderConstructorHierarchyReady, // Avian 完成整棵场景碰撞构造时触发恢复流程。
    ColliderMarker, // 显式先移除 Avian 内部标记，立即触发 ContactGraph 与 PhysicsIslands 清理 observer。
    LinearVelocity, // 冻结玩家前清空线速度，防止恢复后继续下坠。
}; // 完成 Avian 类型导入。
use bevy::diagnostic::FrameCount; // F12暂停虚拟时间时仍递增，用于保证热重载至少跨一个真实帧。
use bevy::ecs::system::SystemParam;
use bevy::prelude::*; // 引入 AssetEvent、MessageReader、WorldAssetRoot、Query 和 Commands 等类型。
use crate::SceneRuntimeObserver; // 精确筛选唯一玩家，不冻结 NPC 和怪物。
use feathrise_scene_core::input::WorldPauseBlockers; // 热重载只登记暂停原因，由游戏组合根唯一写入时间资源。

use crate::components::{
    // 导入全局场景热重载的登记与生命周期组件。
    HotReloadableScene,      // 提供场景 Scene 句柄和可恢复碰撞配置。
    SceneReloadAwaitingCollision, // 标记已重生且等待碰撞完成的场景根。
    SceneReloadPending,           // 标记等待下一帧重新生成的场景根。
}; // 完成热重载组件导入。
type ReloadPlayerData = (
    Entity,
    Option<&'static mut LinearVelocity>,
    Option<&'static mut AngularVelocity>,
);

/// 场景 GLB 修改事件、碰撞层级查询和物理暂停所有权。
#[derive(SystemParam)]
pub(super) struct SceneReloadContext<'w, 's> {
    scene_events: MessageReader<'w, 's, AssetEvent<WorldAsset>>,
    scenes: Query<
        'w,
        's,
        (
            Entity,
            &'static HotReloadableScene,
            Option<&'static mut SceneReloadPending>,
            Option<&'static SceneReloadAwaitingCollision>,
        ),
    >, // 在途场景同样必须消费后续Modified，不能丢失连续保存的最新网格。
    children: Query<'w, 's, &'static Children>,
    colliders: Query<'w, 's, (), With<Collider>>,
    collider_markers: Query<'w, 's, (), With<ColliderMarker>>,
    collider_constructors: Query<'w, 's, (), With<ColliderConstructor>>,
    players: Query<'w, 's, ReloadPlayerData, With<SceneRuntimeObserver>>,
    frame_count: Res<'w, FrameCount>, // Modified事件冻结真实帧身份，不能使用暂停的Fixed时钟。
}

/// 监听全部 Scene 资产修改，只重建显式登记且句柄匹配的场景根。
pub(super) fn begin_modified_scene_reloads(
    mut commands: Commands, // 清理旧场景实例并冻结玩家刚体。
    context: SceneReloadContext,
) {
    let SceneReloadContext {
        mut scene_events,
        mut scenes,
        children,
        colliders,
        collider_markers,
        collider_constructors,
        mut players,
        frame_count,
    } = context;
    for event in scene_events.read() {
        // 逐个处理本帧 Scene 资产事件，支持多个场景文件连续保存。
        let AssetEvent::Modified { id } = event else {
            // 只有现有 Scene 内容修改需要强制重建。
            continue; // Added、Removed 等事件交给 Bevy 默认资产生命周期处理。
        }; // 完成修改事件筛选。
        for (scene_entity, hot_reload, pending, awaiting) in &mut scenes {
            // 在所有已登记场景根中寻找相同 Scene AssetId。
            if hot_reload.scene.id() != *id {
                // 当前场景没有使用被修改的 Scene 子资产。
                continue; // 跳过不相关场景，避免一次保存重建整个世界。
            } // 完成资产句柄匹配判断。
            if let Some(mut pending) = pending {
                pending.detected_frame = frame_count.0; // 多次文件替换合并到最新真实帧，第二阶段只读取最终场景层级。
                info!(
                    "场景 GLB 在跨帧等待期间再次修改，已合并到最新文件：{:?}",
                    hot_reload.scene
                );
                continue; // 旧碰撞已经移除，无需重复递归场景子树。
            }
            remove_scene_colliders_recursively(
                // 仅清理物理派生组件，场景实体生命周期完全交给 Bevy 原生热更新。
                scene_entity,             // 从已登记场景根开始递归访问全部 GLB 子实体。
                &children,              // 使用当前场景层级关系查找所有后代。
                &colliders,             // 判断每个后代是否持有需要清理的旧碰撞体。
                &collider_markers,      // 显式触发 Avian 接触图和物理岛清理 observer。
                &collider_constructors, // 判断每个后代是否持有尚未完成的旧构造器。
                &mut commands,          // 通过延迟命令安全移除 Avian 物理组件。
            ); // 完成当前场景旧碰撞清理安排。
            commands // 对匹配场景根安排跨帧碰撞重建状态。
                .entity(scene_entity) // 保留原 WorldAssetRoot、WorldInstance、子实体映射和场景专用 observer。
                .remove::<ColliderConstructorHierarchy>() // 清理可能存在的层级构造器，保证下一帧 Added 流程唯一。
                .remove::<SceneReloadAwaitingCollision>() // 构造期间再次修改时取消旧等待，迟到Ready事件不再拥有恢复资格。
                .insert(SceneReloadPending {
                    detected_frame: frame_count.0, // 下一真实帧才允许基于Bevy热更新后的新层级构造碰撞。
                });
            for (_player_entity, linear_velocity, angular_velocity) in &mut players {
                // 清空所有当前玩家速度，通常只有唯一玩家实体。
                if let Some(mut velocity) = linear_velocity {
                    // 玩家已具备线速度组件时才执行清零。
                    velocity.0 = Vec3::ZERO; // 清除下坠和移动速度，恢复物理后保持当前位置稳定。
                } // 结束线速度清零。
                if let Some(mut velocity) = angular_velocity {
                    // 玩家已具备角速度组件时才执行清零。
                    velocity.0 = Vec3::ZERO; // 清除旋转冲量，避免场景恢复时角色突然转动。
                } // 结束角速度清零。
            } // 完成玩家冻结遍历。
            info!(
                "场景 GLB {}，等待 Bevy 更新场景并安全重建碰撞：{:?}",
                if awaiting.is_some() {
                    "在碰撞构造期间再次修改"
                } else {
                    "已修改"
                },
                hot_reload.scene
            ); // 输出开发期热重建起点，便于排查资产监听。
        } // 完成已登记场景匹配遍历。
    } // 完成本帧 Scene 资产事件处理。
} // 结束场景修改检测系统。

/// 判断Modified检测帧是否已经结束；wrapping差值兼容长时间运行后的帧号回绕。
fn crossed_real_frame(detected_frame: u32, current_frame: u32) -> bool {
    current_frame.wrapping_sub(detected_frame) > 0 // 同一Update严格拒绝，下一真实帧及以后允许推进。
}

/// 在下一真实帧重新插入碰撞配置；F12暂停虚拟时间时该阶段仍会推进。
pub(super) fn respawn_pending_scenes(
    mut commands: Commands, // 恢复场景碰撞构造器组件，不接管 WorldAssetRoot 生命周期。
    frame_count: Res<FrameCount>, // 使用不受Time<Virtual>暂停影响的真实渲染帧身份。
    scenes: Query<(Entity, &HotReloadableScene, &SceneReloadPending)>, // 查询已经移除旧碰撞且等待跨帧的场景根。
) {
    for (scene_entity, hot_reload, pending) in &scenes {
        // 每个待处理场景根独立恢复自己的资产与碰撞策略。
        if !crossed_real_frame(pending.detected_frame, frame_count.0) {
            continue; // Modified同帧让Bevy场景更新与旧碰撞移除先完整落地，禁止立即重建旧层级。
        }
        commands // 安排场景第二阶段碰撞重建命令。
            .entity(scene_entity) // 复用 Bevy 已原地更新的场景根，不重复生成或删除 GLB 子实体。
            .insert((
                // 同一帧插入碰撞配置和等待状态。
                hot_reload.collision_hierarchy.clone(), // 扫描最新场景层级并重新触发专属 Avian 碰撞构造规则。
                SceneReloadAwaitingCollision,             // 阻止重复重载，并等待碰撞完成后恢复玩家。
            )) // 完成场景碰撞重建组件集合插入。
            .remove::<SceneReloadPending>(); // 第二阶段已安排，移除跨帧等待标记。
    } // 完成所有待重生场景处理。
} // 结束待重生场景恢复系统。

/// 把场景碰撞换代状态发布到公共暂停门；本场景域绝不直接写物理或虚拟时间。
pub(super) fn sync_scene_reload_pause_blocker(
    scenes: Query<(), Or<(With<SceneReloadPending>, With<SceneReloadAwaitingCollision>)>>, // 只扫描两个窄生命周期原型，不遍历场景子树。
    mut blockers: ResMut<WorldPauseBlockers>, // 游戏组合根在统一Apply阶段消费最终阻塞原因。
) {
    let blocked = !scenes.is_empty(); // Pending和Awaiting任一存在都表示世界暂时没有完整场景碰撞。
    if blockers.scene_collision_reload != blocked {
        blockers.scene_collision_reload = blocked; // 只在边沿写资源，稳定帧不持续触发变更检测。
    }
}

/// 场景完成碰撞构造后移除等待身份；公共暂停门在收集阶段观察最终集合并统一恢复时间。
pub(super) fn finish_scene_reload_collision(
    ready: On<ColliderConstructorHierarchyReady>, // 获取刚完成碰撞构造的场景根实体。
    mut commands: Commands,                       // 清理等待标记，暂停恢复仍由唯一时间写侧处理。
    awaiting: Query<(), With<SceneReloadAwaitingCollision>>, // 迟到旧构造和普通初载事件都不得消费当前生命周期。
) {
    if awaiting.get(ready.entity).is_err() {
        // 非热重建场景的初次碰撞完成事件不属于本流程。
        return; // 保持原场景启动 observer 独立工作，不干扰玩家状态。
    } // 完成热重建事件来源校验。
    commands
        .entity(ready.entity)
        .remove::<SceneReloadAwaitingCollision>(); // 当前场景碰撞已恢复，移除等待标记。
    info!("一个场景 GLB 场景已由 Bevy 更新，碰撞已完成安全热重建"); // 公共阻塞只在全部Pending/Awaiting消失后由同步系统解除。
} // 结束场景碰撞完成 observer。

/// 递归移除场景层级中的旧碰撞与未完成构造器，不删除任何由 Bevy SceneSpawner 管理的实体。
fn remove_scene_colliders_recursively(
    entity: Entity,                                     // 当前递归访问的场景根或后代实体。
    children: &Query<&Children>,                        // 提供当前实体的直接子实体关系。
    colliders: &Query<(), With<Collider>>,              // 判断当前实体是否持有旧碰撞体。
    collider_markers: &Query<(), With<ColliderMarker>>, // 判断当前实体是否仍登记在 Avian 接触图和物理岛中。
    collider_constructors: &Query<(), With<ColliderConstructor>>, // 判断当前实体是否持有未完成构造器。
    commands: &mut Commands, // 安排物理组件移除命令并保留场景组件。
) {
    if colliders.contains(entity) {
        // 当前实体持有根据旧网格生成的碰撞体。
        if collider_markers.contains(entity) {
            // Avian 仍把当前碰撞登记在内部图结构中。
            commands.entity(entity).remove::<ColliderMarker>(); // 先同步触发窄相位 observer，完整解除旧 contact 与 island 链接。
        } // 完成 Avian 内部碰撞标记清理安排。
        commands.entity(entity).remove::<Collider>(); // 通过 Avian 正常组件移除路径清理接触与宽相位数据。
    } // 完成旧碰撞体移除安排。
    if collider_constructors.contains(entity) {
        // 当前实体仍有尚未消费的单网格构造器。
        commands.entity(entity).remove::<ColliderConstructor>(); // 防止它在新场景层级完成前根据旧网格生成碰撞。
    } // 完成旧单网格构造器移除安排。
    let Ok(entity_children) = children.get(entity) else {
        // 没有子实体时递归已经到达叶节点。
        return; // 叶节点完成处理后返回上一层。
    }; // 完成子实体存在性判断。
    for child in entity_children.iter() {
        // 遍历全部直接子实体，覆盖 GLB 任意深度层级。
        remove_scene_colliders_recursively(
            // 对当前子树重复执行仅物理组件清理。
            child,                 // 进入当前直接子实体。
            children,              // 复用只读层级查询。
            colliders,             // 复用旧碰撞体查询。
            collider_markers,      // 复用 Avian 内部碰撞标记查询。
            collider_constructors, // 复用旧构造器查询。
            commands,              // 汇总到同一 ECS 命令队列。
        ); // 完成当前子树递归调用。
    } // 完成直接子实体遍历。
} // 结束场景旧碰撞递归清理 helper。

#[cfg(test)] // 热重载跨帧和暂停回归只在cargo test构建中启用。
mod tests {
    use super::*; // 直接运行正式第二阶段系统，避免复制调度逻辑。

    #[test] // F12暂停虚拟时间时，真实Update仍必须在下一帧恢复碰撞构造器。
    fn pending_reload_advances_on_real_update_while_virtual_time_is_paused() {
        let mut app = App::new(); // 最小ECS应用不安装FixedMain，专门证明本流程不再依赖固定时钟。
        app.insert_resource(FrameCount(17)); // 模拟Modified发生的真实渲染帧。
        let mut virtual_time = Time::<Virtual>::default(); // 建立与F12一致的虚拟时间资源。
        virtual_time.pause(); // F12打开期间FixedMain不会推进。
        app.insert_resource(virtual_time); // 暂停资源存在但Update调度仍可运行。
        app.init_resource::<WorldPauseBlockers>(); // 同时验证碰撞生命周期投影到唯一暂停门。
        app.add_systems(
            Update,
            (respawn_pending_scenes, sync_scene_reload_pause_blocker).chain(), // 注册正式真实帧第二阶段和公共阻塞发布。
        );
        let scene_entity = app
            .world_mut()
            .spawn((
                HotReloadableScene::new(
                    Handle::<WorldAsset>::default(), // 测试只需要稳定Scene句柄身份。
                    ColliderConstructorHierarchy::new(None), // 空规则足够验证组件状态迁移。
                ),
                SceneReloadPending {
                    detected_frame: 17, // 当前帧不得立即重建。
                },
            ))
            .id(); // 保存场景根供两帧断言。

        app.update(); // 同一真实帧执行第二阶段，应继续等待。
        assert!(
            app.world()
                .entity(scene_entity)
                .contains::<SceneReloadPending>()
        ); // 保留跨帧标记。
        assert!(
            !app.world()
                .entity(scene_entity)
                .contains::<SceneReloadAwaitingCollision>()
        ); // Modified同帧不能提前宣称正在构造新碰撞。

        app.world_mut().resource_mut::<FrameCount>().0 = 18; // 模拟Last已把真实帧号推进一次。
        app.update(); // 即使虚拟时间仍暂停，普通Update也必须完成第二阶段。
        let map = app.world().entity(scene_entity); // 读取命令应用后的最终场景根。
        assert!(!map.contains::<SceneReloadPending>()); // 跨帧等待已经消费。
        assert!(map.contains::<SceneReloadAwaitingCollision>()); // 正式状态开始等待Avian完成事件。
        assert!(map.contains::<ColliderConstructorHierarchy>()); // 最新场景层级重新取得碰撞构造规则。
        assert!(app.world().resource::<Time<Virtual>>().is_paused()); // 整个过程没有临时恢复游戏时间。
        assert!(
            app.world()
                .resource::<WorldPauseBlockers>()
                .scene_collision_reload
        ); // Awaiting仍存在时游戏组合根必须继续暂停玩法世界。
        app.world_mut()
            .entity_mut(scene_entity)
            .remove::<SceneReloadAwaitingCollision>(); // 模拟Avian完成观察器已经清理最后一个等待身份。
        app.update();
        assert!(
            !app.world()
                .resource::<WorldPauseBlockers>()
                .scene_collision_reload
        ); // 最后一个身份消失后只释放公共原因，不直接改写任何时间资源。
    }

    #[test] // Awaiting期间再次保存GLB必须取消旧构造并重新等待最新文件，不能吞掉Modified。
    fn modified_during_collision_build_restarts_from_the_latest_scene() {
        let mut app = App::new(); // 最小消息和ECS世界即可运行正式Modified入口。
        app.insert_resource(FrameCount(31)); // 本次连续保存发生在真实帧31。
        app.add_message::<AssetEvent<WorldAsset>>(); // 代替AssetPlugin提供标准Scene生命周期消息。
        app.init_resource::<WorldPauseBlockers>(); // 验证重启期间阻塞原因不中断。
        app.add_systems(
            Update,
            (begin_modified_scene_reloads, sync_scene_reload_pause_blocker).chain(),
        );
        let scene = Handle::<WorldAsset>::default(); // 事件与场景根共享同一Scene 0身份。
        let scene_entity = app
            .world_mut()
            .spawn((
                HotReloadableScene::new(scene.clone(), ColliderConstructorHierarchy::new(None)),
                ColliderConstructorHierarchy::new(None), // 模拟上一版碰撞正在构造。
                SceneReloadAwaitingCollision,              // 上一次保存尚未完成。
            ))
            .id();
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<WorldAsset>>>()
            .write(AssetEvent::Modified { id: scene.id() }); // 第二次保存发布相同Scene身份的新修订。

        app.update(); // 正式入口必须把旧Awaiting原子换回Pending。
        let map = app.world().entity(scene_entity);
        assert!(map.contains::<SceneReloadPending>()); // 最新文件重新经过真实跨帧门。
        assert!(!map.contains::<SceneReloadAwaitingCollision>()); // 旧构造完成值已失去采用资格。
        assert!(!map.contains::<ColliderConstructorHierarchy>()); // 旧构造器被取消，下一帧只登记最新一次。
        assert_eq!(
            map.get::<SceneReloadPending>()
                .expect("连续保存必须保留最新Pending身份")
                .detected_frame,
            31
        ); // 不能沿用第一次保存的旧检测帧。
        assert!(
            app.world()
                .resource::<WorldPauseBlockers>()
                .scene_collision_reload
        ); // Awaiting→Pending切换期间暂停原因连续保持，不泄漏世界步。
    }

    #[test] // 帧号回绕也必须把下一真实帧识别为可推进。
    fn real_frame_gate_handles_u32_wraparound() {
        assert!(!crossed_real_frame(9, 9)); // 同一帧严格等待。
        assert!(crossed_real_frame(9, 10)); // 普通下一帧允许推进。
        assert!(crossed_real_frame(u32::MAX, 0)); // 最大帧号回绕到零仍属于下一帧。
    }
}
