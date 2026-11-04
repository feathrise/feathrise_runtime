//! 场景细节质量的新增节点观察与变化驱动应用系统。

use bevy::camera::visibility::VisibilityRange; // 所有视觉 LOD 都通过 Bevy 原生范围参与主视图和阴影视图剔除。
use bevy::prelude::*; // On、Add、Query、Commands 与资源访问接口。
use feathrise_environment::landscape::{LandscapeLodLevel, LandscapeLodMesh}; // 旧景观路径的四档稳定身份。
use feathrise_environment::tree::{TreeLodLevel, TreeLodMesh}; // 旧树木路径的高、中、低、公告板身份。
use feathrise_environment::vegetation::{
    VegetationMeshBatchNode, VegetationMeshBatchQuality, VegetationMeshLodNode,
}; // 逐实例旧路径与分块批次分别接收同一场景细节倍率。
use feathrise_settings::RenderSettings; // 读取持久场景细节档。

use super::helpers::visibility_range_for_quality; // 所有路径复用同一不可累计的纯派生规则。
use crate::components::SceneDetailAuthoredVisibilityRange; // 保存作者范围和最远档身份。
use crate::resources::SceneDetailQualityState; // 档位未变时阻止全量节点扫描。

/// 单帧最多更新的固定景观 LOD 节点数；当前 HN-01 硬预算低于该值，未来扩容则自动分帧。
const SCENE_DETAIL_NODE_BATCH: usize = 8192; // 固定上界防止内容规模无条件放大切档帧成本。

/// 判断一个新增可见范围是否属于树木、灌木或岩石 LOD，并返回其最远档身份。
fn farthest_scene_detail_lod(
    tree: Option<&TreeLodMesh>,           // 旧树木路径可能存在的 LOD 身份。
    landscape: Option<&LandscapeLodMesh>, // 旧灌木或岩石路径可能存在的 LOD 身份。
    mesh: Option<&VegetationMeshLodNode>, // 通用 Mesh 接管路径可能存在的 LOD 身份。
) -> Option<bool> {
    if let Some(tree) = tree {
        return Some(tree.level == TreeLodLevel::Billboard); // 旧树木只有公告板使用独立最远退出倍率。
    } // 完成旧树木分类。
    if let Some(landscape) = landscape {
        return Some(landscape.level == LandscapeLodLevel::Ultra); // 灌木与岩石的超低档是第四个最远节点。
    } // 完成旧固定景观分类。
    mesh.map(|mesh| mesh.level.0 == 3) // 当前正式通用原型统一以第 4 档作为树、灌木和岩石最远节点。
} // 完成内容身份到最远档布尔值的纯分类。

/// 把首次接管的节点加入稳定索引，重复观察不会产生重复实体。
fn register_scene_detail_node(state: &mut SceneDetailQualityState, entity: Entity) {
    if state.indices.contains_key(&entity) {
        return; // 范围或身份重复安装时保持唯一登记。
    } // 完成重复实体快速路径。
    let index = state.entities.len(); // 新实体稳定追加到当前批处理尾部。
    state.entities.push(entity); // 切档系统之后可按连续切片直接定位组件。
    state.indices.insert(entity, index); // 销毁时通过下标 O(1) 移除。
} // 完成场景细节节点登记。

/// 从稳定索引移除已销毁节点，切档进行中则从头重核一次以覆盖 swap 进来的实体。
fn unregister_scene_detail_node(state: &mut SceneDetailQualityState, entity: Entity) {
    let Some(index) = state.indices.remove(&entity) else {
        return; // 未接管范围或重复移除事件无需处理。
    }; // 完成登记身份读取。
    let removed = state.entities.swap_remove(index); // O(1) 删除并把最后一个实体补到空位。
    debug_assert_eq!(removed, entity); // 索引表和稳定向量必须保持同一所有权。
    if index < state.entities.len() {
        let moved = state.entities[index]; // 取得刚从末尾换入当前下标的实体。
        state.indices.insert(moved, index); // 修正其 O(1) 反向索引。
    } // 删除原末尾时无需修正任何实体。
    if state.pending_quality.is_some() {
        state.cursor = 0; // 进行中删除可能把未处理实体换到已处理区，从头幂等重核最安全。
    } // 稳定状态删除不触发任何范围更新。
} // 完成场景细节节点注销。

