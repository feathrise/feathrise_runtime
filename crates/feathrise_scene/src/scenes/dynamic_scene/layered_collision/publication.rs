//! 完整碰撞集合的预检和唯一原子发布点；失败前不发出任何ECS变更。
use super::super::resources::DynamicSceneRuntimeState; // 场景首次就绪。
use super::{planning, resources::*}; // 保留块与几何完整性预算，不再按固定源页数拒绝有效候选。
use crate::components::DynamicSparseTerrainCollider;
use avian3d::prelude::{CollisionLayers, RigidBody}; // 唯一公共地形层。
use bevy::prelude::*; // 主世界组件写入。
use feathrise_scene_core::residency::TerrainCollisionRegion; // 活动门读取同版覆盖。
use feathrise_scene_content::SceneCollisionLodStatus; // 分离已保存与已采用。
use std::{collections::BTreeSet, sync::Arc}; // 完整集合去重与共享请求。 // 公共地面查询标记。

fn validate_candidate(state: &LayeredCollisionRuntime, cooked: &Cooked) -> Result<(), String> {
    // 扫描最多512块，无顶点采样。
    let request = state.desired.as_ref().ok_or("碰撞候选没有当前目标")?; // 不允许无身份提交。
    if state.scene_id.as_ref() != Some(&request.scene_id) || state.root != Some(request.root) {
        return Err("碰撞候选场景或根身份已经失效".into());
    } // 防止跨会话采用。
    if cooked.plans.len() > planning::MAX_CHUNKS
        || cooked.shapes.len() > cooked.plans.len()
        || cooked.bounds.len() != cooked.plans.len()
        || cooked.supported_polygon.len() > 4096
    // 圆弧细分硬预算与后台构建一致。
    {
        return Err("碰撞候选集合或源页超过预算、不完整".into());
    } // 对后台完整性再验收。
    if cooked.plans.values().any(|plan| plan.cells.is_some())
        && cooked
            .supported_rows
            .as_ref()
            .is_none_or(|rows| rows.rows.is_empty() || rows.rows.len() > 524288)
    {
        // 显式完整格集合必须同时发布有效有界行索引。
        return Err("碰撞候选缺少完整格承托索引或超过预算".into()); // 不发布包围盒伪承托。
    } // 完整几何与查询合同一起发布。
    let mut replacements = BTreeSet::new(); // 重复新块会破坏原子集合语义。
    for (plan, _, _) in &cooked.shapes {
        // 不读取形状内部三角面。
        if !replacements.insert(plan.key) || cooked.plans.get(&plan.key) != Some(plan) {
            return Err("碰撞候选包含重复或不属于目标的替换块".into());
        } // 目标与替换须完全一致。
    } // 新形状全部在目标集合内。
    for ((key, plan), (lo, hi)) in cooked.plans.iter().zip(&cooked.bounds) {
        // bounds由有序布局生成，必须一一对应。
        if *key != plan.key || key.level > request.spec.max_mip_level() {
            return Err("碰撞候选块身份无效".into());
        } // 不让损坏键进入移位或边界换算。
        if !replacements.contains(key)
            && state
                .resident
                .get(key)
                .is_none_or(|resident| resident.plan != *plan)
        {
            return Err("碰撞候选缺少新形状且不存在可复用的同版块".into());
        } // 不能把未构建区域标记成有承托。
        let (a, b) = planning::chunk_bounds(&request.spec, *key); // 与真实网格同源。
        if a[0] >= b[0] || a[1] >= b[1] {
            return Err("碰撞候选包含场景外或空块".into());
        } // 有限场景必须裁剪。
        let world_a = request
            .spec
            .grid_point_world(a[0], a[1])
            .ok_or("碰撞候选下界越界")?; // 权威坐标。
        let world_b = request
            .spec
            .grid_point_world(b[0], b[1])
            .ok_or("碰撞候选上界越界")?; // 权威坐标。
        if *lo != Vec2::new(world_a[0] as f32, world_a[1] as f32)
            || *hi != Vec2::new(world_b[0] as f32, world_b[1] as f32)
        {
            return Err("碰撞候选承托边界与几何不一致".into());
        } // 不以伪覆盖提前释放角色。
    } // 首条命令尚未产生。
    Ok(()) // 完整预检后才允许提交。
} // 所有错误都保留旧有效集合。

pub(super) fn preserves_player_ground(region: &TerrainCollisionRegion, cooked: &Cooked) -> bool {
    // 玩家掉头后不能用落后的候选撤掉仍有效的脚下地面。
    let center = region.center; // 当前玩家位置，而非冻结候选圆心。
    let old_supports = region.support_polygon_contains(center)
        && region
            .supported_bounds
            .iter()
            .any(|(lo, hi)| center.cmpge(*lo).all() && center.cmple(*hi).all()); // 只保护真实已采用地面。
    let new_supports = cooked.supported_rows.as_ref().map_or_else(
        || {
            cooked
                .bounds
                .iter()
                .any(|(lo, hi)| center.cmpge(*lo).all() && center.cmple(*hi).all())
        },
        |rows| rows.contains(center),
    ); // 新完整格使用真实行覆盖，不用管理盒填洞。
    !old_supports || new_supports // 初次进入仍可完成第一批；实际脚印和下一步预测继续由物理前活动门检查。
} // 固定最多512块查询，不读取三角形。

