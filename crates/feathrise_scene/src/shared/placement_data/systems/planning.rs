//! 正式placement文件的固定预算增量校验与Scene0计划。
//!
//! 【功能设计】主线程每帧最多启动一百二十八条记录，并最多校验
//! 六十四个整网格水体绑定。只冻结当前记录，不在启动帧深拷贝整份JSON；
//! 全部记录成功后才把完整计划交给隐藏候选根。

use std::collections::{HashSet, VecDeque}; // 单根水体唯一性与Scene首轮检查队列均按公开上限有界。
use std::sync::Arc; // 完整建筑净空只在规划收尾冻结一次并移交候选。

use bevy::prelude::*; // Rect、Transform、Handle与GLTF Scene0资产标签。
use bevy::world_serialization::WorldAsset; // 规划为每条记录建立强WorldAsset句柄。
use feathrise_scene_content::{
    BuildingPresetRegistry, BuildingWorldQuad, MAX_SCENE_PLACEMENT_WATER_NAME_CHARS,
    MAX_SCENE_PLACEMENT_WATER_NODE_PATH_CHARS, MAX_SCENE_PLACEMENT_WATER_NODE_PATH_DEPTH,
    MAX_SCENE_PLACEMENTS_PER_SCENE, SceneId, ScenePlacementRecord, ScenePlacementWaterSurface,
    ScenePlacementsAsset, valid_building_preset_id,
}; // 运行时与编辑器复用同一水体格式与数量硬门。

use super::super::resources::{
    PendingPlacementPlan, PendingPlacementPrimitive, PendingPlacementRevision, PlacementPlanning,
    PlacementRecordPlanning, PlacementSceneLoadTracker,
}; // 规划游标存入唯一运行时事务。

pub(super) const MAX_PLACEMENTS_PER_SCENE: usize = MAX_SCENE_PLACEMENTS_PER_SCENE; // 运行时与F12复制动作共享同一显式实例硬门。
pub(super) const PLACEMENT_PLAN_RECORD_BUDGET_PER_FRAME: usize = 128; // 普通记录的校验、拷贝和句柄请求不得超过本额度。
pub(super) const PLACEMENT_PLAN_WATER_BUDGET_PER_FRAME: usize = 64; // 复杂节点路径校验独立限流，避免一百二十八根各扫六十四项。

const MAX_PLACEMENT_ID_BYTES: usize = 1_024; // 稳定ID足以容纳中文作者命名，同时阻断单条异常大字符串。
const MAX_PLACEMENT_ASSET_PATH_BYTES: usize = 4_096; // AssetServer相对路径在小写化和诊断前先受O(1)字节长度门限。
const MAX_UTF8_BYTES_PER_CHAR: usize = 4; // 合法Unicode标量值最多四字节，用字节前置门保证后续chars扫描有界。

/// 本帧增量规划实际推进的小型证据。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PlacementPlanningAdvance {
    pub(super) records_started: usize, // 只计本帧首次冻结的记录。
    pub(super) water_surfaces_validated: usize, // 只计本帧完成校验的水体绑定。
    pub(super) records_completed: usize, // 含水记录可以在后续帧完成，因此与启动数分开。
    pub(super) complete: bool,         // 只有全部记录变成强句柄计划时为true。
}

impl PlacementPlanningAdvance {
    /// 稳定轮询不得冒充保存交接的进度心跳。
    pub(super) fn made_progress(self) -> bool {
        self.records_started > 0 || self.water_surfaces_validated > 0 || self.records_completed > 0
    }
}

/// 相对assets根的GLB/GLTF路径在任何分配和小写化之前先通过长度门。
pub(super) fn validate_asset_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PLACEMENT_ASSET_PATH_BYTES {
        return false; // 空路径和异常大输入都不进入后续线性扫描。
    }
    let lower = path.to_ascii_lowercase(); // 路径已受四KiB硬门，小写化不会产生无界单项分配。
    !path.starts_with('/')
        && !path.contains("..")
        && !path.contains('\\')
        && (lower.ends_with(".glb") || lower.ends_with(".gltf"))
}