/// 新增任意 `VisibilityRange` 时，仅接管同时具有固定景观 LOD 身份的节点。
#[allow(clippy::type_complexity)] // 三条互斥 LOD 身份路径与作者备份必须在同一观察器快照中判定。
pub(super) fn capture_scene_detail_visibility_range(
    add: On<Add, VisibilityRange>, // 观察每个新范围，避免后续场景或热重载需要全量轮询。
    settings: Res<RenderSettings>,   // 新节点直接继承 UI 当前档位而非等待下一次切档。
    mut commands: Commands,        // 延迟插入不可变作者备份，不干扰生成系统的实体借用。
    mut nodes: Query<(
        &mut VisibilityRange,
        Option<&TreeLodMesh>,
        Option<&LandscapeLodMesh>,
        Option<&VegetationMeshLodNode>,
        Option<&VegetationMeshBatchNode>,
        Option<&SceneDetailAuthoredVisibilityRange>,
    )>, // 一个观察器覆盖旧树、旧景观和新通用 Mesh 三条互斥路径。
) {
    let Ok((mut current, tree, landscape, mesh, batch, existing_backup)) =
        nodes.get_mut(add.entity)
    else {
        return; // 组件已移除或触发实体不可查询时安全跳过。
    }; // 完成新增实体读取。
    if batch.is_some() {
        return; // 批次节点的占位范围只为PBR接口专用化；真实逐株作者范围由批次质量资源派生。
    }
    let Some(farthest_lod) = farthest_scene_detail_lod(tree, landscape, mesh) else {
        return; // 相机或其他系统的 VisibilityRange 不属于本设置作用域。
    }; // 完成固定景观范围过滤。
    let authored = existing_backup
        .map(|backup| backup.authored.clone())
        .unwrap_or_else(|| current.clone()); // 重复身份安装时仍从首次作者值派生，绝不对已缩放值累计缩放。
    if existing_backup.is_none() {
        commands
            .entity(add.entity)
            .insert(SceneDetailAuthoredVisibilityRange {
                authored: authored.clone(), // 高档恢复和后续切档共同使用同一不可变作者值。
                farthest_lod,               // 保存分类结果，切档系统无需再读取三类业务组件。
            }); // 完成当前节点首次接管登记。
    } // 已有备份表示范围组件被重装，保持首次作者所有权不变。
    let target = visibility_range_for_quality(
        &authored,
        farthest_lod,
        settings.graphics.scene_detail_quality,
    ); // 从作者值直接派生当前档目标。
    if *current != target {
        *current = target; // 新节点只在非高档或实际不一致时触发一次 Bevy 可见性更新。
    } // 高档新节点保持生成系统原值，不产生无意义组件变更。
} // 完成新增固定景观 LOD 节点即时继承。

/// 作者备份真实落地后才把节点加入批处理索引。
pub(super) fn register_scene_detail_visibility_range(
    add: On<Add, SceneDetailAuthoredVisibilityRange>, // 只登记已完成作者范围备份的节点。
    mut state: ResMut<SceneDetailQualityState>,       // 增量维护实体向量与 O(1) 反向索引。
) {
    register_scene_detail_node(&mut state, add.entity); // 瞬时生成后未落地备份的实体不会污染索引。
} // 完成已接管节点登记观察器。

/// 作者备份移除或实体销毁时同步回收批处理索引。
pub(super) fn unregister_scene_detail_visibility_range(
    remove: On<Remove, SceneDetailAuthoredVisibilityRange>, // 只观察本系统实际接管过的节点。
    mut state: ResMut<SceneDetailQualityState>,             // O(1) 更新实体向量与反向索引。
) {
    unregister_scene_detail_node(&mut state, remove.entity); // 实体已不可查询也能凭事件身份安全移除。
} // 完成切图、热替换与单节点销毁索引回收。

