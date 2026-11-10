//! 地形描述兼容读取与可配置区域共享碰撞的默认值、规格校验。
use crate::{SceneCollisionLod, SparseHeightSpec}; // 作者数据使用同一格距基准。
use serde::Deserialize; // 所有Serde入口执行同一兼容迁移，不限Bevy加载器。

impl<'de> Deserialize<'de> for crate::SceneTerrainDescriptor {
    // 直接磁盘读、热加载和作者保存使用相同合同。
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 显式空列表与缺字段分别处理。
        let mut data = serde_json::Value::deserialize(deserializer)?; // 场景清单本来就是JSON协议。
        let result = decode_terrain(&mut data).map_err(serde::de::Error::custom); // 统一把校验错误返回加载器。
        result // 不在反序列化时写盘。
    } // 旧场景下次作者保存才迁移文件。
} // 不改地形作者高度或显示LOD。

fn field<T: serde::de::DeserializeOwned>(
    data: &mut serde_json::Value,
    name: &str,
) -> Result<T, String> {
    // required字段缺失明确报错。
    let value = data
        .as_object_mut()
        .ok_or("地形描述必须为JSON对象")?
        .remove(name)
        .ok_or_else(|| format!("地形描述缺少{name}"))?; // 取走字段避免复制大数组。
    serde_json::from_value(value).map_err(|error| format!("地形{name}无效：{error}")) // 保留字段名诊断。
} // 只有小配置向量，不涉及高度页。

fn optional<T: serde::de::DeserializeOwned + Default>(
    data: &mut serde_json::Value,
    name: &str,
) -> Result<T, String> {
    // 旧字段可缺省。
    if data.get(name).is_some() {
        field(data, name)
    } else {
        Ok(T::default())
    } // 显式null继续按类型校验。
} // 不偷偷修复显式坏数据。

/// 缺字段迁移和新建初始化可依据已有分辨率收紧初始倍率，之后不自动调整作者值。
pub fn default_collision_lods_for_resolution(resolution: [u32; 2]) -> Vec<SceneCollisionLod> {
    // 无高度IO即可得出最高父页级别。
    let pages = resolution[0]
        .saturating_sub(1)
        .div_ceil(256)
        .max(resolution[1].saturating_sub(1).div_ceil(256))
        .max(1); // 对齐SparseHeightSpec最高Mip算法。
    let max_multiplier = pages.next_power_of_two().min(256); // 页数上取二次幂即最高可用倍率。
    let mut lods = default_collision_lods(); // 用户确认统一默认，不推测旧双层参数。
    lods[1].spacing_multiplier = 4.min(max_multiplier); // 只在缺字段初始化时适配小场景。
    lods // 显式保存列表从不经过这条初始化路径。
} // 半径固定保留16米和64米。

