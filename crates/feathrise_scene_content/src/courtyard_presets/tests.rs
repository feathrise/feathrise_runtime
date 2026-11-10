//! 院落目录格式、严格校验和失败热更保留旧快照的回归。

use super::*; // 测试公开作者格式与已解析目录 API。
use crate::{
    BuildingPresetCatalogAsset, CourtyardPresetRegistry, ResolvedBuildingPresetCatalog,
    systems::apply_courtyard_preset_catalog,
}; // 随包样板同时验证建筑引用和生产采用系统。

/// 构造一个带明确成员模型来源的合法组合。
fn preset(id: &str) -> CourtyardPresetDescriptor {
    CourtyardPresetDescriptor {
        id: id.to_owned(),          // 测试调用方控制组合稳定身份。
        label: format!("院落{id}"), // 中文展示名与技术 ID 独立。
        members: vec![CourtyardMemberDescriptor {
            mesh_lod: None,
            id: "main-house".to_owned(), // 后续局部覆盖可追踪的成员身份。
            asset_path: "gltf/buildings/house.glb".to_owned(), // 相对 assets 根的完整场景来源。
            asset_target: ScenePlacementAssetTarget::Scene0, // 默认使用整栋房屋场景。
            building_preset_id: Some("house-a".to_owned()), // 建筑规则将在实际印章时解析。
            local_translation: [2.0, 0.0, -3.0], // 模型位于院落枢轴的局部位置。
            local_yaw_degrees: 90.0,     // 成员局部朝向需与院落整体朝向相加。
            scale: [1.0, 1.0, 1.0],      // 适用普通显式摆放缩放预算。
            height_mode: ScenePlacementHeightMode::FixedHeight, // 固定高度模板的兼容基线。
            terrain_offset: 0.0,         // 固定高度不额外请求贴地偏移。
        }],
    }
}

/// 构造一份只有单个合法院落的目录。
fn catalog(preset_id: &str) -> CourtyardPresetCatalogAsset {
    CourtyardPresetCatalogAsset {
        schema_version: COURTYARD_PRESET_SCHEMA_VERSION, // 当前格式必须明确声明。
        presets: vec![preset(preset_id)],                // 完整目录只含一个组合，便于观察热更替换。
    }
}

#[test]
fn legacy_placements_remain_independent_of_shipped_courtyard_sample() {
    let old_placement = r#"{"id":"old-house","asset_path":"gltf/old.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]}"#; // 历史摆放没有任何院落字段。
    let placement: crate::ScenePlacementRecord =
        serde_json::from_str(old_placement).expect("旧摆放仍可反序列化"); // 新目录不应成为旧场景的隐式必填依赖。
    assert_eq!(placement.id, "old-house"); // 原实例身份保持不变。
}

#[test]
fn members_round_trip_scene_target_and_terrain_semantics() {
    let mut source = catalog("yard-a"); // 从合法房屋组合派生显式子资产目标。
    let member = &mut source.presets[0].members[0]; // 本测试只改变唯一成员。
    member.asset_target = ScenePlacementAssetTarget::Primitive {
        mesh: 2,
        primitive: 1,
        material: Some(3),
    }; // 保留精确原关联材质。
    member.height_mode = ScenePlacementHeightMode::FollowTerrain; // 按成员保存贴地策略。
    member.terrain_offset = 0.6; // 贴地时保留离地偏移。
    let json = serde_json::to_string(&source).expect("合法院落目录应可保存"); // 模拟 F12 原子保存前的完整序列化。
    let decoded: CourtyardPresetCatalogAsset =
        serde_json::from_str(&json).expect("院落目录应能重载"); // 模拟资产服务器读取新版本。
    assert_eq!(decoded, source); // 模型目标、身份、局部变换与高度策略均完整保留。
    assert!(ResolvedCourtyardPresetCatalog::build(&decoded).is_ok()); // 往返后仍满足全目录校验。
}

#[test]
fn invalid_ids_paths_targets_and_nonfinite_transforms_reject_whole_catalog() {
    let mut duplicate = catalog("yard-a"); // 第一项是合法模板。
    duplicate.presets.push(preset("yard-a")); // 重复组合 ID 不得覆盖先前内容。
    assert!(duplicate.validate().is_err()); // 全目录身份空间唯一。

    let mut duplicate_member = catalog("yard-a"); // 测试同一组合内的成员身份。
    let repeated_member = duplicate_member.presets[0].members[0].clone(); // 先复制成员，避免同一语句冲突借用目录。
    duplicate_member.presets[0].members.push(repeated_member); // 第二成员占用相同 ID。
    assert!(duplicate_member.validate().is_err()); // 局部覆盖不能产生歧义。

    let mut invalid = catalog("yard-a"); // 每一类非法值独立验证。
    invalid.presets[0].members[0].asset_path = "../escape.glb".to_owned(); // 禁止离开 assets 根。
    assert!(invalid.validate().is_err()); // 路径穿越整份拒绝。
    invalid.presets[0].members[0].asset_path = "gltf/house.glb#Scene0".to_owned(); // 文件路径不得混入 Bevy 子资产标签。
    assert!(invalid.validate().is_err()); // 目标必须走独立结构化字段。
    invalid.presets[0].members[0].asset_path = "gltf/house.glb".to_owned(); // 恢复合法路径。
    invalid.presets[0].members[0].asset_target = ScenePlacementAssetTarget::Primitive {
        mesh: u32::MAX,
        primitive: 0,
        material: None,
    }; // 离谱编号不是可接受静态目标。
    assert!(invalid.validate().is_err()); // 非法目标不能进入可查询快照。
    invalid.presets[0].members[0].asset_target = ScenePlacementAssetTarget::Scene0; // 恢复合法整场景目标。
    invalid.presets[0].members[0].scale[0] = f32::NAN; // NaN 不得进入世界几何。
    assert!(invalid.validate().is_err()); // 非有限变换拒绝整份新目录。
}