/// 校验单条摆放的非水字段，不拷贝整条记录。
fn validate_record_transform(
    record: &ScenePlacementRecord, // 当前资产中即将被单条冻结的记录。
    scene_id: &SceneId,              // 诊断与范围冻结所属场景。
    bounds: Rect,                // 规划启动时已校验的世界XZ范围。
) -> Result<Transform, String> {
    if record.id.len() > MAX_PLACEMENT_ID_BYTES
        || record.id.trim().is_empty()
        || !(if record.asset_target.is_builtin() { record.asset_path.is_empty() } else { validate_asset_path(&record.asset_path) })
        || record
            .building_preset_id
            .as_deref()
            .is_some_and(|id| !valid_building_preset_id(id))
    {
        return Err(format!("实例{}的ID、模型路径或建筑预设ID无效", record.id)); // 过长值与路径越界同样失败关闭。
    }
    record.validated_transform(scene_id, bounds) // 公共入口保证F12、保存预检和运行时不存在边界分叉。
}

/// 用O(1)字节长度前置门保证公共Unicode纯校验的单项成本有界。
fn validate_water_surface_bounded(surface: &ScenePlacementWaterSurface) -> Result<(), String> {
    if surface.node_path.len() > MAX_SCENE_PLACEMENT_WATER_NODE_PATH_DEPTH {
        return surface.validate(); // 公共校验会返回统一路径深度诊断。
    }
    let max_name_bytes = MAX_SCENE_PLACEMENT_WATER_NAME_CHARS * MAX_UTF8_BYTES_PER_CHAR; // 任意合法Unicode名称的绝对字节上限。
    if surface.mesh_name.len() > max_name_bytes
        || surface
            .node_path
            .iter()
            .any(|segment| segment.len() > max_name_bytes)
    {
        return Err(format!(
            "GLB水体节点或网格名称超过{}个Unicode字符",
            MAX_SCENE_PLACEMENT_WATER_NAME_CHARS,
        )); // 异常大单段不再进入chars().count()。
    }
    let max_path_bytes = MAX_SCENE_PLACEMENT_WATER_NODE_PATH_CHARS * MAX_UTF8_BYTES_PER_CHAR; // 合法完整路径的绝对字节上限。
    let path_bytes = surface
        .node_path
        .iter()
        .try_fold(0usize, |total, segment| total.checked_add(segment.len())); // 溢出与超长路径都失败关闭。
    if path_bytes.is_none_or(|bytes| bytes > max_path_bytes) {
        return Err(format!(
            "GLB水体节点路径超过{}个Unicode字符",
            MAX_SCENE_PLACEMENT_WATER_NODE_PATH_CHARS,
        )); // 不让数千个合法短段组成异常大单项。
    }
    surface.validate() // 数值、稳定名称与真实Unicode字符数继续复用公共入口。
}

/// 从已确认完整的清单和摆放资产建立空增量事务。
#[allow(clippy::too_many_arguments)]
pub(super) fn begin_planning(
    generation: u32,               // 当前场景代次。
    revision: u32,                 // 新尝试修订。
    scene_id: SceneId,                 // 当前场景稳定ID。
    catalog_revision: u64,         // 当前目录修订。
    building_preset_revision: u64, // 当前建筑目录修订；净空必须与这一快照一致。
    manifest_id: bevy::asset::AssetId<feathrise_scene_content::SceneManifestAsset>, // 清单输入身份。
    document_id: bevy::asset::AssetId<ScenePlacementsAsset>, // 摆放JSON输入身份。
    world_bounds: Rect,            // 已校验世界范围。
    total_records: usize,          // 启动时冻结的记录数。
) -> PlacementPlanning {
    PlacementPlanning {
        terrain_filter: Default::default(),
        terrain_patch_count: 0,
        terrain_holes: Vec::new(),
        generation,                      // 迟到规划必须同时匹配代次。
        revision,                        // 迟到规划必须同时匹配修订。
        scene_id,                          // 迟到规划必须同时匹配场景。
        catalog_revision,                // 目录换句柄时取消本事务。
        building_preset_revision,        // 建筑净空参数换代时取消本事务。
        manifest_id,                     // 清单身份不在跨帧通过场景ID重新猜测。
        document_id,                     // 摆放身份不在跨帧通过路径重新猜测。
        world_bounds,                    // 后续记录仅执行廉价Rect包含检查。
        total_records,                   // 跨帧长度变化不能被混入旧计划。
        next_record: 0,                  // 从文档首条开始。
        total_water_surfaces: 0,         // 全图水体预算从零逐条累计。
        ids: HashSet::new(),             // 不在启动帧为八千一百九十二项预先建全表。
        current_record: None,            // 首帧尚未冻结任何单条记录。
        plans: Vec::new(),               // 按完成顺序增量增长，不一次克隆文档。
        building_clearances: Vec::new(), // 只在建筑记录完整通过后追加，不发布规划前缀。
        building_clearance_floor_heights: Vec::new(),
        scene_ids: HashSet::new(),        // 只在首次见到Scene0句柄时增长。
        queued_scene_ids: HashSet::new(), // 与去重ID同帧增量建立。
        scene_checks: VecDeque::new(),    // 每个唯一Scene只排入一次首轮检查。
    }
}

