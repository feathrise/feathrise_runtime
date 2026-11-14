//! placement正式文件增量规划与Scene依赖队列的大规模回归。

use feathrise_scene_core::residency::{InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use std::collections::{HashSet, VecDeque}; // 加载跟踪夹具与生产资源使用同样的去重集和队列。

use bevy::asset::{AssetId, uuid::Uuid}; // 测试生成不依赖AssetServer的唯一类型化资产ID。
use bevy::prelude::*; // Rect、Handle与App测试世界。
use bevy::world_serialization::WorldAsset; // 规划与轮询的正式Scene资产类型。
use feathrise_scene_content::{
    BuildingPresetRegistry, MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_PLACEMENT, SceneId,
    SceneManifestAsset, ScenePlacementAssetTarget, ScenePlacementRecord, ScenePlacementWaterKind,
    ScenePlacementWaterSurface, ScenePlacementsAsset, ResolvedBuildingPresetCatalog,
}; // 构造合法普通记录和最大单根水体记录。

use super::super::{loading, planning, retirement}; // 直接验证三个私有阶段的预算和取消边界。
use crate::BuildingVegetationClearanceState;
use crate::shared::placement_data::resources::{
    ScenePlacementRuntimeState, PendingPlacementPlan, PendingPlacementPrimitive,
    PendingPlacementRevision, PlacementSceneLoadTracker,
}; // 新修订取消测试通过唯一写侧资源进行。 // 候选夹具携带“已完成但无建筑”的正式净空快照。

const TEST_BUILDING_PRESET_REVISION: u64 = 17; // 所有规划阶段固定到同一份空建筑目录。

fn empty_building_registry() -> BuildingPresetRegistry {
    BuildingPresetRegistry {
        catalog: Handle::default(), // 纯规划测试不向 AssetServer 注册目录资产。
        revision: TEST_BUILDING_PRESET_REVISION,
        resolved: ResolvedBuildingPresetCatalog::default(), // 普通 placement 不需要建筑条目。
        last_error: None,
    }
}

/// 为类型化资产生成可重现且互不相同的UUID身份。
fn asset_id<A: Asset>(value: u128) -> AssetId<A> {
    AssetId::Uuid {
        uuid: Uuid::from_u128(value), // 测试不把ID注册进Assets，只验证集合和队列算法。
    }
}

/// 构造一条位于测试Rect内的普通placement。
fn record(index: usize) -> ScenePlacementRecord {
    ScenePlacementRecord {
        mesh_lod: None,
        terrain_binding: None,
        editor: Default::default(),
        terrain_patch: None,
        id: format!("placement-{index}"), // 八千条记录拥有各自唯一稳定ID。
        asset_path: "gltf/shared/test.glb".to_owned(), // 所有记录复用同一GLB，额外验证Scene ID去重。
        asset_target: Default::default(),              // 大规模基线全部使用历史Scene0目标。
        building_preset_id: None,                      // 大规模普通资产基线不附带建筑语义。
        courtyard: None,                               // 独立摆放记录不属于任何院落副本。
        translation: [0.0, 0.0, 0.0],                  // 原点位于测试世界范围内。
        height_mode: feathrise_scene_content::ScenePlacementHeightMode::FixedHeight, // 规划预算测试不访问R16。
        terrain_offset: 0.0,        // 固定高度不携带离地偏移。
        yaw_degrees: 0.0,           // 零旋转是有限合法作者值。
        scale: [1.0, 1.0, 1.0],     // 单位缩放处于正式闭区间。
        water_surfaces: Vec::new(), // 普通规模测试不消耗水体预算。
    }
}

#[test]
fn builtin_cuboid_planning_never_tracks_a_fake_scene_dependency() {
    let id = SceneId::parse("MAP-builtin-planning").unwrap();
    let mut cuboid = record(0);
    cuboid.asset_path.clear();
    cuboid.asset_target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
    let document = ScenePlacementsAsset { schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION, scene_id: id.clone(), placements: vec![cuboid] };
    let mut work = planning::begin_planning(1, 1, id, 1, TEST_BUILDING_PRESET_REVISION,
        asset_id::<SceneManifestAsset>(1), asset_id::<ScenePlacementsAsset>(2),
        Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0)), 1);
    let advance = planning::advance_planning(&mut work, &document, &empty_building_registry(), &mut |_| (Handle::default(), None)).unwrap();
    assert!(advance.complete);
    let (pending, tracker) = planning::finish_planning(work);
    assert_eq!(pending.plans.len(), 1);
    assert!(tracker.scene_ids.is_empty());
    assert!(tracker.scene_checks.is_empty());
}