fn decode_terrain(data: &mut serde_json::Value) -> Result<crate::SceneTerrainDescriptor, String> {
    // 兼容数据读取唯一入口。
    let kind: crate::SceneTerrainKind = field(data, "kind")?; // 只有SparseClipmap消费新物理列表。
    let resolution = field(data, "resolution")?; // 作者格点数保持原值。
    let collision_lods = if data.get("collision_lods").is_some() {
        field(data, "collision_lods")?
    } else if kind == crate::SceneTerrainKind::SparseClipmap {
        default_collision_lods_for_resolution(resolution)
    } else {
        Vec::new()
    }; // 缺字段迁移与显式空列表严格区分。
    if kind == crate::SceneTerrainKind::SparseClipmap && collision_lods.is_empty() {
        return Err("自定义碰撞至少需要一层LOD，显式空列表不能作为默认值".into());
    } // 防止损坏清单无碰撞进入游戏。
    let display_lods = if data.get("display_lods").is_some() {
        let lods: Vec<crate::SceneTerrainDisplayLod> = field(data, "display_lods")?; // 字段存在时null也必须按坏数据拒绝。
        if kind != crate::SceneTerrainKind::SparseClipmap {
            return Err("只有SparseClipmap地形可以保存display_lods".into()); // GLB路线不得携带无效死配置。
        } // 完成地形类型边界检查。
        crate::validate_display_lod_shape(&lods)?; // 反序列化边界先执行不依赖场景规格的完整结构校验。
        Some(lods) // 显式列表与旧字段缺失保持可区分。
    } else {
        None // 旧场景继续使用现有64半格自动显示算法，普通保存不写入新字段。
    }; // 完成显示LOD兼容读取。
    Ok(crate::SceneTerrainDescriptor {
        resolved_stitches: Default::default(),
        // 所有旧物理字段只兼容读，已无运行路线。
        kind,                                              // 原地形类型不迁移。
        collision_mode: optional(data, "collision_mode")?, // 旧两种字符串由单变体别名规范化。
        collision_lods,                                    // 完整有效作者配置或缺字段初始化。
        display_lods, // 显式作者列表或旧自动模式身份原样进入编辑器和运行时。
        player_collision_preload_radius_meters: if data
            .get("player_collision_preload_radius_meters")
            .is_some()
        {
            field(data, "player_collision_preload_radius_meters")?
        } else {
            16.0
        }, // 过渡只读兼容字段。
        role_collision_preload_radius_meters: optional(
            data,
            "role_collision_preload_radius_meters",
        )?, // 不参与新方案推导。
        player_collision_chunk_cells: optional(data, "player_collision_chunk_cells")?, // 不修改作者精度。
        role_collision_chunk_cells: optional(data, "role_collision_chunk_cells")?, // 只读旧格式。
        player_collision_span_meters: optional(data, "player_collision_span_meters")?, // 下一保存不再序列化。
        role_collision_span_meters: optional(data, "role_collision_span_meters")?, // 下一保存不再序列化。
        glb_path: field(data, "glb_path")?, // GLB路线保持原资产路径。
        height_path: optional(data, "height_path")?, // 稀疏清单路径不改。
        height_texture_path: optional(data, "height_texture_path")?, // 显示资源不改。
        resolution,                         // 保留作者端点数。
        base_height: field(data, "base_height")?, // 编码量程不改。
        height_scale: field(data, "height_scale")?, // 不改地形高度数据。
    }) // 所有消费者见到同一规范化结果。
} // 未知字段依旧不影响读取。

/// 所有稀疏场景统一使用这组起点；小场景初始化依据真实高度层级适配倍率。
pub fn default_collision_lods() -> Vec<SceneCollisionLod> {
    // 层数是可编辑列表，不是固定两档。
    vec![
        // 用户可以继续添加或删除条目。
        SceneCollisionLod {
            max_distance_meters: 16.0,
            spacing_multiplier: 1,
        }, // 最近区域作者最高精度。
        SceneCollisionLod {
            max_distance_meters: 64.0,
            spacing_multiplier: 4,
        }, // 外围每轴格距四倍。
    ] // 不推断显示覆盖和显示精度。
} // 默认不意味着所有规格均支持四倍，确认时严格校验。

/// 纯结构校验；实际块预算还须由物理规划器按场景边界和接缝校验。
pub fn validate_collision_lods(
    lods: &[SceneCollisionLod],
    spec: &SparseHeightSpec,
) -> Result<(), String> {
    // 两个入口复用相同错误语义。
    if lods.is_empty() {
        return Err("自定义碰撞至少需要一层LOD".into());
    } // 不允许空配置冒充远处无碰撞。
    let mut previous_distance = 0.0; // 第一层也必须拥有正覆盖距离。
    let mut previous_multiplier = 1; // 向外不允许反向加细。
    for (index, lod) in lods.iter().enumerate() {
        // 检查全部用户条目，不截断为五层。
        if !lod.max_distance_meters.is_finite() || lod.max_distance_meters <= previous_distance {
            // 包含NaN与重复距离。
            return Err(format!("碰撞LOD {index} 的距离必须有限、为正且逐层递增")); // 明确定位无效条目。
        } // 保留输入，不静默排序。
        if !lod.spacing_multiplier.is_power_of_two() || lod.spacing_multiplier < previous_multiplier
        {
            // 零及非二次幂均拒绝。
            return Err(format!(
                "碰撞LOD {index} 的倍率必须为1、2、4、8等且向外不减小"
            )); // 不隐式量化倍率。
        } // 层数与倍率无固定对应关系。
        if lod.spacing_multiplier.trailing_zeros() > u32::from(spec.max_mip_level()) {
            // 使用现有高度页层级上界。
            return Err(format!(
                "碰撞LOD {index} 的倍率超过此场景高度层级（最高{}倍）",
                1_u32 << spec.max_mip_level()
            )); // 不回退到不同精度。
        } // 真正块数预算由固定块规划验证。
        previous_distance = lod.max_distance_meters; // 下一层使用累计半径比较。
        previous_multiplier = lod.spacing_multiplier; // 相同倍率的不同距离仍合法。
    } // 所有层均合法才允许保存。
    Ok(()) // 配置不修改作者源。
} // 运行时可在后台规划前复用。

