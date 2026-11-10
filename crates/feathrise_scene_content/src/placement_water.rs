//! 显式摆放GLB实例的整网格水体覆盖格式与纯校验。
//!
//! 【功能设计】Blender负责复杂水面拓扑、细分与UV，场景作者文件按实例内节点路径与
//! 网格名称失败关闭匹配，网格索引只用于诊断；本模块不生成几何，也不复制动态水材质实现。

use std::collections::BTreeSet; // 同一摆放实例内按确定顺序拒绝重复节点路径与网格名称身份。

use serde::{Deserialize, Serialize}; // 编辑器保存与正式AssetLoader共享同一JSON格式。

use crate::assets::{ScenePlacementRecord, ScenePlacementsAsset}; // 在独立模块为摆放记录和整份文档补充水体覆盖纯校验入口。

/// 单个整网格水体允许的最大作者流速，单位为沿UV0.U下游移动的米每秒。
pub const MAX_SCENE_PLACEMENT_WATER_FLOW_SPEED: f32 = 20.0; // 覆盖平水到瀑布的可见范围，同时阻止异常Uniform输入。

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_PLACEMENT: usize = 64;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_SCENE: usize = 256;

/// 整网格水体允许相对Blender原始顶点高度偏移的绝对上限，单位为米。
pub const MAX_SCENE_PLACEMENT_WATER_HEIGHT_OFFSET_METERS: f32 = 10_000.0; // 覆盖当前最大场景高程且拒绝损坏文件的极端位移。

/// 整网格水体节点路径允许的最大层级深度。
pub const MAX_SCENE_PLACEMENT_WATER_NODE_PATH_DEPTH: usize = 64; // 覆盖复杂Blender集合层级且拒绝损坏文件制造无界路径。

/// 节点路径单段或网格名称允许的最大Unicode字符数。
pub const MAX_SCENE_PLACEMENT_WATER_NAME_CHARS: usize = 256; // 保留可读中文名称并限制单项诊断和比较成本。

/// 整条节点路径允许的最大Unicode字符总数。
pub const MAX_SCENE_PLACEMENT_WATER_NODE_PATH_CHARS: usize = 4_096; // 防止大量短段组合成异常庞大的作者身份。

/// 判断名称是否是Bevy glTF loader为缺少显式名称的节点生成的`GltfNodeN`占位值。
pub fn is_generated_gltf_node_name(name: &str) -> bool {
    let Some(index) = name.strip_prefix("GltfNode") else {
        return false; // 普通Blender名称即使包含相同字样，也不能被误判为loader占位名。
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()) // 仅完整十进制索引后缀属于loader生成规则。
}

/// Blender整网格被场景编辑器赋予的水体表现类别。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)] // 纯值类型可直接进入撤销快照和JSON回归测试。
#[serde(rename_all = "snake_case")] // 作者文件使用稳定的小写下划线名称，不绑定Rust枚举拼写。
pub enum ScenePlacementWaterKind {
    /// 保留网格XZ轮廓并把水面按统一高度偏移处理。
    #[default] // 旧或省略类别的覆盖安全落到最常见的水平水面。
    Horizontal,
    /// 保留Blender网格逐顶点高程，用于沿河下降的连续斜水面。
    Sloped,
    /// 保留陡峭或竖直网格，并启用瀑布方向的流动表现。
    Waterfall,
}

/// 一个摆放GLB实例内针对完整网格的动态水覆盖。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] // 拷贝作者快照并稳定执行Serde往返。
pub struct ScenePlacementWaterSurface {
    /// GLB根资产中的网格索引快照；仅供诊断，不作为最终实例身份或匹配候选。
    pub mesh_index: u32,
    /// 从摆放实例根到Blender对象节点的稳定名称路径；最终接管必须完整匹配。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // 旧试验值允许读取为空，但正式采用前的纯校验会失败关闭。
    pub node_path: Vec<String>, // 路径区分复用同一Mesh datablock的多个Blender对象实例。
    /// Blender网格名称快照；与节点路径共同组成最终失败关闭身份。
    pub mesh_name: String,
    /// 水体表现类别；缺省时按水平水面处理。
    #[serde(default)] // 允许早期试验JSON只声明网格和数值参数。
    pub kind: ScenePlacementWaterKind,
    /// 沿UV0.U下游输运的米每秒流速，必须有限且位于零至正式上限之间。
    pub flow_speed: f32,
    /// 相对Blender原始水面高度的统一米制偏移。
    pub height_offset: f32,
}