#[test]
fn volume_stream_carrier_is_validated_without_loading_scene_or_cutting_author_bbox() {
    let id = SceneId::parse("MAP-stream-planning").unwrap();
    let mut r = record(0);
    r.terrain_patch=Some(serde_json::from_value(serde_json::json!({"sculpted_volume":true,"streamed_volume":true,"volume_cell_size":0.25,"cutouts":[{"min":[0,0],"max":[8,8]}]})).unwrap());
    let doc = ScenePlacementsAsset {
        schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
        scene_id: id.clone(),
        placements: vec![r.clone()],
    };
    let begin = || {
        planning::begin_planning(
            1,
            1,
            id.clone(),
            1,
            TEST_BUILDING_PRESET_REVISION,
            asset_id::<SceneManifestAsset>(1),
            asset_id::<ScenePlacementsAsset>(2),
            Rect::from_corners(Vec2::splat(-10.), Vec2::splat(10.)),
            1,
        )
    };
    let mut work = begin();
    let mut load = |_: &ScenePlacementRecord| panic!("v3载体不能走普通GLB实例化");
    assert!(
        planning::advance_planning(&mut work, &doc, &empty_building_registry(), &mut load)
            .unwrap()
            .complete
    );
    let (pending, tracker) = planning::finish_planning(work);
    assert!(pending.plans.is_empty());
    assert!(pending.terrain_holes.is_empty());
    assert_eq!(pending.terrain_patch_count, 1);
    assert!(tracker.scene_ids.is_empty());
    r.terrain_patch.as_mut().unwrap().volume_cell_size = None;
    let bad = ScenePlacementsAsset {
        placements: vec![r],
        ..doc
    };
    assert!(
        planning::advance_planning(&mut begin(), &bad, &empty_building_registry(), &mut load)
            .is_err()
    );
}

/// 为同一placement构造一个唯一且完整合法的水体绑定。
fn water_surface(index: usize) -> ScenePlacementWaterSurface {
    ScenePlacementWaterSurface {
        mesh_index: index as u32, // 索引只作诊断，不承担最终唯一性。
        node_path: vec!["场景根".to_owned(), format!("水面对象_{index}")], // 节点路径为每项建立唯一作者身份。
        mesh_name: format!("水面网格_{index}"), // 网格名与路径共同匹配Blender对象。
        kind: ScenePlacementWaterKind::Sloped,    // 类别不影响规划预算。
        flow_speed: 1.0,                        // 一米每秒处于公开合法范围。
        height_offset: 0.0,                     // 零偏移保留Blender原高程。
    }
}

/// 构造一项已经完成校验、等待候选生成的强句柄计划。
fn pending_plan(index: usize) -> PendingPlacementPlan {
    PendingPlacementPlan {
        record: record(index),                  // 每项拥有独立稳定ID和有界作者快照。
        scene: Handle::<WorldAsset>::default(), // 弱默认句柄足以验证析构预算。
        primitive: None,                        // 历史Scene0基线不请求直接子网格。
        transform: Transform::IDENTITY,         // 测试不进入真实实体生成。
    }
}