/// 确认跨帧事务仍然对应当前目录和资产句柄。
pub(super) fn planning_matches_inputs(
    planning: &PlacementPlanning,  // 进行中事务的冻结身份。
    generation: u32,               // 当前场景代次。
    scene_id: &SceneId,                // 当前场景ID。
    catalog_revision: u64,         // 当前目录修订。
    building_preset_revision: u64, // 当前建筑目录修订。
    manifest_id: bevy::asset::AssetId<feathrise_scene_content::SceneManifestAsset>, // 当前清单句柄ID。
    document_id: bevy::asset::AssetId<ScenePlacementsAsset>, // 当前摆放句柄ID。
) -> bool {
    planning.generation == generation
        && planning.scene_id == *scene_id
        && planning.catalog_revision == catalog_revision
        && planning.building_preset_revision == building_preset_revision
        && planning.manifest_id == manifest_id
        && planning.document_id == document_id // 任一身份变化都必须由新事务从头规划。
}

/// 在两组独立预算内推进记录与整网格水体校验。
#[cfg(test)]
pub(super) fn advance_planning(
    planning: &mut PlacementPlanning, // 本帧只改动精确游标和已完成前缀。
    document: &ScenePlacementsAsset,    // 当前Assets内与冻结ID相同的资产。
    building_registry: &BuildingPresetRegistry, // 建筑引用、模型来源和净空都从同一原子目录快照解析。
    load_assets: &mut impl FnMut(
        &ScenePlacementRecord,
    ) -> (Handle<WorldAsset>, Option<PendingPlacementPrimitive>), // 生产路径请求Scene0锚点及可选直接网格，测试可注入弱句柄。
) -> Result<PlacementPlanningAdvance, String> {
    advance_planning_bound(planning,document,building_registry,load_assets,&mut |_|Ok(None))
}