impl ScenePlacementWaterSurface {
    /// 校验一项整网格水体的名称与有限数值，唯一性由所属数组统一检查。
    pub fn validate(&self) -> Result<(), String> {
        if self.node_path.is_empty() {
            return Err(format!(
                "GLB网格索引{}的水体覆盖缺少节点路径",
                self.mesh_index,
            )); // 只有mesh_index会误接管共享Mesh datablock的多个对象，必须失败关闭。
        }
        if self.node_path.len() > MAX_SCENE_PLACEMENT_WATER_NODE_PATH_DEPTH {
            return Err(format!(
                "GLB水体节点路径深度{}超过上限{}",
                self.node_path.len(),
                MAX_SCENE_PLACEMENT_WATER_NODE_PATH_DEPTH,
            )); // 异常深层级不得放大运行时匹配与错误诊断成本。
        }
        let mut path_chars = 0usize; // 使用Unicode字符数而非UTF-8字节数约束中文Blender节点名。
        for segment in &self.node_path {
            let segment_chars = segment.chars().count(); // 当前路径段只扫描一次并复用长度结果。
            if segment.trim().is_empty()
                || segment_chars > MAX_SCENE_PLACEMENT_WATER_NAME_CHARS
                || is_generated_gltf_node_name(segment)
            {
                return Err(format!(
                    "GLB水体节点路径包含空白段、loader生成名或超过{}字符的名称",
                    MAX_SCENE_PLACEMENT_WATER_NAME_CHARS,
                )); // 空段、GltfNodeN占位名或超长段都不能形成跨Blender重导出的稳定身份。
            }
            path_chars = path_chars.saturating_add(segment_chars); // 饱和累计让恶意超长输入稳定落入总长拒绝分支。
        }
        if path_chars > MAX_SCENE_PLACEMENT_WATER_NODE_PATH_CHARS {
            return Err(format!(
                "GLB水体节点路径总长{}字符超过上限{}",
                path_chars, MAX_SCENE_PLACEMENT_WATER_NODE_PATH_CHARS,
            )); // 合法单段组成的整条路径仍必须具备固定总预算。
        }
        let mesh_name_chars = self.mesh_name.chars().count(); // 网格名称同样按用户看到的Unicode字符计数。
        if self.mesh_name.trim().is_empty() || mesh_name_chars > MAX_SCENE_PLACEMENT_WATER_NAME_CHARS
        {
            return Err(format!(
                "GLB网格索引{}的水体覆盖缺少名称或名称超过{}字符",
                self.mesh_index, MAX_SCENE_PLACEMENT_WATER_NAME_CHARS,
            )); // 空白或异常超长名称都不能参与最终失败关闭匹配。
        }
        if !self.flow_speed.is_finite()
            || !(0.0..=MAX_SCENE_PLACEMENT_WATER_FLOW_SPEED).contains(&self.flow_speed)
        {
            return Err(format!(
                "GLB网格{}的水体流速必须位于0至{}之间",
                self.mesh_name, MAX_SCENE_PLACEMENT_WATER_FLOW_SPEED,
            )); // NaN、无穷或过大倍率不得进入动态水材质Uniform。
        }
        if !self.height_offset.is_finite()
            || !(-MAX_SCENE_PLACEMENT_WATER_HEIGHT_OFFSET_METERS
                ..=MAX_SCENE_PLACEMENT_WATER_HEIGHT_OFFSET_METERS)
                .contains(&self.height_offset)
        {
            return Err(format!(
                "GLB网格{}的水面高度偏移必须位于正负{}米之间",
                self.mesh_name, MAX_SCENE_PLACEMENT_WATER_HEIGHT_OFFSET_METERS,
            )); // 有界偏移避免损坏作者文件把水面送到不可表示或不可取景的位置。
        }
        Ok(()) // 当前覆盖全部字段均满足公共作者边界。
    }
}

/// 校验同一摆放实例的全部网格水体覆盖，并拒绝重复节点路径与网格名称身份。
pub fn validate_scene_placement_water_surfaces(
    surfaces: &[ScenePlacementWaterSurface], // 空数组合法，代表该实例保持普通GLB材质。
) -> Result<(), String> {
    let mut surface_identities = BTreeSet::<(&[String], &str)>::new(); // 借用作者身份，完整校验不深克隆至多64条路径。
    for surface in surfaces {
        surface.validate()?; // 先拒绝缺路径或名称的旧试验值，避免空身份进入唯一集合。
        let identity = (surface.node_path.as_slice(), surface.mesh_name.as_str()); // 最终身份不依赖可被多个对象复用的Mesh索引。
        if !surface_identities.insert(identity) {
            return Err(format!(
                "GLB节点路径{}下的网格{}被重复赋予水体覆盖",
                surface.node_path.join("/"),
                surface.mesh_name,
            )); // 同一对象网格只能有一个类别、流速和高度偏移所有者。
        }
    }
    Ok(()) // 空数组或全部唯一且合法的覆盖均可进入作者草稿。
}

