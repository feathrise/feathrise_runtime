//! 显式地图资产摆放的公共变换范围与纯校验。
//!
//! 【功能设计】F12作者工具、保存预检和运行时候选必须共享同一份变换判定；
//! 不能出现编辑器已经接受、保存后却由动态地图运行时拒载的分叉结果。

use bevy::prelude::*; // Rect、Transform与向量转换保持和正式场景完全一致。

use crate::{MapId, MapPlacementRecord}; // 公共记录和地图身份共同生成可定位诊断。

/// 单张地图允许的显式实例上限；海量重复内容必须继续使用植被后端。
pub const MAX_MAP_PLACEMENTS_PER_MAP: usize = 8_192;
/// 单轴缩放允许的闭区间下界；零或负缩放不得进入场景层级。
pub const MIN_MAP_PLACEMENT_SCALE: f32 = 0.001;
/// 单轴缩放允许的闭区间上界；阻止损坏数据制造异常世界包围盒。
pub const MAX_MAP_PLACEMENT_SCALE: f32 = 1_000.0;

impl MapPlacementRecord {
    /// 校验当前记录的世界变换，并返回运行时与作者预览可直接采用的同一`Transform`。
    pub fn validated_transform(
        &self,          // 当前完整摆放记录。
        _map_id: &MapId, // 保留调用契约；地图容器不再限制实例原点。
        bounds: Rect,   // 清单已经验证过的世界XZ范围。
    ) -> Result<Transform, String> {
        if let Some(binding)=self.terrain_binding.as_ref() {
            binding.validate()?;
            if self.terrain_patch.is_some() { return Err("三维地形载体不能同时保存普通内容关联".into()); }
        }
        self.editor.validate()?; // 编辑器名称和组名在预览、保存及正式加载入口统一受检。
        self.asset_target.validate()?; // 参数化形状不能把零尺寸或损坏材质送入渲染、缓存或物理。
        if self.asset_target.is_builtin() && (!self.asset_path.is_empty() || self.terrain_patch.is_some() || self.building_preset_id.is_some()) {
            return Err("内置立方体不能同时引用外部模型、三维地形或建筑预设".into());
        }
        if let Some(policy) = self.mesh_lod { policy.validate()?; }
        if !self.asset_target.is_scene0() && !self.water_surfaces.is_empty() {
            return Err(format!(
                "实例{}是单primitive摆放，不能保存只适用于Scene0层级的整网格水绑定",
                self.id,
            )); // 子资产没有Blender节点路径树，拒绝把Scene0水语义错误套到直接网格上。
        }
        let translation = Vec3::from_array(self.translation); // 作者数组只转换一次。
        let scale = Vec3::from_array(self.scale); // 三轴必须共同满足有限闭区间。
        if !translation.is_finite()
            || !scale.is_finite()
            || scale.cmplt(Vec3::splat(MIN_MAP_PLACEMENT_SCALE)).any()
            || scale.cmpgt(Vec3::splat(MAX_MAP_PLACEMENT_SCALE)).any()
            || !self.yaw_degrees.is_finite()
        {
            return Err(format!("实例{}的变换无效", self.id)); // 保持既有保存诊断契约，NaN、无穷和非法缩放统一失败关闭。
        }
        if !self.terrain_offset.is_finite() {
            return Err(format!("实例{}的地形偏移无效", self.id)); // 新增元数据使用独立精确诊断，不改写既有变换错误文本。
        }
        if let Some(courtyard) = &self.courtyard {
            courtyard
                .validate()
                .map_err(|error| format!("实例{}的{error}", self.id))?; // 作者预览和正式运行时同时拒绝损坏的成员锚点与身份。
        }
        // 世界位置只需有限；bounds只供旧主地形切洞校验，不能作为地图容器边界。
        self.terrain_cutout(bounds)?; // 首版三维地形约束在预览、保存和正式加载入口一致。
        Ok(Transform::from_translation(translation)
            .with_rotation(Quat::from_rotation_y(self.yaw_degrees.to_radians()))
            .with_scale(scale)) // 唯一受检结果直接供预览与正式候选采用。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 直接锁定公共编辑、保存和运行时共同边界。

    fn record() -> MapPlacementRecord {
        MapPlacementRecord {
            mesh_lod: None,
            terrain_binding: None,
            editor: Default::default(),
            terrain_patch: None,
            id: "placement-test".to_owned(), // 稳定测试实例身份。
            asset_path: "gltf/test.glb".to_owned(), // 变换测试不依赖真实AssetServer。
            asset_target: Default::default(), // 变换基线沿用完整Scene0。
            building_preset_id: None,        // 普通摆放夹具不附带建筑目录语义。
            courtyard: None,                 // 独立摆放记录不属于任何院落副本。
            translation: [0.0, 2.0, 0.0],    // 原点位于测试地图中央。
            height_mode: crate::MapPlacementHeightMode::FixedHeight, // 测试基线模拟旧文件的固定世界高度。
            terrain_offset: 0.0,                                     // 固定高度不携带地形偏移。
            yaw_degrees: 45.0,                                       // 有限角度允许采用。
            scale: [1.0, 1.0, 1.0],                                  // 单位缩放是普通合法基线。
            water_surfaces: Vec::new(),                              // 水体语义由独立公共校验负责。
        }
    }

    #[test]
    fn placement_transform_uses_closed_scale_range_without_container_bounds() {
        let map_id = MapId::parse("MAP-transform-test").expect("测试地图ID合法");
        let bounds = Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0));
        let mut value = record();
        value.scale[0] = MIN_MAP_PLACEMENT_SCALE; // 精确下界应与编辑器数字框一致地合法。
        value.scale[2] = MAX_MAP_PLACEMENT_SCALE; // 精确上界同样允许采用。
        assert!(value.validated_transform(&map_id, bounds).is_ok());