pub(super) fn advance_planning_bound(
    planning: &mut PlacementPlanning,
    document: &ScenePlacementsAsset,
    building_registry: &BuildingPresetRegistry,
    load_assets: &mut impl FnMut(&ScenePlacementRecord)->(Handle<WorldAsset>,Option<PendingPlacementPrimitive>),
    resolve_binding: &mut impl FnMut(&ScenePlacementRecord)->Result<Option<feathrise_scene_content::TerrainObjectTransform>,String>,
) -> Result<PlacementPlanningAdvance, String> {
    if building_registry.revision != planning.building_preset_revision {
        return Err("建筑预设目录在显式实例增量规划期间换代".to_owned()); // 不把两个目录修订的净空拼成同一候选。
    }
    if document.scene_id != planning.scene_id || document.placements.len() != planning.total_records {
        return Err(format!(
            "场景{}的摆放文件在增量规划期间换代",
            planning.scene_id,
        )); // 不将新旧文档的记录前缀组成候选。
    }
    let mut advance = PlacementPlanningAdvance::default(); // 本帧真实工作量供heartbeat与测试读取。
    let mut record_budget = PLACEMENT_PLAN_RECORD_BUDGET_PER_FRAME; // 只限制新启动记录，不重复计跨帧当前记录。
    let mut water_budget = PLACEMENT_PLAN_WATER_BUDGET_PER_FRAME; // 水体名称与路径扫描有独立额度。

    loop {
        if planning.current_record.is_none() {
            if planning.next_record >= planning.total_records || record_budget == 0 {
                break; // 全部记录已完成，或本帧普通记录预算已用完。
            }
            let source = &document.placements[planning.next_record]; // 先借用源记录通过O(1)长度与基础数值门，不盲目深拷贝异常字符串。
            if !planning.terrain_filter.accepts(source) {
                planning.next_record += 1;
                record_budget -= 1;
                advance.records_completed += 1;
                continue; // 跳过的记录同样消耗固定扫描预算，不能在一帧遍历完整场景。
            }
            let transform = validate_record_transform(
                source,                // 从同一资产记录校验基础字段。
                &planning.scene_id,      // 诊断使用冻结场景ID。
                planning.world_bounds, // 范围使用冻结合法Rect。
            )?;
            let source_pose=feathrise_scene_content::TerrainObjectTransform{translation:source.translation,yaw_degrees:source.yaw_degrees,scale:source.scale, ..Default::default()};
            let resolved=resolve_binding(source)?.unwrap_or(source_pose);
            let transform=if source.terrain_binding.is_some() {resolved.as_transform()} else {transform};
            if !planning.ids.insert(source.id.clone()) {
                return Err(format!(
                    "场景{}包含重复实例ID {}",
                    planning.scene_id, source.id,
                )); // 重复ID在首次见到的记录帧立即失败。
            }
            let next_total = planning
                .total_water_surfaces
                .checked_add(source.water_surfaces.len())
                .ok_or_else(|| "单个场景整网格水体绑定总数计算溢出".to_owned())?;
            planning.total_water_surfaces = next_total; // 只在当前记录基础门通过后累计。
            planning.current_record = Some(PlacementRecordPlanning {
                source_pose,
                record: ScenePlacementRecord {
                    mesh_lod: source.mesh_lod, // 完整候选复制实例策略，不因姿态/水绑定分帧计划退回全局。
                    terrain_binding: source.terrain_binding.clone(),
                    editor: Default::default(),
                    terrain_patch: source.terrain_patch.clone(), // 有界元数据随同一实例冻结，不在强句柄准备时丢失洞口。
                    id: source.id.clone(), // ID已通过一KiB前置门，只做一次有界拷贝。
                    asset_path: source.asset_path.clone(), // 路径已通过四KiB前置门。
                    asset_target: source.asset_target, // Scene0或单primitive数字身份是固定大小值。
                    building_preset_id: source.building_preset_id.clone(), // 可选稳定身份已通过有界字符校验。
                    courtyard: source.courtyard.clone(), // 保留已校验的院落成员身份，不把正式候选拆离组合。
                    translation: resolved.translation, // 完整姿态以已采用的对象身份解析，不能只跟随Y。
                    height_mode: source.height_mode, // 完整计划保留固定或跟随作者语义供热接管诊断。
                    terrain_offset: source.terrain_offset, // 离地偏移随完整快照跨候选保存。
                    yaw_degrees: resolved.yaw_degrees,
                    scale: resolved.scale,
                    water_surfaces: Vec::with_capacity(source.water_surfaces.len()), // 最多六十四个席位，内容按水体预算逐项追加。
                },
                transform, // 完成后直接移入强句柄计划。
                total_water_surfaces: source.water_surfaces.len(), // 冻结跨帧源长度。
                next_water_surface: 0, // 从单根首个绑定开始。
                water_identities: HashSet::new(), // 最多六十四项，不在启动帧预分配。
            });
            record_budget -= 1; // 冻结和基础校验已消耗一条记录额度。
            advance.records_started += 1; // heartbeat只在游标确实进入新记录时推进。
        }

        let current = planning
            .current_record
            .as_mut()
            .expect("上方已建立当前记录"); // 单条水体游标继续消耗独立预算。
        let source = &document.placements[planning.next_record]; // 源记录仍位于冻结文档索引。
        let source_matches = source.id == current.record.id
            && source.mesh_lod == current.record.mesh_lod
            && source.terrain_patch == current.record.terrain_patch
            && source.asset_path == current.record.asset_path
            && source.asset_target == current.record.asset_target
            && source.building_preset_id == current.record.building_preset_id
            && source.courtyard == current.record.courtyard
            && source.terrain_binding == current.record.terrain_binding
            && source.translation == current.source_pose.translation
            && source.yaw_degrees == current.source_pose.yaw_degrees
            && source.scale == current.source_pose.scale
            && source.water_surfaces.len() == current.total_water_surfaces; // 同句柄原地变异也不能继续混合旧前缀。
        if !source_matches {
            return Err(format!(
                "摆放实例{}在整网格水体增量校验期间换代",
                current.record.id,
            )); // 资产事件理论上会先取消，本门仍防御无事件的原地改写。
        }
        while current.next_water_surface < current.total_water_surfaces && water_budget > 0 {
            let surface = &source.water_surfaces[current.next_water_surface]; // 每帧只读取预算内的源单项。
            validate_water_surface_bounded(surface).map_err(|detail| {
                format!("摆放实例{}的整网格水体无效：{detail}", current.record.id,)
            })?;
            let identity = (surface.node_path.clone(), surface.mesh_name.clone()); // 只对已通过公开长度硬门的小身份分配。
            if !current.water_identities.insert(identity) {
                return Err(format!(
                    "摆放实例{}的GLB节点路径{}下网格{}被重复赋予水体覆盖",
                    current.record.id,
                    surface.node_path.join("/"),
                    surface.mesh_name,
                )); // 不使用可复用Mesh datablock索引冒充最终身份。
            }
            current.record.water_surfaces.push(surface.clone()); // 只在名称、路径与数值完整通过后拷贝当前有界单项。
            current.next_water_surface += 1; // 游标只在当前绑定完整成功后前进。
            water_budget -= 1; // 路径与名称校验消耗一项水体额度。
            advance.water_surfaces_validated += 1; // 记录真实水体进度。
        }
        if current.next_water_surface < current.total_water_surfaces {
            break; // 本帧水体额度用完，同一记录下帧继续。
        }

        let current = planning
            .current_record
            .take()
            .expect("当前记录已通过全部水体校验"); // 释放游标借用后请求同源资产句柄。
        if let Some(building_preset_id) = current.record.building_preset_id.as_deref() {
            let preset = building_registry
                .resolve(building_preset_id)
                .ok_or_else(|| {
                    format!(
                        "摆放实例{}引用不存在的建筑预设{}",
                        current.record.id, building_preset_id,
                    )
                })?; // 旧别名由目录统一解析，真正失效的身份拒绝整份修订。
            if preset.asset_path != current.record.asset_path
                || preset.asset_target != current.record.asset_target
            {
                return Err(format!(
                    "摆放实例{}的模型来源与建筑预设{}不一致",
                    current.record.id, building_preset_id,
                )); // 防止只借用另一预设净空却实例化不对应的模型。
            }
            planning
                .building_clearances
                .push(BuildingWorldQuad::from_local_rect(
                    preset.vegetation_clearance, // 净空独立于实体占地与门前空间。
                    current.record.translation,  // 与正式根使用同一世界平移。
                    current.record.yaw_degrees,  // 与正式根使用同一Y轴旋转。
                    current.record.scale,        // 非均匀X/Z缩放同样保留为精确四边形。
                )); // 每条建筑只追加一个固定大小值，完整规划后再统一冻结。
            planning.building_clearance_floor_heights.push(
                current.record.translation[1]
                    + preset.foundation.model_base_y * current.record.scale[1],
            ); // 模型根Y不一定是地基Y，必须沿用建筑实际模型基准。
        }
        let holes = current.record.terrain_cutouts(planning.world_bounds)?;
        if current
            .record
            .terrain_patch
            .as_ref()
            .is_some_and(|p| p.streamed_volume)
        {
            // v3载体只记录作者包围范围。附近真实网格由流送域授信，不能整体开洞或实例化标记三角。
            planning.terrain_patch_count += 1;
            if planning.terrain_patch_count > 32 {
                return Err("场景三维地形作者层超过32项".into());
            }
            planning.next_record += 1;
            advance.records_completed += 1;
            continue;
        }
        for hole in holes {
            if planning.terrain_holes.len() >= feathrise_scene_content::MAX_TERRAIN_CUTOUTS {
                return Err("场景三维地形洞口超过32项预算".into());
            }
            planning.terrain_holes.push(hole);
        }
        if current.record.terrain_patch.is_some() {
            planning.terrain_patch_count += 1;
            if planning.terrain_patch_count > 32 {
                return Err("场景三维地形实例超过32项预算，请拆分场景内容".into());
            }
        }
        let (scene, primitive) = load_assets(&current.record); // 每帧请求次数不超过记录预算。
        let scene_id = scene.id(); // 复用同一GLB的所有记录共享一个依赖身份。
        if !current.record.asset_target.is_builtin() && planning.scene_ids.insert(scene_id) {
            planning.queued_scene_ids.insert(scene_id); // 初始检查队列与去重集同步建立。
            planning.scene_checks.push_back(scene_id); // 收尾时只移动队列，不遍历全部计划。
        }
        planning.plans.push(PendingPlacementPlan {
            record: current.record,       // 已校验完整快照移入候选计划。
            scene,                        // Scene0锚点与上述记录的asset_path精确对应。
            primitive,                    // 子资产记录额外持有精确网格及其原关联材质句柄。
            transform: current.transform, // 变换不在候选生成帧重新校验或转换。
        });
        planning.next_record += 1; // 只有计划所有权完整存入后才推进文档游标。
        advance.records_completed += 1; // 跨帧含水记录的收尾也是真实进展。
    }

    advance.complete =
        planning.next_record == planning.total_records && planning.current_record.is_none(); // 空文档与全部成功的文档都能明确收尾。
    Ok(advance) // 失败不会移出半成品，由调用者保留旧活动批次。
}