#[cfg(test)] // 配置和兼容迁移无需启动物理或渲染。
mod tests {
    // 真实Serde协议回归。
    use super::*; // 共享正式校验函数。
    fn source() -> serde_json::Value {
        serde_json::json!({"kind":"sparse_clipmap","glb_path":"","resolution":[4001,4001],"base_height":-80.0,"height_scale":160.0})
    } // 500米/0.125米的真实规格。
    #[test] // 缺字段和两种旧字符串都迁移为同一默认列表。
    fn missing_list_and_old_modes_migrate_to_defaults() {
        // 不推测旧两条滑块的意图。
        for mode in [None, Some("legacy_chunks"), Some("shared_display_mesh")] {
            // 覆盖旧协议三种表达。
            let mut json = source(); // 独立夹具。
            if let Some(mode) = mode {
                json["collision_mode"] = mode.into();
            } // 显式旧字符串读取别名。
            json["player_collision_span_meters"] = 31.0.into(); // 用户要求忽略旧半径统一默认。
            let terrain: crate::SceneTerrainDescriptor = serde_json::from_value(json).unwrap(); // 所有入口统一反序列化。
            assert_eq!(terrain.collision_lods, default_collision_lods()); // 不继承旧半径。
            let saved = serde_json::to_value(terrain).unwrap(); // 下一保存规范化新格式。
            assert!(saved.get("collision_mode").is_none()); // 删除旧模式字段。
            assert!(saved.get("player_collision_span_meters").is_none()); // 删除旧联动字段。
            assert_eq!(saved["collision_lods"][1]["spacing_multiplier"], 4); // 格距倍率四倍保持精确。
        } // 不需要批量改写现有场景文件。
    } // 迁移不改变高度数据。
    #[test] // 小图初始化允许适配，显式列表不被自动修正。
    fn small_scene_default_adapts_but_explicit_empty_and_invalid_list_do_not() {
        // 缺字段和空字段不是同义词。
        let mut json = source();
        json["resolution"] = serde_json::json!([513, 513]); // 此图最高只有二倍父页。
        let initial: crate::SceneTerrainDescriptor = serde_json::from_value(json.clone()).unwrap(); // 缺列表初始化。
        assert_eq!(initial.collision_lods[1].spacing_multiplier, 2); // 唯一自动适配时机。
        json["collision_lods"] = serde_json::json!([]); // 显式空数据必须失败。
        assert!(serde_json::from_value::<crate::SceneTerrainDescriptor>(json.clone()).is_err()); // 不偷偷恢复默认。
        json["collision_lods"] = serde_json::to_value(default_collision_lods()).unwrap(); // 显式四倍仍保留，交给规格校验拒绝。
        let explicit: crate::SceneTerrainDescriptor = serde_json::from_value(json).unwrap(); // 作者值不被改写。
        let spec = SparseHeightSpec::new([0.0; 2], [64.0; 2], 0.125, -80.0, 160.0).unwrap(); // 二倍最高页。
        assert!(validate_collision_lods(&explicit.collision_lods, &spec).is_err()); // 超出层级明确拒绝。
    } // 不以回退倍率掩盖坏配置。
    #[test] // 层数无五层上限，距离与倍率各有独立约束。
    fn configurable_count_and_invalid_values() {
        // 只验证结构与高度页上限。
        let spec = SparseHeightSpec::new([0.0; 2], [8192.0; 2], 0.125, -80.0, 160.0).unwrap(); // 合法最大规格。
        for count in [1, 2, 5, 8] {
            let lods: Vec<_> = (0..count)
                .map(|index| SceneCollisionLod {
                    max_distance_meters: (index + 1) as f32,
                    spacing_multiplier: 1,
                })
                .collect();
            assert!(validate_collision_lods(&lods, &spec).is_ok());
        } // 不用层数推导精度。
        for distance in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                validate_collision_lods(
                    &[SceneCollisionLod {
                        max_distance_meters: distance,
                        spacing_multiplier: 1
                    }],
                    &spec
                )
                .is_err()
            );
        } // 非有限半径不得进入规划。
        for multiplier in [0, 3, 512] {
            assert!(
                validate_collision_lods(
                    &[SceneCollisionLod {
                        max_distance_meters: 16.0,
                        spacing_multiplier: multiplier
                    }],
                    &spec
                )
                .is_err()
            );
        } // 二次幂和最高Mip双门。
        let mut lods = default_collision_lods();
        lods[1].max_distance_meters = 16.0;
        assert!(validate_collision_lods(&lods, &spec).is_err()); // 累计距离必须严格递增。
        lods[1].max_distance_meters = 64.0;
        lods[0].spacing_multiplier = 8;
        assert!(validate_collision_lods(&lods, &spec).is_err()); // 向外不允许反向加细。
    } // 真实块数由场景规划测试覆盖。
    #[test] // GLB旧数据读取兼容，不加入不使用的新字段。
    fn glb_stays_without_lod_fields() {
        let mut json = source();
        json["kind"] = "gltf".into();
        let terrain: crate::SceneTerrainDescriptor = serde_json::from_value(json).unwrap();
        assert!(terrain.collision_lods.is_empty());
        assert!(
            serde_json::to_value(terrain)
                .unwrap()
                .get("collision_lods")
                .is_none()
        );
    } // 保持成熟GLB路径不变。

    #[test] // 显示LOD缺字段、显式列表和坏数据在Serde边界保持严格可区分。
    fn display_lod_serde_preserves_legacy_none_and_rejects_invalid_explicit_values() {
        let legacy: crate::SceneTerrainDescriptor = serde_json::from_value(source()).unwrap();
        assert_eq!(legacy.display_lods, None); // 旧图继续使用64半格自动模式。
        assert!(
            serde_json::to_value(legacy)
                .unwrap()
                .get("display_lods")
                .is_none()
        ); // 普通资料保存不得顺带迁移旧图。

        let mut single = source();
        single["display_lods"] = serde_json::json!([
            {"spacing_multiplier": 1}
        ]); // 单层省略范围即覆盖场景边界。
        let single: crate::SceneTerrainDescriptor = serde_json::from_value(single).unwrap();
        assert_eq!(
            single.display_lods,
            Some(vec![crate::SceneTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 1,
            }])
        );

        let mut multi = source();
        multi["display_lods"] = serde_json::json!([
            {"outer_half_extent_meters": 8.0, "spacing_multiplier": 1},
            {"outer_half_extent_meters": 32.0, "spacing_multiplier": 2},
            {"spacing_multiplier": 8}
        ]); // 跳级由编译器插入内部2:1过渡环，不改序列化列表。
        let decoded: crate::SceneTerrainDescriptor = serde_json::from_value(multi.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap()["display_lods"],
            multi["display_lods"]
        );

        for invalid in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!([
                {"spacing_multiplier": 1},
                {"spacing_multiplier": 2}
            ]), // 非末层缺范围。
            serde_json::json!([
                {"outer_half_extent_meters": 8.0, "spacing_multiplier": 1},
                {"outer_half_extent_meters": 16.0, "spacing_multiplier": 2}
            ]), // 末层仍有限。
            serde_json::json!([
                {"outer_half_extent_meters": 0.0, "spacing_multiplier": 1},
                {"spacing_multiplier": 2}
            ]), // 非正距离。
            serde_json::json!([
                {"outer_half_extent_meters": 8.0, "spacing_multiplier": 3},
                {"spacing_multiplier": 4}
            ]), // 非二次幂。
        ] {
            let mut json = source();
            json["display_lods"] = invalid;
            assert!(serde_json::from_value::<crate::SceneTerrainDescriptor>(json).is_err());
        }

        let mut glb = source();
        glb["kind"] = "gltf".into();
        glb["display_lods"] = serde_json::json!([{"spacing_multiplier": 1}]);
        assert!(serde_json::from_value::<crate::SceneTerrainDescriptor>(glb).is_err()); // GLB路线不得携带死配置。
    }
} // 无文件写入的快速回归。