/// 档位变化时按固定批量重算已登记节点，稳定帧仅比较一个枚举后返回。
pub(super) fn apply_scene_detail_quality(
    settings: Res<RenderSettings>, // UI 和 settings.json 共享的权威画质资源。
    mut batch_quality: ResMut<VegetationMeshBatchQuality>, // 批次不登记成千上万个逐株范围，只同步两个有限倍率。
    mut state: ResMut<SceneDetailQualityState>,            // 记录上次完成整批应用的档位。
    mut nodes: Query<(&SceneDetailAuthoredVisibilityRange, &mut VisibilityRange)>, // 只按资源中的实体切片定位组件，不重新扫描整个 ECS 世界。
) {
    let quality = settings.graphics.scene_detail_quality; // 复制轻量枚举供整批计算。
    let batch_multipliers = quality.runtime_multipliers();
    if batch_quality.multipliers() != batch_multipliers {
        batch_quality.set_multipliers(batch_multipliers.0, batch_multipliers.1); // 变化检测只在真实切档时触发批次LOD重建。
    }
    if state.pending_quality != Some(quality) && state.last_applied != Some(quality) {
        state.pending_quality = Some(quality); // 任意新目标都建立一轮从头开始的幂等更新。
        state.last_applied = None; // 处理中不能宣称某一档已经覆盖全部登记节点。
        state.cursor = 0; // 玩家快速连续切档时直接从作者范围重新应用最新目标。
    } // 完成新一轮质量变更初始化。
    if state.pending_quality.is_none() {
        return; // 稳定帧不遍历数千 LOD 节点，也不触发 Bevy 变更检测。
    } // 完成 O(1) 稳定帧快速路径。
    let batch_end = state
        .cursor
        .saturating_add(SCENE_DETAIL_NODE_BATCH)
        .min(state.entities.len()); // 每帧成本由固定常量封顶，空地图得到零长度批次。
    {
        let entities = &state.entities[state.cursor..batch_end]; // 借用连续实体切片，不产生每帧临时分配。
        let mut batch = nodes.iter_many_mut(entities); // 直接定位本批节点，跳过期间已失效实体。
        while let Some((backup, mut current)) = batch.fetch_next() {
            let target =
                visibility_range_for_quality(&backup.authored, backup.farthest_lod, quality); // 每次都从作者值重算，避免多次切档累计误差。
            if *current != target {
                *current = target; // 只有真实范围变化才触发主视图与阴影视图可见性更新。
            } // 已由新增观察器应用当前档的节点不会被首帧重复改写。
        } // 完成本帧固定上限内的全部有效节点。
    } // 释放实体切片借用后再推进队列游标。
    state.cursor = batch_end; // 下一帧从本批末尾继续，不重复遍历已处理实体。
    if state.cursor >= state.entities.len() {
        state.last_applied = Some(quality); // 全部登记节点完成后才发布稳定档位。
        state.pending_quality = None; // 清空待处理标记，后续稳定帧恢复 O(1) 快速路径。
        state.cursor = 0; // 为下一次切档保留确定性起点。
    } // 超出单批预算的未来场景自动在后续帧继续处理。
} // 完成变化驱动场景细节同步。

#[cfg(test)]
mod tests {
    use super::*; // 使用正式观察器、系统与三个公共 LOD 身份构造最小 ECS 回归。
    use feathrise_environment::landscape::LandscapeKind; // 构造灌木或岩石旧路径身份。
    use feathrise_environment::tree::TreeMeshPart; // 构造树冠旧路径身份。
    use feathrise_environment::vegetation::VegetationMeshLodLevel; // 构造正式通用第 4 档身份。
    use feathrise_settings::SceneDetailQuality; // 测试运行时低、极高和高档切换。

    use super::super::SceneDetailQualityPlugin; // 测试完整插件注册的观察器和 PostUpdate 系统。

    fn authored_range(start: (f32, f32), end: (f32, f32)) -> VisibilityRange {
        VisibilityRange {
            start_margin: start.0..start.1, // 保存代表性宽域淡入区。
            end_margin: end.0..end.1,       // 保存代表性宽域淡出区。
            use_aabb: false,                // 正式树木和景观统一按根位置测距。
        } // 完成测试作者范围。
    } // 完成 ECS fixture 纯构造函数。