/// 将完整规划O(1)移交给候选记录与去重Scene加载跟踪器。
pub(super) fn finish_planning(
    planning: PlacementPlanning, // 调用前必须已确认complete。
) -> (PendingPlacementRevision, PlacementSceneLoadTracker) {
    debug_assert_eq!(planning.next_record, planning.total_records); // 内部误用不能发布半份计划。
    debug_assert!(planning.current_record.is_none()); // 含水记录必须已完成全部绑定校验。
    let pending = PendingPlacementRevision {
        terrain_patch_count: planning.terrain_patch_count,
        terrain_holes: Arc::from(planning.terrain_holes),
        generation: planning.generation, // 候选根继承规划代次。
        revision: planning.revision,     // 候选根继承规划修订。
        scene_id: planning.scene_id,         // 候选根继承规划场景身份。
        building_preset_revision: planning.building_preset_revision, // 候选携带生成净空的目录身份。
        building_clearances: Arc::from(planning.building_clearances), // Vec零拷贝冻结，后台消费者只浅克隆Arc。
        building_clearance_floor_heights: Arc::from(planning.building_clearance_floor_heights),
        plans: planning.plans, // 已校验记录和强句柄整体移动，不深拷贝。
    };
    let tracker = PlacementSceneLoadTracker {
        generation: pending.generation,  // 加载结果必须对应同一代次。
        revision: pending.revision,      // 加载结果必须对应同一修订。
        placement_ids: planning.ids, // 已完成唯一性校验的稳定ID随候选移动，最终由固定预算退役器逐项释放。
        scene_ids: planning.scene_ids, // 事件归属集直接移动。
        ready_scene_ids: HashSet::new(), // 只有AssetServer实际证明完整依赖后才增加。
        queued_scene_ids: planning.queued_scene_ids, // 首轮队列去重集直接移动。
        scene_checks: planning.scene_checks, // 首轮检查队列直接移动。
    };
    (pending, tracker) // 两份结果在同一系统帧一起安装。
}