#[test]
fn eight_thousand_records_are_planned_across_frames_with_fixed_budgets() {
    let scene_id = SceneId::parse("MAP-placement-planning-budget").expect("测试场景ID合法"); // 与文档和规划共用同一身份。
    let document = ScenePlacementsAsset {
        schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,      // 规划阶段不重复解释loader已接纳的schema。
        scene_id: scene_id.clone(), // 文档必须与当前场景匹配。
        placements: (0..planning::MAX_PLACEMENTS_PER_SCENE).map(record).collect(), // 精确命中八千一百九十二项闭区间上限。
    };
    let mut work = planning::begin_planning(
        7,                                                         // 测试代次。
        3,                                                         // 测试修订。
        scene_id,                                                    // 冻结文档所属场景。
        11,                                                        // 测试目录修订。
        TEST_BUILDING_PRESET_REVISION,                             // 冻结同一建筑目录修订。
        asset_id::<SceneManifestAsset>(1),                           // 测试清单资产身份。
        asset_id::<ScenePlacementsAsset>(2),                         // 测试摆放资产身份。
        Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0)), // 所有记录原点都在范围内。
        document.placements.len(),                                 // 冻结完整文档长度。
    );
    let building_registry = empty_building_registry(); // 普通记录仍需通过统一建筑目录事务边界。
    let mut frames = 0usize; // 统计达到完整计划所需更新帧。
    let mut handle_requests = 0usize; // 每条记录只请求一次Scene0句柄。
    loop {
        let mut load = |_record: &ScenePlacementRecord| {
            handle_requests += 1; // 关注请求次数，不需要真实AssetServer。
            (Handle::<WorldAsset>::default(), None) // 同一默认ID模拟八千个placement复用一份GLB。
        };
        let advance =
            planning::advance_planning(&mut work, &document, &building_registry, &mut load)
                .expect("全部测试记录都应合法");
        assert!(advance.records_started <= planning::PLACEMENT_PLAN_RECORD_BUDGET_PER_FRAME); // 任何一帧都不能启动超额记录。
        assert!(
            advance.water_surfaces_validated <= planning::PLACEMENT_PLAN_WATER_BUDGET_PER_FRAME
        ); // 独立水体预算在普通记录文档中保持零。
        frames += 1; // 本轮更新帧已完成。
        if advance.complete {
            break; // 只在完整前缀达到文档尾部时退出。
        }
    }

    assert_eq!(
        frames,
        planning::MAX_PLACEMENTS_PER_SCENE / planning::PLACEMENT_PLAN_RECORD_BUDGET_PER_FRAME,
    ); // 八千一百九十二项必须用六十四帧，不得在首帧全扫。
    assert_eq!(work.plans.len(), planning::MAX_PLACEMENTS_PER_SCENE); // 完成后计划不丢记录。
    assert_eq!(handle_requests, planning::MAX_PLACEMENTS_PER_SCENE); // 一条记录对应一个强Scene0句柄请求。
    assert_eq!(work.scene_ids.len(), 1); // 加载门按唯一GLB去重，不重复轮询八千次。
    assert_eq!(work.scene_checks.len(), 1); // 首轮依赖检查队列同样只有一个ID。
}

#[test]
fn water_bindings_use_a_budget_independent_from_record_count() {
    let scene_id = SceneId::parse("MAP-placement-water-budget").expect("测试场景ID合法"); // 文档和规划身份一致。
    let mut placements = Vec::new(); // 四条记录各自命中单根六十四项上限。
    for index in 0..4 {
        let mut placement = record(index); // 复用合法基础变换和路径。
        placement.water_surfaces = (0..MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_PLACEMENT)
            .map(|surface| water_surface(index * 100 + surface))
            .collect(); // 每根内部身份唯一，全图精确二百五十六项。
        placements.push(placement); // 文档顺序稳定。
    }
    let document = ScenePlacementsAsset {
        schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,      // 测试不依赖schema分支。
        scene_id: scene_id.clone(), // 文档场景身份合法。
        placements,             // 四个最大含水根。
    };
    let mut work = planning::begin_planning(
        1,                                                         // 测试代次。
        1,                                                         // 测试修订。
        scene_id,                                                    // 冻结场景ID。
        1,                                                         // 测试目录修订。
        TEST_BUILDING_PRESET_REVISION,                             // 冻结同一建筑目录修订。
        asset_id::<SceneManifestAsset>(11),                          // 测试清单ID。
        asset_id::<ScenePlacementsAsset>(12),                        // 测试摆放ID。
        Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0)), // 全部原点合法。
        document.placements.len(),                                 // 冻结四条记录。
    );
    let building_registry = empty_building_registry(); // 水体预算测试不声明建筑条目。
    let mut load = |_record: &ScenePlacementRecord| (Handle::<WorldAsset>::default(), None); // 本测试只验证水体游标。

    for expected_completed in 1..=4 {
        let advance =
            planning::advance_planning(&mut work, &document, &building_registry, &mut load)
                .expect("二百五十六项精确位于全图上限");
        assert_eq!(
            advance.water_surfaces_validated,
            planning::PLACEMENT_PLAN_WATER_BUDGET_PER_FRAME,
        ); // 每帧精确消耗六十四项，不会因记录预算较大而全扫。
        assert_eq!(work.next_record, expected_completed); // 每帧只有一个最大含水根完成。
    }
    assert_eq!(work.plans.len(), 4); // 四帧后才得到完整文档计划。
}