impl ScenePlacementRecord {
    /// 校验本摆放实例的整网格水体覆盖，不读取文件、资产表或ECS世界。
    pub fn validate_water_surfaces(&self) -> Result<(), String> {
        validate_scene_placement_water_surfaces(&self.water_surfaces) // 编辑器与运行时复用唯一纯校验入口。
    }
}

impl ScenePlacementsAsset {
    /// 校验整份摆放文档的单实例格式与全图整网格水体绑定总预算，不读取文件或ECS世界。
    pub fn validate_water_surfaces(&self) -> Result<(), String> {
        let mut total_surface_count = 0usize; // 按记录顺序累计，超预算时可以指出第一个越界placement。
        for placement in &self.placements {
            total_surface_count = total_surface_count
                .checked_add(placement.water_surfaces.len())
                .ok_or_else(|| "单个场景整网格水体绑定总数计算溢出".to_owned())?; // 损坏输入即使无法正常累计也必须失败关闭。
            placement
                .validate_water_surfaces()
                .map_err(|detail| format!("摆放实例{}的整网格水体无效：{detail}", placement.id))?; // 每项身份与GPU参数继续复用单实例唯一纯校验。
        }
        Ok(()) // 全部记录格式合法且累计绑定数未超过固定全图预算。
    }
}

#[cfg(test)] // JSON兼容、枚举拼写和非法参数均由无IO单元测试锁定。
mod tests {
    use super::*; // 测试直接覆盖本模块全部公开格式和纯校验入口。

    fn valid_surface(mesh_index: u32) -> ScenePlacementWaterSurface {
        ScenePlacementWaterSurface {
            mesh_index, // 调用方指定仅用于作者诊断的GLB网格索引。
            node_path: vec!["场景根".to_owned(), format!("水面对象_{mesh_index}")], // 完整对象路径承担最终唯一匹配。
            mesh_name: format!("水面网格_{mesh_index}"), // 非空名称供错误诊断使用。
            kind: ScenePlacementWaterKind::Horizontal,     // 普通复杂湖面使用水平类别。
            flow_speed: 1.0,                             // 一倍流速处于正式合法范围内。
            height_offset: 0.0,                          // 默认完整保留Blender作者高度。
        }
    }