#[test]
fn empty_or_over_budget_members_and_wrong_schema_reject_catalog() {
    let mut invalid = catalog("yard-a"); // 单项组合足以测试成员预算。
    invalid.presets[0].members.clear(); // 空组合无法产生有效印章。
    assert!(invalid.validate().is_err()); // 全局空目录合法，单个空预设非法。
    invalid = catalog("yard-a"); // 恢复合法第一成员。
    let member = invalid.presets[0].members[0].clone(); // 每个超额成员需要唯一身份。
    for index in 1..=MAX_COURTYARD_MEMBERS_PER_PRESET {
        let mut extra = member.clone(); // 纯测试生成有限数量成员。
        extra.id = format!("member-{index}"); // 避免先触发重复 ID 校验。
        invalid.presets[0].members.push(extra); // 第 257 项应越过单组合门槛。
    }
    assert!(invalid.validate().is_err()); // 不允许单次组合印章无界扩大。
    invalid = catalog("yard-a"); // 再单独测试格式版本。
    invalid.schema_version += 1; // 不兼容格式不能被猜测读取。
    assert!(invalid.validate().is_err()); // 版本错误拒绝整份目录。
}

#[test]
fn invalid_hot_reload_retains_previous_complete_snapshot_and_revision() {
    let mut app = App::new(); // 纯 ECS 测试无需游戏窗口或真实资产 IO。
    app.init_resource::<Assets<CourtyardPresetCatalogAsset>>() // 提供与资产服务器相同的资产容器。
        .add_message::<AssetEvent<CourtyardPresetCatalogAsset>>() // 生产系统通过类型化事件识别热更新。
        .add_systems(Update, apply_courtyard_preset_catalog); // 执行正式原子采用路径。
    let handle = app
        .world_mut()
        .resource_mut::<Assets<CourtyardPresetCatalogAsset>>()
        .add(catalog("yard-old")); // 保留单个全局目录强句柄。
    let id = handle.id(); // 后续修改必须指向同一全局资产身份。
    app.world_mut().insert_resource(CourtyardPresetRegistry {
        catalog: handle.clone(),      // 注册表持有目录强句柄。
        revision: 0,                  // 首次成功采用前没有有效修订。
        resolved: Default::default(), // 旧快照从空值开始。
        last_error: None,             // 首载无失败。
    });
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<CourtyardPresetCatalogAsset>>>()
        .write(AssetEvent::Added { id }); // 触发第一次完整采用。
    app.update(); // 系统构造旧有效快照。
    assert_eq!(
        app.world().resource::<CourtyardPresetRegistry>().revision,
        1
    ); // 首次采用推进修订。
    assert!(
        app.world()
            .resource::<CourtyardPresetRegistry>()
            .resolve("yard-old")
            .is_some()
    ); // 旧组合已经可查询。

    let mut broken = catalog("yard-new"); // 模拟作者热更时输入一份损坏组合。
    broken.presets[0].members[0].scale[0] = 0.0; // 零缩放使整份目录不合法。
    *app.world_mut()
        .resource_mut::<Assets<CourtyardPresetCatalogAsset>>()
        .get_mut(id)
        .unwrap() = broken; // 资产容器收到新 JSON。
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<CourtyardPresetCatalogAsset>>>()
        .write(AssetEvent::Modified { id }); // 通知系统重新校验。
    app.update(); // 坏热更只更新诊断，不替换快照。
    let registry = app.world().resource::<CourtyardPresetRegistry>(); // 读取完整采用状态。
    assert_eq!(registry.revision, 1); // 失败不能冒充一次新修订。
    assert!(registry.resolve("yard-old").is_some()); // 原有效组合继续服务已有实例。
    assert!(registry.resolve("yard-new").is_none()); // 坏目录不得部分发布新 ID。
    assert!(registry.last_error.is_some()); // UI 可见具体错误。

    *app.world_mut()
        .resource_mut::<Assets<CourtyardPresetCatalogAsset>>()
        .get_mut(id)
        .unwrap() = catalog("yard-new"); // 修复为完整合法目录。
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<CourtyardPresetCatalogAsset>>>()
        .write(AssetEvent::Modified { id }); // 触发下一次全量校验。
    app.update(); // 成功后整份快照一次替换。
    let registry = app.world().resource::<CourtyardPresetRegistry>(); // 确认新旧身份边界。
    assert_eq!(registry.revision, 2); // 仅成功修复提升修订。
    assert!(registry.resolve("yard-old").is_none()); // 完整替换后旧组合不再出现。
    assert!(registry.resolve("yard-new").is_some()); // 新组合可按稳定 ID 查询。
    assert!(registry.last_error.is_none()); // 修复后诊断清除。
}