#[test]
fn mesh_lod_policy_enters_complete_plan_and_mid_record_policy_change_rejects_partial_source() {
    for change_policy in [false, true] {
        let map = SceneId::parse("MAP-lod-plan").unwrap();
        let policy = feathrise_scene_content::MeshLodPolicy {
            display: feathrise_scene_content::MeshLodMode::Override(feathrise_scene_content::MeshDisplayLodSettings { max_error_pixels: 0.5, ..Default::default() }),
            collision: feathrise_scene_content::MeshLodMode::Disabled,
        };
        let mut first = record(0);
        first.mesh_lod = Some(policy);
        first.water_surfaces = (0..32).map(water_surface).collect();
        let mut second = record(1);
        second.mesh_lod = Some(policy);
        second.water_surfaces = (0..64).map(|index| water_surface(100 + index)).collect();
        let mut document = ScenePlacementsAsset { schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION, scene_id: map.clone(), placements: vec![first, second] };
        let mut work = planning::begin_planning(1, 1, map, 1, TEST_BUILDING_PRESET_REVISION,
            asset_id::<SceneManifestAsset>(71), asset_id::<ScenePlacementsAsset>(72),
            Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0)), document.placements.len());
        let registry = empty_building_registry();
        let mut load = |value: &ScenePlacementRecord| {
            assert_eq!(value.mesh_lod, Some(policy));
            (Handle::<WorldAsset>::default(), None)
        };
        let first_frame = planning::advance_planning(&mut work, &document, &registry, &mut load).unwrap();
        assert!(!first_frame.complete);
        assert_eq!(work.plans.len(), 1);
        assert_eq!(work.plans[0].record.mesh_lod, Some(policy));
        if change_policy { document.placements[1].mesh_lod = None; }
        let second_frame = planning::advance_planning(&mut work, &document, &registry, &mut load);
        if change_policy {
            assert!(second_frame.unwrap_err().contains("换代"));
        } else {
            assert!(second_frame.unwrap().complete);
            let (prepared, _) = planning::finish_planning(work);
            assert!(prepared.plans.iter().all(|plan| plan.record.mesh_lod == Some(policy)));
        }
    }
}

#[test]
fn primitive_target_enters_plan_with_direct_mesh_and_material_handles() {
    let scene_id = SceneId::parse("MAP-placement-primitive-plan").expect("测试场景ID合法");
    let mut primitive_record = record(0);
    primitive_record.asset_target = ScenePlacementAssetTarget::Primitive {
        mesh: 6,
        primitive: 0,
        material: Some(2),
        collision: false,
    };
    let document = ScenePlacementsAsset {
        schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
        scene_id: scene_id.clone(),
        placements: vec![primitive_record],
    };
    let mut work = planning::begin_planning(
        1,
        1,
        scene_id,
        1,
        TEST_BUILDING_PRESET_REVISION,
        asset_id::<SceneManifestAsset>(41),
        asset_id::<ScenePlacementsAsset>(42),
        Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0)),
        1,
    );
    let building_registry = empty_building_registry(); // primitive 记录同样绑定建筑目录修订。
    let mut load = |record: &ScenePlacementRecord| {
        assert!(matches!(
            record.asset_target,
            ScenePlacementAssetTarget::Primitive {
                mesh: 6,
                primitive: 0,
                material: Some(2),
                collision: false,
            }
        ));
        (
            Handle::<WorldAsset>::default(),
            Some(PendingPlacementPrimitive {
                mesh: Handle::<Mesh>::default(),
                material: Handle::<StandardMaterial>::default(),
            }),
        )
    };

    let advance = planning::advance_planning(&mut work, &document, &building_registry, &mut load)
        .expect("合法primitive记录应完成规划");
    assert!(advance.complete);
    assert!(work.plans[0].primitive.is_some());
    assert_eq!(
        work.plans[0].record.asset_target,
        document.placements[0].asset_target
    ); // 候选不能退化回Scene0。
}