        value.scale[1] = MIN_MAP_PLACEMENT_SCALE * 0.5; // 过小缩放会制造不可操作实例。
        assert!(value.validated_transform(&map_id, bounds).is_err());
        value = record();
        value.translation[0] = 50_000.0; // 超过旧地图范围的有限位置在编辑、保存和运行时均合法。
        assert!(value.validated_transform(&map_id, bounds).is_ok());
        value.translation[0] = f32::INFINITY;
        assert!(value.validated_transform(&map_id, bounds).is_err());
        value = record();
        value.yaw_degrees = f32::NAN; // 非有限旋转不能写入场景层级。
        assert!(value.validated_transform(&map_id, bounds).is_err());
        value = record();
        value.terrain_offset = f32::INFINITY; // 跟随元数据同样不能把非有限值带入保存文件。
        assert!(value.validated_transform(&map_id, bounds).is_err());
    }

    #[test]
    fn placement_mesh_lod_compatibility_round_trips_and_formal_transform_rejects_bad_policy() {
        let mut value = record();
        let old_json = serde_json::to_value(&value).unwrap();
        assert!(old_json.get("mesh_lod").is_none());
        let restored: MapPlacementRecord = serde_json::from_value(old_json).unwrap();
        assert!(restored.mesh_lod.is_none());
        value.mesh_lod = Some(crate::MeshLodPolicy {
            display: crate::MeshLodMode::Disabled,
            collision: crate::MeshLodMode::Override(Default::default()),
        });
        let restored: MapPlacementRecord = serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
        assert_eq!(restored, value);
        let map = MapId::parse("MAP-transform-test").unwrap();
        let bounds = Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0));
        assert!(restored.validated_transform(&map, bounds).is_ok());
        value.mesh_lod.as_mut().unwrap().display = crate::MeshLodMode::Override(crate::MeshDisplayLodSettings {
            max_error_pixels: f32::NAN, ..Default::default()
        });
        assert!(value.validated_transform(&map, bounds).unwrap_err().contains("显示LOD"));
    }

    #[test]
    fn legacy_placement_json_defaults_to_fixed_height() {
        let legacy = r#"{
            "id":"legacy-tree",
            "asset_path":"gltf/tree.glb",
            "translation":[1.0,2.0,3.0],
            "yaw_degrees":0.0,
            "scale":[1.0,1.0,1.0]
        }"#; // 历史文件没有高度模式和离地偏移字段。
        let record: MapPlacementRecord =
            serde_json::from_str(legacy).expect("旧摆放JSON应继续可读");

        assert_eq!(
            record.height_mode,
            crate::MapPlacementHeightMode::FixedHeight
        ); // 旧桥梁、建筑与悬空装饰不得被自动贴地。
        assert_eq!(record.terrain_offset, 0.0); // 缺失偏移使用无噪音默认值。
        assert_eq!(record.asset_target, crate::MapPlacementAssetTarget::Scene0); // 缺失文件内目标继续实例化历史Scene0。
        assert!(record.courtyard.is_none()); // 历史单体实例不能被自动归入院落。
        assert!(record.terrain_binding.is_none()); // 未关联的世界固定内容不能因新格式自动移动。
        let encoded = serde_json::to_value(&record).expect("兼容记录应可重新序列化");
        assert!(encoded.get("asset_target").is_none()); // 默认目标不制造旧地图迁移噪音。
        assert!(encoded.get("courtyard").is_none()); // 旧地图再次保存不新增空组合字段。
        assert!(encoded.get("terrain_binding").is_none());
    }

    fn courtyard_member(member_id: &str) -> crate::MapPlacementCourtyardMembership {
        crate::MapPlacementCourtyardMembership {
            instance_id: "courtyard-instance-1".to_owned(), // 两栋房共享一次落印的副本身份。
            preset_id: "courtyard-small".to_owned(),        // 全局院落模板引用不依赖数组下标。
            member_id: member_id.to_owned(),                // 每个模板成员有独立稳定身份。
            origin: [2.0, 0.5, 3.0],                        // 落印锚点与成员自身平移相互独立。
            yaw_degrees: 90.0,                              // 整体朝向在地图中持久化。
        }
    }

    fn courtyard_document() -> crate::MapPlacementsAsset {
        let mut house = record(); // 现有普通实例被纳入院落副本。
        house.courtyard = Some(courtyard_member("main-house")); // 主屋保留模板成员身份。
        let mut gate = record(); // 第二成员允许独立调整自身变换。
        gate.id = "placement-gate".to_owned(); // placement ID 与 member ID 分别承担不同职责。
        gate.translation = [5.0, 2.0, 6.0]; // 局部修改不重写共同落印锚点。
        gate.courtyard = Some(courtyard_member("gate")); // 同一副本的另一成员。
        crate::MapPlacementsAsset {
            schema_version: crate::MAP_SCHEMA_VERSION, // 测试使用正式地图格式版本。
            map_id: MapId::parse("MAP-courtyard-test").expect("测试地图ID合法"), // 完整文档可序列化。
            placements: vec![house, gate], // 成员仍是普通摆放记录，不引入第二套运行时实体。
        }
    }

    #[test]
    fn courtyard_membership_round_trips_without_replacing_member_transform() {
        let document = courtyard_document(); // 两名不同成员共享合法副本元数据。
        document
            .validate_courtyards()
            .expect("合法院落副本应通过整图校验"); // 保存前能够确认唯一与一致。
        let json = serde_json::to_string(&document).expect("院落地图应可序列化"); // 实际JSON保留成员身份。
        let decoded: crate::MapPlacementsAsset =
            serde_json::from_str(&json).expect("院落地图应可读回"); // 重载使用同一格式。
        decoded
            .validate_courtyards()
            .expect("重载后院落身份应保持合法"); // 保存重载不破坏组合关系。
        assert_eq!(decoded.placements[1].translation, [5.0, 2.0, 6.0]); // 门的局部修改未被模板锚点覆盖。
        assert_eq!(
            decoded.placements[1].courtyard,
            document.placements[1].courtyard
        ); // 副本与成员身份完整往返。
    }

    #[test]
    fn courtyard_membership_rejects_invalid_identity_anchor_and_duplicate_member() {
        let mut document = courtyard_document(); // 从完整合法文档逐项破坏关键不变量。
        document.placements[1].courtyard.as_mut().unwrap().member_id = "main-house".to_owned(); // 同一副本不能有两个相同成员。
        assert!(document.validate_courtyards().is_err()); // 单体复制若不清除身份会被保存门拒绝。
        document = courtyard_document();
        document.placements[1].courtyard.as_mut().unwrap().origin[0] = 4.0; // 同组锚点不能随个别房屋改变。
        assert!(document.validate_courtyards().is_err()); // 成员间的落印变换必须一致。
        document = courtyard_document();
        document.placements[1]
            .courtyard
            .as_mut()
            .unwrap()
            .yaw_degrees = 180.0; // 同组朝向也是共享落印变换。
        assert!(document.validate_courtyards().is_err()); // 旋转元数据冲突不能被成员顺序掩盖。
        document = courtyard_document();
        document.placements[1].courtyard.as_mut().unwrap().preset_id = "other".to_owned(); // 同一副本不能引用两个模板。
        assert!(document.validate_courtyards().is_err()); // 目录引用冲突不能依赖数组顺序解释。
        document = courtyard_document();
        document.placements[0].courtyard.as_mut().unwrap().member_id = "bad/path".to_owned(); // 身份不能含路径语义。
        assert!(document.validate_courtyards().is_err()); // 单项ID先于整图集合检查。
        document = courtyard_document();
        document.placements[0]
            .courtyard
            .as_mut()
            .unwrap()
            .instance_id = "x".repeat(129); // 超长副本身份不得进入地图索引。
        assert!(document.validate_courtyards().is_err()); // 所有成员身份共享128字节硬门。
        document = courtyard_document();
        document.placements[0]
            .courtyard
            .as_mut()
            .unwrap()
            .yaw_degrees = f32::NAN; // 非有限锚点不能保存。
        assert!(document.validate_courtyards().is_err()); // 有限数值是整图一致性的前提。
    }

    #[test]
    fn following_height_mode_round_trips_with_offset() {
        let mut value = record(); // 从兼容固定模式切到新格式。
        value.height_mode = crate::MapPlacementHeightMode::FollowTerrain; // 新作者实例显式保存跟随语义。
        value.terrain_offset = 1.25; // 正偏移可表达树根、底座或悬浮物离地高度。
        let json = serde_json::to_string(&value).expect("跟随记录应可序列化");
        let decoded: MapPlacementRecord =
            serde_json::from_str(&json).expect("跟随记录应可反序列化");

        assert_eq!(
            decoded.height_mode,
            crate::MapPlacementHeightMode::FollowTerrain
        ); // 模式不能在保存后退化为固定高度。
        assert_eq!(decoded.terrain_offset, 1.25); // 离地偏移必须保持完整作者值。
    }

    #[test]
    fn primitive_target_round_trips_with_original_material_reference() {
        let mut value = record();
        value.asset_target = crate::MapPlacementAssetTarget::Primitive {
            mesh: 6,
            primitive: 2,
            material: Some(4),
        }; // 预览中选中的精确primitive及原关联材质共同进入作者记录。
        let json = serde_json::to_string(&value).expect("子资产记录应可序列化");
        let decoded: MapPlacementRecord =
            serde_json::from_str(&json).expect("子资产记录应可重新载入");

        assert_eq!(decoded.asset_target, value.asset_target);
        assert_eq!(
            decoded
                .asset_target
                .mesh_reference(&decoded.asset_path)
                .as_deref(),
            Some("gltf/test.glb#Mesh6/Primitive2"),
        );
        assert_eq!(
            decoded
                .asset_target
                .material_reference(&decoded.asset_path)
                .as_deref(),
            Some("gltf/test.glb#Material4/std"),
        ); // 重新进入地图仍能请求同一Mesh与StandardMaterial标签。
    }

    #[test]
    fn builtin_cuboid_round_trips_without_external_asset_dependency() {
        let mut value = record();
        value.asset_path.clear();
        value.asset_target = crate::MapPlacementAssetTarget::Cuboid {
            size_mm: [1_250, 2_000, 3_500], color_srgb: [30, 90, 180], roughness_milli: 250, collision: false,
        };
        let json = serde_json::to_string(&value).unwrap();
        let decoded: MapPlacementRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, value);
        assert!(decoded.asset_target.mesh_reference("").is_none());
        assert!(decoded.asset_target.material_reference("").is_none());
        assert!(decoded.validated_transform(&MapId::parse("MAP-cuboid").unwrap(), Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0))).is_ok());
    }

    #[test]
    fn builtin_cuboid_rejects_bad_dimensions_material_and_ambiguous_source() {
        let mut value = record();
        value.asset_path.clear();
        let bounds = Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0));
        let map = MapId::parse("MAP-cuboid-validation").unwrap();
        for (size_mm, roughness_milli) in [([0, 2_000, 2_000], 700), ([10_000_001; 3], 700), ([2_000; 3], 1_001)] {
            value.asset_target = crate::MapPlacementAssetTarget::Cuboid { size_mm, color_srgb: [160; 3], roughness_milli, collision: true };
            assert!(value.validated_transform(&map, bounds).is_err());
        }
        value.asset_target = crate::MapPlacementAssetTarget::DEFAULT_CUBOID;
        value.asset_path = "gltf/ambiguous.glb".into();
        assert!(value.validated_transform(&map, bounds).is_err());
    }

    #[test]
    fn editable_cuboid_round_trips_the_complete_record_without_an_external_asset() {
        let mut value = record();
        value.asset_path.clear();
        value.asset_target = crate::MapPlacementAssetTarget::DEFAULT_CUBOID.into_editable().unwrap();
        let crate::MapPlacementAssetTarget::EditableCuboid { ref mut vertices_half_mm, .. } = value.asset_target else { unreachable!() };
        vertices_half_mm[7] = [2_500, 2_800, 1_700];
        let json = serde_json::to_string(&value).unwrap();
        let decoded: MapPlacementRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, value);
        assert!(json.contains("vertices_half_mm"));
        let map = MapId::parse("MAP-editable-cuboid").unwrap();
        let bounds = Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0));
        assert!(decoded.validated_transform(&map, bounds).is_ok());
        let mut ambiguous = decoded;
        ambiguous.asset_path = "gltf/ambiguous.glb".into();
        assert!(ambiguous.validated_transform(&map, bounds).is_err());
    }

    #[test]
    fn primitive_target_rejects_scene_hierarchy_water_bindings() {
        let mut value = record();
        value.asset_target = crate::MapPlacementAssetTarget::Primitive {
            mesh: 1,
            primitive: 0,
            material: None,
        };
        value.water_surfaces.push(crate::MapPlacementWaterSurface {
            mesh_index: 1,
            node_path: vec!["水面对象".to_owned()],
            mesh_name: "水面网格".to_owned(),
            kind: crate::MapPlacementWaterKind::Horizontal,
            flow_speed: 1.0,
            height_offset: 0.0,
        });
        let map_id = MapId::parse("MAP-primitive-water").expect("测试地图ID合法");
        let error = value
            .validated_transform(
                &map_id,
                Rect::from_corners(Vec2::splat(-10.0), Vec2::splat(10.0)),
            )
            .expect_err("直接primitive没有Scene0节点树，必须拒绝整网格水绑定");

        assert!(error.contains("单primitive"));
    }
}