pub(super) fn adopt(
    commands: &mut Commands,
    state: &mut LayeredCollisionRuntime,
    region: &mut TerrainCollisionRegion,
    feedback: &mut SceneCollisionLodStatus,
    runtime: &mut DynamicSceneRuntimeState,
    cooked: Cooked,
) -> Result<(), String> {
    // 单一发布点位于物理步之前。
    if let Err(error) = validate_candidate(state, &cooked) {
        // 所有校验在第一个ECS命令前完成。
        super::retirement::completion(state, Completion::Cooked(Ok(cooked))); // 错误候选仍可能拥有大型规划/形状，后台析构。
        return Err(error); // 失败不改变旧集合和已采用缓存。
    } // 完整校验保证下面存在desired。
    let request = Arc::clone(state.desired.as_ref().expect("发布预检已确认碰撞目标")); // 同一独占状态借用内身份不可能消失，避免额外按值错误析构路径。
    let rebuilt = cooked.shapes.len(); // 诊断反映真实形状构造数量。
    feedback.peak_shape_count = feedback
        .peak_shape_count
        .max(state.resident.len() + rebuilt); // 旧形状与后台候选同时存活，单独统计替换期间峰值。
    for (plan, stamp, collider) in cooked.shapes {
        // 先安装所有新形状。
        let entity = if let Some(old) = state.resident.get(&plan.key) {
            commands.entity(old.entity).insert(collider);
            old.entity
        } else {
            // 原位替换或新建固定叶。
            let entity = commands
                .spawn((
                    Name::new("LayeredSharedTerrainCollision"),
                    DynamicSparseTerrainCollider,
                    RigidBody::Static,
                    collider,
                    CollisionLayers::DEFAULT,
                    Transform::IDENTITY,
                ))
                .id(); // 世界坐标形状、所有实体共享地面。
            commands.entity(request.root).add_child(entity); // 切换场景随场景根回收。
            entity // 记录新的实体身份。
        }; // 新旧形状不会在同一物理步并存。
        // 不覆盖 DebugRender：F1 沿用 Avian 原生绘制完整真实碰撞网格，不以自定义块轮廓替代。
        state.resident.insert(
            plan.key,
            Resident {
                entity,
                plan,
                stamp,
            },
        ); // 更新实际块缓存。
    } // 接缝和主体已经全部排入同一命令提交。
    state.resident.retain(|key, resident| {
        if cooked.plans.contains_key(key) {
            true
        } else {
            commands.entity(resident.entity).try_despawn();
            false
        }
    }); // 撤销离开或被细分的父块。
    state.retained_sources = cooked.sources; // 维持源身份直到下一代比较完成。
    state.retained_craters = cooked.craters; // 会话源同理。
    state.applied = Some(Arc::clone(&request)); // 完整采用身份。
    if let Some(layout) = &cooked.layout {
        // 只有成功完整提交的布局可作为下一轮复用基线。
        feedback.layout_milliseconds = layout.elapsed_ms; // 包含规划与逐块布局。
        feedback.layout_rebuilt_blocks = layout.rebuilt_blocks; // 与形状重建分开。
        feedback.layout_reused_blocks = layout.reused_blocks; // 真正跳过网格生成的块。
    } // 失败和迟到候选不会改写这些已采用统计。
    let old_layout = std::mem::replace(&mut state.applied_layout, cooked.layout); // 新完整快照原子成为下轮基线。
    super::retirement::applied(state, old_layout); // 旧版最后Arc不能在主帧递归释放规划组。
    feedback.request_to_adopt_milliseconds = state
        .request_started
        .map_or(0.0, |start| start.elapsed().as_secs_f32() * 1000.0); // 包含等待IO和任务调度的真实端到端时间。
    feedback.adopted_center_lag_meters = region.center.distance(request.anchor); // 发布时反映当前玩家而非冻结位置。
    region.supported_bounds = cooked.bounds; // 活动门只见完整集合。
    region.supported_polygon = cooked.supported_polygon; // 圆外块包围盒不再被当作已存在的真实地面。
    region.supported_rows = cooked.supported_rows; // 与碰撞体同一步完整采用。
    region.max_distance_meters = request
        .lods
        .last()
        .map_or(0.0, |lod| lod.max_distance_meters); // 外扩块不扩大玩法活动距离。
    region.revision = region.revision.wrapping_add(1); // 触发承托复核。
    feedback.applied_lods = request.lods.clone(); // 已采用配置与已保存配置分开。
    feedback.applied_spacing_meters = request
        .lods
        .iter()
        .map(|lod| {
            [
                request.spec.actual_spacing_meters[0] as f32 * lod.spacing_multiplier as f32,
                request.spec.actual_spacing_meters[1] as f32 * lod.spacing_multiplier as f32,
            ]
        })
        .collect(); // 两轴真实格距不混同单块边长。
    feedback.resident_source_pages = state.retained_sources.len(); // 隐式平地不伪造驻留页数。
    feedback.resident_blocks = state.resident.len(); // 实际驻留数。
    feedback.rebuilt_blocks = rebuilt; // 本次局部更新数。
    feedback.build_milliseconds = cooked.elapsed_ms; // 不将此值误称为总物理帧时。
    feedback.detail = "已采用：多层共享碰撞（实验）".into(); // 不宣称已实机验收。
    runtime.collision_ready = true; // 初次承托满足场景启动门。
    Ok(()) // 完整集合已排入同一次命令提交。
} // chain提交命令后再允许实体恢复。