#[test]
fn new_input_identity_cancels_an_old_partial_plan() {
    let scene_id = SceneId::parse("MAP-placement-cancel").expect("测试场景ID合法"); // 旧规划所属场景。
    let manifest_id = asset_id::<SceneManifestAsset>(21); // 旧新规划共用清单。
    let old_document_id = asset_id::<ScenePlacementsAsset>(22); // 旧JSON资产身份。
    let new_document_id = asset_id::<ScenePlacementsAsset>(23); // 热更后新JSON资产身份。
    let old = planning::begin_planning(
        5,                                                       // 旧代次。
        8,                                                       // 旧尝试修订。
        scene_id.clone(),                                          // 场景ID未变，只替换JSON。
        13,                                                      // 目录修订未变。
        TEST_BUILDING_PRESET_REVISION,                           // 建筑目录修订未变。
        manifest_id,                                             // 清单句柄未变。
        old_document_id,                                         // 冻结旧JSON句柄。
        Rect::from_corners(Vec2::splat(-1.0), Vec2::splat(1.0)), // 合法范围。
        256,                                                     // 模拟已规划一部分的文档。
    );
    assert!(planning::planning_matches_inputs(
        &old,                          // 旧规划。
        5,                             // 同代次。
        &scene_id,                       // 同场景。
        13,                            // 同目录修订。
        TEST_BUILDING_PRESET_REVISION, // 同建筑目录修订。
        manifest_id,                   // 同清单。
        old_document_id,               // 同摆放JSON。
    )); // 身份未变时允许下帧继续。
    assert!(!planning::planning_matches_inputs(
        &old,                          // 旧规划。
        5,                             // 同代次。
        &scene_id,                       // 同场景。
        13,                            // 同目录修订。
        TEST_BUILDING_PRESET_REVISION, // 同建筑目录修订。
        manifest_id,                   // 同清单。
        new_document_id,               // 新JSON句柄必须使旧游标失效。
    ));

    let mut app = App::new(); // 通过生产取消函数验证资源不保留旧规划。
    let mut state = ScenePlacementRuntimeState::default(); // 构造唯一写侧状态。
    state.planning = Some(old); // 模拟旧修订已在跨帧规划。
    app.insert_resource(state).add_systems(
        Update,
        |mut commands: Commands, mut state: InstanceResMut<ScenePlacementRuntimeState>| {
            retirement::retire_candidate(&mut commands, &mut state); // 新身份检测后生产路径使用同一取消边界。
        },
    );
    app.update(); // 执行一帧取消。
    assert!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .planning
            .is_none()
    ); // 旧游标不能在新JSON上继续。
    assert!(
        app.world()
            .resource::<ScenePlacementRuntimeState>()
            .retired_plan
            .is_some()
    ); // 取消只移交所有权，旧规划由后续固定预算系统释放。
}

#[test]
fn cancelled_large_planning_prefix_is_released_with_a_fixed_budget() {
    let scene_id = SceneId::parse("MAP-placement-retire-planning").expect("测试场景ID合法"); // 构造中途取消的最大规划前缀。
    let mut work = planning::begin_planning(
        6,                                                       // 测试代次。
        4,                                                       // 测试修订。
        scene_id,                                                  // 规划所属场景。
        2,                                                       // 测试目录修订。
        TEST_BUILDING_PRESET_REVISION,                           // 测试建筑目录修订。
        asset_id::<SceneManifestAsset>(31),                        // 测试清单身份。
        asset_id::<ScenePlacementsAsset>(32),                      // 测试摆放身份。
        Rect::from_corners(Vec2::splat(-1.0), Vec2::splat(1.0)), // 本测试不再推进记录，仅保留合法冻结范围。
        planning::MAX_PLACEMENTS_PER_SCENE,                        // 最大合法记录总量。
    );
    work.plans = (0..planning::MAX_PLACEMENTS_PER_SCENE)
        .map(pending_plan)
        .collect(); // 模拟已完成八千一百九十二条的计划前缀。
    work.ids = (0..planning::MAX_PLACEMENTS_PER_SCENE)
        .map(|index| format!("placement-{index}"))
        .collect(); // 重复ID门已持有同规模稳定身份集合。

    let mut state = ScenePlacementRuntimeState::default(); // 唯一写侧资源接管待取消规划。
    state.planning = Some(work); // 模拟输入在规划收尾前换代。
    retirement::retire_plan_data(&mut state); // 取消帧只做O(1)所有权转移。
    assert!(state.planning.is_none()); // 旧输入游标立即从活动状态失效。
    assert_eq!(
        state
            .retired_plan
            .as_ref()
            .and_then(|retired| retired.planning.as_ref())
            .expect("旧规划进入唯一退役槽")
            .plans
            .len(),
        planning::MAX_PLACEMENTS_PER_SCENE,
    ); // O(1)移交帧尚未同步析构任何计划项。

    let mut app = App::new(); // 独立运行生产固定预算清理系统。
    app.insert_resource(state)
        .add_systems(Update, retirement::cleanup_retired_placement_plan);
    app.update(); // 第一帧只能释放计划Vec尾部的固定前缀。
    let retired_planning = app
        .world()
        .resource::<ScenePlacementRuntimeState>()
        .retired_plan
        .as_ref()
        .and_then(|retired| retired.planning.as_ref())
        .expect("首帧预算不足以释放完整大规划");
    let remaining_plans =
        retired_planning.plans.len() + usize::from(retired_planning.current_plan.is_some()); // 下一个计划可O(1)移入游标，但未计费前仍属于未释放总数。
    assert_eq!(
        remaining_plans,
        planning::MAX_PLACEMENTS_PER_SCENE - retirement::PLACEMENT_PLAN_RETIRE_BUDGET_PER_FRAME,
    ); // 单帧绝不能多释放第一个一百二十八项计划。

    let mut frames = 1usize; // 已经执行首个退役帧。
    while app
        .world()
        .resource::<ScenePlacementRuntimeState>()
        .retired_plan
        .is_some()
    {
        app.update(); // 每次Update继续消费同一固定预算。
        frames += 1; // 统计完整释放所需帧数。
        assert!(frames <= 130); // 防御回归：游标必须持续前进而非卡住。
    }
    assert_eq!(frames, 128); // 8192计划和8192稳定ID共16384个顶层条目，每帧严格释放128个。
}