    fn assert_margin_close(actual: &std::ops::Range<f32>, expected: std::ops::Range<f32>) {
        assert!((actual.start - expected.start).abs() <= 0.0001); // 容纳十进制质量倍率的 f32 末位误差。
        assert!((actual.end - expected.end).abs() <= 0.0001); // 仍以万分之一米锁定运行时边界。
    } // 完成 ECS 测试专用区间近似断言。

    fn quality_app(quality: SceneDetailQuality) -> App {
        let mut app = App::new(); // 无窗口主世界足以验证 Bevy 可见范围组件。
        app.add_plugins(SceneDetailQualityPlugin); // 安装与生产相同的观察和变化驱动路径。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .scene_detail_quality = quality; // 在生成节点前模拟已加载持久设置。
        app // 返回可继续生成三类节点的最小应用。
    } // 完成测试应用构造。

    #[test]
    fn legacy_and_generic_nodes_inherit_low_quality_when_spawned() {
        let mut app = quality_app(SceneDetailQuality::Low); // 新节点应在生成时直接继承六成范围。
        let tree = app
            .world_mut()
            .spawn((
                TreeLodMesh {
                    level: TreeLodLevel::Billboard, // 旧树最远公告板。
                    part: TreeMeshPart::Billboard,  // 公告板单部件身份。
                },
                authored_range((360.0, 400.0), (1050.0, 1120.0)),
            ))
            .id(); // 完成旧树最远节点生成。
        let landscape = app
            .world_mut()
            .spawn((
                LandscapeLodMesh {
                    kind: LandscapeKind::Rock,      // 使用旧岩石路径代表固定景观。
                    level: LandscapeLodLevel::High, // 非最远档的两段边界都服从 LOD 倍率。
                },
                authored_range((0.0, 0.0), (45.0, 60.0)),
            ))
            .id(); // 完成旧岩石高档节点生成。
        let generic = app
            .world_mut()
            .spawn((
                VegetationMeshLodNode {
                    level: VegetationMeshLodLevel(3), // 正式通用原型第 4 档是最远节点。
                    part_id: 0,                       // 单部件 fixture。
                    billboard: true,                  // 最远树节点可使用 GPU 公告板。
                },
                authored_range((240.0, 280.0), (320.0, 380.0)),
            ))
            .id(); // 完成通用最远节点生成。
        app.update(); // 执行首个 PostUpdate 并落地作者备份。
        assert_margin_close(
            &app.world().get::<VisibilityRange>(tree).unwrap().end_margin,
            630.0..672.0,
        ); // 旧树最远退出边界缩到六成。
        assert_margin_close(
            &app.world()
                .get::<VisibilityRange>(landscape)
                .unwrap()
                .end_margin,
            27.0..36.0,
        ); // 非最远旧景观 LOD 边界同样缩到六成。
        assert_margin_close(
            &app.world()
                .get::<VisibilityRange>(generic)
                .unwrap()
                .end_margin,
            192.0..228.0,
        ); // 通用第 4 档最远边界继承低档倍率。
        assert!(
            app.world()
                .get::<SceneDetailAuthoredVisibilityRange>(generic)
                .is_some(),
            "通用新节点必须保存作者范围供后续切档恢复",
        ); // 锁定通用接管路径也拥有不可变作者备份。
    } // 完成旧树、旧景观和通用 Mesh 新节点继承覆盖。