#[cfg(test)] // 完整性检查必须在任何ECS副作用之前拒绝坏批次。
mod tests {
    use super::super::types::{ChunkKey, ChunkPlan};
    use super::*; // 直接测试生产预检，不构造真实物理世界。
    use feathrise_scene_content::{SceneCollisionLod, SceneId, SparseHeightSpec}; // 正式身份合同。
    use std::collections::BTreeMap; // 小型固定测试集合。 // 单块有限覆盖夹具。

    #[test] // 缺失形状、错版边界及换根都不得发布伪承托。
    fn preflight_rejects_incomplete_or_stale_candidate_without_mutation() {
        let scene_id = SceneId::parse("MAP-publication-test").unwrap(); // 独立测试场景。
        let root = Entity::from_bits(5); // 稳定场景根。
        let key = ChunkKey {
            band: 0,       // 发布夹具的首个LOD圆环身份。
            cells_log2: 5, // 固定夹具采用32×32单元管理块。
            level: 0,
            x: 0,
            z: 0,
        }; // 原点4米块。
        let plan = ChunkPlan {
            key,
            finer_edges: 0,
            signature: String::new(), // 完整矩形夹具不含显式单元身份。
            cells: None,              // 保留完整矩形夹具。
        }; // 无接缝单块。
        let request = Arc::new(Request {
            stitch_profiles:Default::default(),
            stitches:Default::default(),
            holes: Default::default(),
            // 有效小型请求。
            scene_id: scene_id.clone(), // 当前图。
            root,                   // 当前根。
            spec: SparseHeightSpec::new([0.0; 2], [512.0; 2], 0.125, -128.0, 256.0).unwrap(), // 标准作者规格。
            manifest_revision: 1,  // 正式版本。
            live_revision: None,   // 无草稿。
            crater_revision: None, // 无会话坑。
            lods: vec![SceneCollisionLod {
                max_distance_meters: 16.0,
                spacing_multiplier: 1,
            }], // 一层有效配置。
            anchor: Vec2::ZERO,    // 玩家起点。
        }); // 不含大数组。
        let mut state = LayeredCollisionRuntime {
            scene_id: Some(scene_id),
            root: Some(root),
            desired: Some(request),
            ..default()
        }; // 尚无可复用块。
        let mut cooked = Cooked {
            shapes: Vec::new(),
            plans: BTreeMap::from([(key, plan.clone())]),
            sources: BTreeMap::new(),
            craters: BTreeMap::new(),
            bounds: vec![(Vec2::ZERO, Vec2::splat(4.0))],
            supported_polygon: Vec::new(), // 旧完整矩形夹具无圆形裁切。
            supported_rows: None,          // 无显式单元的兼容夹具。
            elapsed_ms: 0.0,
            layout: None, // 兼容人工完整块预检夹具不参与缓存。
        }; // 故意缺失唯一形状。
        assert!(validate_candidate(&state, &cooked).is_err()); // 不能只凭边界就把玩家解冻。
        assert!(state.resident.is_empty() && state.applied.is_none()); // 预检不改变已有世界状态。
        state.resident.insert(
            key,
            Resident {
                entity: Entity::from_bits(6),
                plan,
                stamp: SourceStamp {
                    sources: Vec::new(),
                },
            },
        ); // 模拟已经真实驻留的同版形状。
        assert!(validate_candidate(&state, &cooked).is_ok()); // 完整复用不要求重复构造。
        cooked.bounds[0].1.x = 5.0; // 候选把承托范围错误扩大一米。
        assert!(validate_candidate(&state, &cooked).is_err()); // 区域门必须与实际几何完全一致。
        cooked.bounds[0].1.x = 4.0; // 恢复正确边界。
        state.root = Some(Entity::from_bits(7)); // 同图场景根已经换代。
        assert!(validate_candidate(&state, &cooked).is_err()); // 迟到批次不能跨根安装。
        state.root = Some(root); // 恢复原根单独检验跨场景身份。
        state.scene_id = Some(SceneId::parse("MAP-publication-other").unwrap()); // 场景已切换但旧候选仍留在后台。
        assert!(validate_candidate(&state, &cooked).is_err()); // 相同根位模式也不能绕过场景身份。
        assert_eq!(state.resident.len(), 1); // 所有失败都保留旧有效集合。
    } // 不调用Commands即可证明预检是纯读取。
}