#[test]
fn adopted_large_candidate_moves_plans_and_ids_into_the_same_retirement_slot() {
    let scene_id = SceneId::parse("MAP-placement-retire-adopted").expect("测试场景ID合法"); // 模拟完整候选原子采用后的大数据所有权。
    let pending = PendingPlacementRevision {
        terrain_holes: Default::default(),
        terrain_patch_count: 0,
        generation: 9, // 候选代次仅作为固定大小壳保留。
        revision: 7,   // 候选修订仅作为固定大小壳保留。
        scene_id,        // 场景身份与计划同代退役。
        building_preset_revision: TEST_BUILDING_PRESET_REVISION, // 退役夹具仍保留候选目录身份。
        building_clearances: BuildingVegetationClearanceState::empty_regions(), // 无建筑也表示完整净空快照。
        building_clearance_floor_heights: Default::default(),
        plans: (0..planning::MAX_PLACEMENTS_PER_SCENE)
            .map(pending_plan)
            .collect(), // 最大合法候选含八千一百九十二个强句柄计划。
    };
    let tracker = PlacementSceneLoadTracker {
        generation: 9, // Scene跟踪身份与候选一致。
        revision: 7,   // Scene跟踪身份与候选一致。
        placement_ids: (0..planning::MAX_PLACEMENTS_PER_SCENE)
            .map(|index| format!("placement-{index}"))
            .collect(), // 规划唯一性集合不得在采用帧整表析构。
        scene_ids: HashSet::new(), // 本测试聚焦最大placement集合。
        ready_scene_ids: HashSet::new(), // 无额外Scene信用即可验证计划退役。
        queued_scene_ids: HashSet::new(), // 无额外排队身份。
        scene_checks: VecDeque::new(), // 无额外检查队列。
    };
    let mut state = ScenePlacementRuntimeState::default(); // 构造采用前的正式候选阶段。
    state.pending = Some(pending); // 完整计划仍供候选根生成使用。
    state.scene_load = Some(tracker); // Scene身份仍用于采用前热更事件门。
    retirement::retire_plan_data(&mut state); // 采用边界只移动两份所有权。
    assert!(state.pending.is_none() && state.scene_load.is_none()); // 采用后旧候选身份立即失效。
    let retired = state.retired_plan.as_ref().expect("候选进入唯一退役槽");
    assert_eq!(
        retired
            .candidate
            .as_ref()
            .expect("强计划仍由退役槽持有")
            .plans
            .len(),
        planning::MAX_PLACEMENTS_PER_SCENE,
    ); // 采用调用本身没有同步析构大Vec。
    assert_eq!(
        retired
            .candidate
            .as_ref()
            .expect("Scene跟踪器与计划同代退役")
            .placement_ids
            .len(),
        planning::MAX_PLACEMENTS_PER_SCENE,
    ); // 稳定ID集合同样完整转移。

    let mut app = App::new(); // 使用生产清理系统验证采用后的退役预算。
    app.insert_resource(state)
        .add_systems(Update, retirement::cleanup_retired_placement_plan);
    app.update(); // 第一帧优先释放pending计划。
    let retired_candidate = app
        .world()
        .resource::<ScenePlacementRuntimeState>()
        .retired_plan
        .as_ref()
        .and_then(|retired| retired.candidate.as_ref())
        .expect("首帧后仍有强计划");
    let remaining_plans =
        retired_candidate.plans.len() + usize::from(retired_candidate.current_plan.is_some()); // O(1)取出的下一计划仍由嵌套游标完整持有。
    assert_eq!(
        remaining_plans,
        planning::MAX_PLACEMENTS_PER_SCENE - retirement::PLACEMENT_PLAN_RETIRE_BUDGET_PER_FRAME,
    ); // 最坏候选采用后每帧只释放一百二十八条计划。
}