    #[test] // 旧文件没有水体字段时必须继续加载，并在再次保存时保持字段缺省。
    fn legacy_placement_json_defaults_to_no_water_surfaces() {
        let record: ScenePlacementRecord = serde_json::from_str(
            r#"{
                "id": "placement-legacy",
                "asset_path": "gltf/legacy.glb",
                "translation": [1.0, 2.0, 3.0],
                "yaw_degrees": 15.0,
                "scale": [1.0, 1.0, 1.0]
            }"#,
        )
        .expect("旧摆放JSON缺少water_surfaces时必须继续反序列化"); // Serde默认空数组是正式兼容边界。
        assert!(record.water_surfaces.is_empty()); // 旧普通模型不得被推断成任何水面。
        assert!(record.validate_water_surfaces().is_ok()); // 空覆盖数组是合法的普通摆放实例。
        let encoded = serde_json::to_value(&record).expect("兼容摆放记录必须能重新序列化"); // 保存路径使用相同Serde结构。
        assert!(encoded.get("water_surfaces").is_none()); // 空覆盖继续省略，避免无意义改写全部旧场景。
    }

    #[test] // 新格式必须稳定保存三类水体和snake_case枚举名称。
    fn placement_water_kind_uses_stable_snake_case_json() {
        let mut surface = valid_surface(0); // GLB首个网格索引零必须被视为合法身份。
        surface.kind = ScenePlacementWaterKind::Sloped; // 斜河面使用明确类别。
        let encoded = serde_json::to_value(&surface).expect("合法网格水体必须能序列化"); // 取得字段级JSON断言。
        assert_eq!(encoded["kind"], "sloped"); // 枚举不得泄漏Rust的PascalCase名称。
        let waterfall: ScenePlacementWaterKind =
            serde_json::from_str(r#""waterfall""#).expect("瀑布snake_case名称必须能读取"); // 正式作者JSON使用小写名称。
        assert_eq!(waterfall, ScenePlacementWaterKind::Waterfall); // 反序列化必须落到唯一瀑布类别。
        let default_kind: ScenePlacementWaterSurface = serde_json::from_str(
            r#"{"mesh_index":1,"node_path":["场景根","旧试验水面对象"],"mesh_name":"旧试验水面","flow_speed":1.0,"height_offset":0.0}"#,
        )
        .expect("早期整网格水体缺少kind时必须按水平水面读取"); // 字段级serde default保持试验作者数据兼容。
        assert_eq!(default_kind.kind, ScenePlacementWaterKind::Horizontal); // 缺省类别不得猜成斜坡或瀑布。
        assert!(default_kind.validate().is_ok()); // 非空稳定节点路径使缺省类别值仍可正式采用。

        let pathless_legacy: ScenePlacementWaterSurface = serde_json::from_str(
            r#"{"mesh_index":1,"mesh_name":"旧无路径水面","flow_speed":1.0,"height_offset":0.0}"#,
        )
        .expect("旧试验整网格水体缺少node_path时仍必须能读取"); // Serde兼容与正式采用校验保持两个明确阶段。
        assert!(pathless_legacy.node_path.is_empty()); // 缺省路径只作为待修复的内存值存在。
        assert!(pathless_legacy.validate().is_err()); // 正式匹配必须失败关闭，不能按mesh_index接管全部共享对象。
    }

    #[test] // 重复索引、空名称和非有限或越界数值必须在采用前失败。
    fn placement_water_validation_rejects_ambiguous_and_invalid_overrides() {
        let mut first = valid_surface(0); // 零号网格本身合法。
        first.flow_speed = MAX_SCENE_PLACEMENT_WATER_FLOW_SPEED; // 正式流速上限属于允许的闭区间。
        first.height_offset = -MAX_SCENE_PLACEMENT_WATER_HEIGHT_OFFSET_METERS; // 负向一万米边界同样合法。
        assert!(first.validate().is_ok()); // 两项精确边界不得因浮点比较被误拒绝。
        let mut shared_mesh = valid_surface(0); // 同一Mesh datablock允许被另一个Blender对象复用。
        shared_mesh.node_path[1] = "另一水面对象".to_owned(); // 不同节点路径形成不同最终实例身份。
        assert!(validate_scene_placement_water_surfaces(&[first.clone(), shared_mesh]).is_ok()); // mesh_index相同但对象路径不同不得被误判为重复。
        let mut duplicate = first.clone(); // 从合法首项复制完整路径与名称。
        duplicate.mesh_index = 9; // 即使诊断索引不同，最终路径与网格名重复仍是同一目标。
        assert!(validate_scene_placement_water_surfaces(&[first.clone(), duplicate]).is_err()); // 最终身份重复必须拒绝。

        let mut blank_name = valid_surface(1); // 构造第二个独立网格覆盖。
        blank_name.mesh_name = "   ".to_owned(); // 纯空白名称无法用于作者诊断。
        assert!(blank_name.validate().is_err()); // trim后为空必须拒绝。

        let mut blank_path = valid_surface(1); // 构造节点路径输入测试目标。
        blank_path.node_path[1] = " ".to_owned(); // 空白路径段无法稳定匹配Blender对象。
        assert!(blank_path.validate().is_err()); // 任一路径段为空都必须失败关闭。

        let mut generated_path = valid_surface(1); // 构造loader缺名节点占位值测试目标。
        generated_path.node_path[1] = "GltfNode17".to_owned(); // 十进制后缀属于Bevy loader生成规则而非Blender稳定名称。
        assert!(generated_path.validate().is_err()); // 手改JSON也不得绕过浏览器的不稳定名称拒绝门。
        assert!(is_generated_gltf_node_name("GltfNode0")); // 零索引占位名同样必须被识别。
        assert!(!is_generated_gltf_node_name("桥_GltfNode17")); // 用户显式名称只包含相同字样时保持合法。

        let mut deep_path = valid_surface(1); // 构造异常深Blender层级测试目标。
        deep_path.node_path =
            vec!["节点".to_owned(); MAX_SCENE_PLACEMENT_WATER_NODE_PATH_DEPTH + 1]; // 超出固定层级预算但每个路径段本身合法。
        assert!(deep_path.validate().is_err()); // 层级深度超过上限必须拒绝。

        let mut long_segment = valid_surface(1); // 构造异常长对象名称测试目标。
        long_segment.node_path[1] = "水".repeat(MAX_SCENE_PLACEMENT_WATER_NAME_CHARS + 1); // Unicode字符数刚好超过单段上限。
        assert!(long_segment.validate().is_err()); // 超长路径段不得进入运行时匹配键。

        let mut invalid_speed = valid_surface(2); // 构造流速边界测试目标。
        invalid_speed.flow_speed = f32::NAN; // NaN不能进入GPU Uniform。
        assert!(invalid_speed.validate().is_err()); // 非有限流速必须拒绝。
        invalid_speed.flow_speed = MAX_SCENE_PLACEMENT_WATER_FLOW_SPEED + 0.01; // 略超正式上限同样非法。
        assert!(invalid_speed.validate().is_err()); // 上限必须为闭区间二十。

        let mut invalid_offset = valid_surface(3); // 构造高度偏移边界测试目标。
        invalid_offset.height_offset = f32::INFINITY; // 无穷高度无法取景或构建包围盒。
        assert!(invalid_offset.validate().is_err()); // 非有限偏移必须拒绝。
        invalid_offset.height_offset = MAX_SCENE_PLACEMENT_WATER_HEIGHT_OFFSET_METERS + 0.01; // 超出正向一万米限制。
        assert!(invalid_offset.validate().is_err()); // 绝对偏移超过正式范围必须拒绝。

        let too_many = (0..=MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_PLACEMENT)
            .map(|index| valid_surface(index as u32))
            .collect::<Vec<_>>(); // 每项身份各自合法，只让总数刚好超过公开上限。
        assert!(validate_scene_placement_water_surfaces(&too_many).is_ok()); // 旧六十四项门不会截断合法作者绑定。
    }

    #[test] // 四个各含六十四项的placement合法，第257项必须由文档级公共预算拒绝。
    fn placement_document_accepts_global_water_surfaces_beyond_the_old_budget() {
        let placement_with_surfaces = |id: &str, count: usize| ScenePlacementRecord {
            mesh_lod: None,
            terrain_binding: None,
            editor: Default::default(),
            terrain_patch: None,
            id: id.to_owned(), // 每个测试placement使用稳定非空身份，错误可定位到首个越界记录。
            asset_path: "gltf/test/water.glb".to_owned(), // 纯水体预算校验不访问或解释资产路径。
            asset_target: Default::default(), // 水绑定文档只能引用完整Scene0层级。
            building_preset_id: None, // 水体预算夹具是普通摆放，不附带建筑预设身份。
            courtyard: None,   // 独立摆放记录不属于任何院落副本。
            translation: [0.0, 0.0, 0.0], // 变换不属于本纯校验职责，使用有限默认值。
            height_mode: crate::ScenePlacementHeightMode::FixedHeight, // 水预算测试保持兼容固定高度。
            terrain_offset: 0.0, // 固定高度没有有效离地偏移。
            yaw_degrees: 0.0,  // 零旋转保持测试构造直观。
            scale: [1.0, 1.0, 1.0], // 单位缩放避免引入无关非法输入。
            water_surfaces: (0..count)
                .map(|index| valid_surface(index as u32))
                .collect(), // 单实例内每个稳定身份唯一，只有全图累计量决定结果。
        }; // 完成可复用的固定大小placement构造器。
        let mut document = ScenePlacementsAsset {
            schema_version: crate::SCENE_SCHEMA_VERSION, // 文档级水体纯校验不绑定当前schema常量。
            scene_id: crate::SceneId::parse("MAP-water-surface-budget").expect("测试场景ID合法"), // 使用正式稳定场景身份。
            placements: (0..4)
                .map(|index| placement_with_surfaces(&format!("placement-{index}"), 64))
                .collect(), // 四乘六十四精确达到全图二百五十六项闭区间上限。
        }; // 完成合法边界文档。
        assert_eq!(
            document
                .placements
                .iter()
                .map(|placement| placement.water_surfaces.len())
                .sum::<usize>(),
            MAX_SCENE_PLACEMENT_WATER_SURFACES_PER_SCENE,
        ); // 测试构造必须准确命中公共上限而不是低一项。
        assert!(document.validate_water_surfaces().is_ok()); // 精确二百五十六项必须允许正式采用。

        document
            .placements
            .push(placement_with_surfaces("placement-over-budget", 1)); // 新根只有一个合法绑定，累计值成为二百五十七。
        document.validate_water_surfaces().expect("合法第257项水绑定应完整保留");
    }
}