    #[test]
    fn oversized_future_scene_is_updated_in_fixed_batches_and_despawn_unregisters_nodes() {
        let mut app = quality_app(SceneDetailQuality::High); // 先以作者基线生成超过单帧预算一个节点的未来场景。
        let authored = authored_range((0.0, 0.0), (100.0, 120.0)); // 非最远档便于识别低档 LOD 边界变化。
        let mut entities = Vec::with_capacity(SCENE_DETAIL_NODE_BATCH + 1); // 测试分配只发生在测试进程，不进入生产帧路径。
        for part_id in 0..=SCENE_DETAIL_NODE_BATCH {
            entities.push(
                app.world_mut()
                    .spawn((
                        VegetationMeshLodNode {
                            level: VegetationMeshLodLevel(0), // 全部节点属于非最远高模档。
                            part_id: part_id as u32,          // 唯一部件号仅用于 fixture 区分。
                            billboard: false,                 // 普通实体 LOD 不需要公告板能力。
                        },
                        authored.clone(),
                    ))
                    .id(),
            ); // 新增观察器同步登记每个节点并保持高档作者范围。
        } // 完成超过单批上限的固定节点集合。
        app.update(); // 高档首批完成，最后一个节点仍由新增观察器正确保持高档。
        app.update(); // 第二批收口并发布稳定高档状态。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .scene_detail_quality = SceneDetailQuality::Low; // 模拟未来超大场景运行时切低档。
        app.update(); // 只允许第一批固定数量节点改变。
        assert_margin_close(
            &app.world()
                .get::<VisibilityRange>(entities[0])
                .unwrap()
                .end_margin,
            60.0..72.0,
        ); // 第一批首节点已经应用低档。
        assert_eq!(
            app.world()
                .get::<VisibilityRange>(*entities.last().unwrap())
                .unwrap()
                .end_margin,
            authored.end_margin,
        ); // 超出批量的最后节点在本帧保持作者范围，不得突破固定预算。
        app.update(); // 下一帧完成剩余一个节点。
        assert_margin_close(
            &app.world()
                .get::<VisibilityRange>(*entities.last().unwrap())
                .unwrap()
                .end_margin,
            60.0..72.0,
        ); // 第二批必须完成未来扩容场景的最终一致状态。
        let removed = entities[0]; // 选择已完成切档的登记节点模拟切图销毁。
        let _ = app.world_mut().despawn(removed); // Remove 观察器应同步回收反向索引。
        let state = app.world().resource::<SceneDetailQualityState>();
        assert_eq!(state.entities.len(), SCENE_DETAIL_NODE_BATCH); // 销毁后登记总数精确减少一个。
        assert!(!state.indices.contains_key(&removed)); // 反向索引不得保留失效实体。
    } // 完成固定批量上限与 O(1) 注销覆盖。

    #[test]
    fn quality_changes_restore_authored_high_and_stable_frames_do_not_rewrite() {
        let mut app = quality_app(SceneDetailQuality::Ultra); // 极高先延长 LOD，保持最远退出边界。
        let authored = authored_range((240.0, 280.0), (320.0, 380.0)); // 使用灌木最远档代表范围。
        let entity = app
            .world_mut()
            .spawn((
                LandscapeLodMesh {
                    kind: LandscapeKind::Shrub,      // 旧灌木路径。
                    level: LandscapeLodLevel::Ultra, // 第 4 档使用独立最大范围倍率。
                },
                authored.clone(),
            ))
            .id(); // 完成运行时切档 fixture。
        app.update(); // 应用极高档。
        let ultra = app.world().get::<VisibilityRange>(entity).unwrap();
        assert_eq!(ultra.start_margin, 264.0..308.0); // 极高延长精细 LOD 交接 10%。
        assert_eq!(ultra.end_margin, authored.end_margin); // 极高保持作者最远上限。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .scene_detail_quality = SceneDetailQuality::High; // 运行时切回严格作者基线。
        app.update(); // 变化帧重算已接管节点。
        assert!(
            app.world().get::<VisibilityRange>(entity).unwrap() == &authored,
            "高档必须完整恢复首次作者范围",
        ); // VisibilityRange 没有 Debug，使用显式相等断言锁定作者值恢复。
        app.world_mut().clear_trackers(); // 清除切档帧合法写入产生的变更标记。
        app.update(); // 同一高档稳定帧再次运行插件。
        assert!(
            !app.world()
                .entity(entity)
                .get_ref::<VisibilityRange>()
                .expect("稳定帧固定景观节点应仍存在")
                .is_changed(),
            "稳定帧不得重复写入 VisibilityRange",
        ); // 锁定 O(1) 快速路径不会持续触发可见性重算。
    } // 完成切档恢复与稳定帧幂等覆盖。
} // 完成场景细节 ECS 测试。