#[test]
fn nested_water_paths_cannot_hide_inside_one_top_level_plan_drop() {
    let scene_id = SceneId::parse("MAP-placement-retire-water-paths").expect("测试场景ID合法"); // 构造一个最大单根水绑定计划。
    let mut plan = pending_plan(0); // 从普通合法计划添加嵌套水路径。
    plan.record.water_surfaces = (0..MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_PLACEMENT)
        .map(|surface| ScenePlacementWaterSurface {
            mesh_index: surface as u32, // 诊断索引不影响退役身份。
            node_path: (0..64).map(|segment| format!("p{segment}")).collect(), // 每个绑定命中六十四段路径深度上限。
            mesh_name: format!("水面_{surface}"), // 每项保留独立有界网格名。
            kind: ScenePlacementWaterKind::Sloped,  // 固定大小枚举在转游标时直接释放。
            flow_speed: 1.0,                      // 固定大小数值不占字符串预算。
            height_offset: 0.0,                   // 固定大小数值不占字符串预算。
        })
        .collect(); // 一个PendingPlacementPlan内部共有四千零九十六个路径段。
    let pending = PendingPlacementRevision {
        terrain_holes: Default::default(),
        terrain_patch_count: 0,
        generation: 1,                                           // 测试候选身份。
        revision: 1,                                             // 测试候选修订。
        scene_id,                                                  // 测试场景身份。
        building_preset_revision: TEST_BUILDING_PRESET_REVISION, // 候选目录身份随计划退役。
        building_clearances: BuildingVegetationClearanceState::empty_regions(), // 本测试不包含建筑。
        building_clearance_floor_heights: Default::default(),
        plans: vec![plan], // 只有一个顶层计划，防止顶层数量掩盖嵌套工作。
    };
    let mut state = ScenePlacementRuntimeState::default(); // 构造即将采用的候选。
    state.pending = Some(pending); // 强计划仍归当前候选。
    retirement::retire_plan_data(&mut state); // 只转换为拥有型嵌套游标。
    let advance =
        retirement::advance_retired_plan(state.retired_plan.as_mut().expect("单计划已进入退役槽")); // 精确推进一个生产帧预算。
    assert_eq!(
        advance.path_segments,
        retirement::PLACEMENT_PLAN_RETIRE_PATH_BUDGET_PER_FRAME,
    ); // 单个计划也只能释放五百一十二段路径。
    assert!(advance.water_surfaces <= retirement::PLACEMENT_PLAN_RETIRE_WATER_BUDGET_PER_FRAME); // 完整水绑定数量另受六十四项门限制。
    assert_eq!(advance.top_level_items, 0); // 嵌套路径尚未清空时不能提前计为已释放计划。
    assert!(state.retired_plan.is_some()); // 一个合法大计划必须跨帧继续，而不是在当前帧深析构。
}

#[test]
fn nested_water_strings_obey_the_shared_byte_budget() {
    let scene_id = SceneId::parse("MAP-placement-retire-string-bytes").expect("测试场景ID合法"); // 构造合法Unicode最坏字节密度路径。
    let long_segment = "🀄".repeat(256); // 二百五十六字符精确占一千零二十四UTF-8字节。
    let mut plan = pending_plan(0); // 单计划排除顶层数量预算影响。
    plan.record.water_surfaces = (0..MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_PLACEMENT)
        .map(|surface| ScenePlacementWaterSurface {
            mesh_index: surface as u32,               // 固定大小诊断索引。
            node_path: vec![long_segment.clone(); 4], // 每项路径总字符数一千零二十四，仍处于正式上限。
            mesh_name: format!("m{surface}"),         // 小网格名也计入同一字符串预算。
            kind: ScenePlacementWaterKind::Sloped,      // 固定大小语义。
            flow_speed: 1.0,                          // 固定大小流速。
            height_offset: 0.0,                       // 固定大小高度。
        })
        .collect(); // 全部合法绑定合计远超单帧三十二KiB。
    let mut state = ScenePlacementRuntimeState::default(); // 构造采用终止边界。
    state.pending = Some(PendingPlacementRevision {
        terrain_holes: Default::default(),
        terrain_patch_count: 0,
        generation: 1,                                           // 测试代次。
        revision: 1,                                             // 测试修订。
        scene_id,                                                  // 测试场景。
        building_preset_revision: TEST_BUILDING_PRESET_REVISION, // 候选目录身份随计划退役。
        building_clearances: BuildingVegetationClearanceState::empty_regions(), // 本测试不包含建筑。
        building_clearance_floor_heights: Default::default(),
        plans: vec![plan], // 一个计划即可验证共享字节门。
    });
    retirement::retire_plan_data(&mut state); // 转成不析构内部字符串的游标。
    let advance = retirement::advance_retired_plan(
        state
            .retired_plan
            .as_mut()
            .expect("长字符串计划已进入退役槽"),
    );
    assert!(advance.string_bytes <= retirement::PLACEMENT_PLAN_RETIRE_STRING_BYTES_PER_FRAME); // 多个普通合法字符串不能合计越过三十二KiB。
    assert!(advance.path_segments < retirement::PLACEMENT_PLAN_RETIRE_PATH_BUDGET_PER_FRAME); // 本夹具应由字节门先于五百一十二段数量门阻塞。
    assert_eq!(advance.top_level_items, 0); // 未清空嵌套字符串前顶层计划仍由退役槽持有。
}

#[test]
fn scene_load_polling_never_scans_the_full_plan_in_one_frame() {
    let ids = (1..=planning::MAX_PLACEMENTS_PER_SCENE)
        .map(|index| asset_id::<WorldAsset>(1_000 + index as u128))
        .collect::<Vec<_>>(); // 最坏情况下每条placement引用不同GLB。
    let mut tracker = PlacementSceneLoadTracker {
        generation: 3,                                                 // 测试代次。
        revision: 9,                                                   // 测试修订。
        placement_ids: HashSet::new(), // 本测试只验证Scene轮询，规划ID退役另有专项回归。
        scene_ids: ids.iter().copied().collect::<HashSet<_>>(), // O(1)事件归属全集。
        ready_scene_ids: HashSet::new(), // 初始没有任何Ready信用。
        queued_scene_ids: ids.iter().copied().collect::<HashSet<_>>(), // 首轮全部ID都已去重排队。
        scene_checks: ids.iter().copied().collect::<VecDeque<_>>(), // 队列总量可大，单帧弹出量必须小。
    };
    let mut calls = 0usize; // 统计实际状态查询次数。
    let first = loading::poll_scene_loads(&mut tracker, &mut |_id| {
        calls += 1; // 每个弹出ID只查询一次。
        loading::PlacementSceneDependencyState::Waiting // Waiting ID回到队尾，但同帧不再弹出。
    });
    assert_eq!(
        first.checked,
        loading::PLACEMENT_SCENE_LOAD_CHECK_BUDGET_PER_FRAME,
    ); // 首帧精确只查一百二十八项。
    assert_eq!(calls, loading::PLACEMENT_SCENE_LOAD_CHECK_BUDGET_PER_FRAME); // 闭包证明没有隐藏第二遍。
    assert_eq!(tracker.scene_checks.len(), planning::MAX_PLACEMENTS_PER_SCENE); // Waiting轮转不丢ID，也不同帧缩短全队列。

    let mut frames = 0usize; // 所有ID变成Ready必须跨足够多帧。
    while tracker.ready_scene_ids.len() < tracker.scene_ids.len() {
        let advance = loading::poll_scene_loads(&mut tracker, &mut |_id| {
            loading::PlacementSceneDependencyState::Ready // 本轮弹出的唯一ID全部首次Ready。
        });
        assert!(advance.checked <= loading::PLACEMENT_SCENE_LOAD_CHECK_BUDGET_PER_FRAME); // 后续每帧同样受硬预算限制。
        frames += 1; // 统计Ready扫描帧数。
    }
    assert_eq!(
        frames,
        planning::MAX_PLACEMENTS_PER_SCENE / loading::PLACEMENT_SCENE_LOAD_CHECK_BUDGET_PER_FRAME,
    ); // 八千一百九十二个唯一Scene同样需要六十四帧。
    assert!(tracker.scene_checks.is_empty()); // 全Ready后稳定帧不再轮询任何ID。
}
