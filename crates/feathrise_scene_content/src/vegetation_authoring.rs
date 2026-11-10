//! 植被作者图层、图层组和编译规则数据结构。
//!
//! 【功能设计】作者可以为同一个正式植被profile建立任意多个区域图层；保存与实时预览时，
//! 编辑器先按图层组解析覆盖、共享或共存关系，再把结果合并到预设对应的正式运行时层。
//! 作者层、组和正式输出均使用动态容器；仅实际像素、内存与设备能力决定可用规模。

use std::collections::{BTreeMap, BTreeSet}; // 校验稳定ID、组引用和正式输出引用时使用确定性容器。

use serde::{Deserialize, Serialize}; // 作者图层随vegetation.scene_vegetation.json热加载和原子保存。

use crate::SceneVegetationLayer; // 作者图层只能输出到同文件已登记的正式运行时层。

/// 植被作者图层集合；缺少此字段的旧文件由F12在内存中无损迁移。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneVegetationAuthoring {
    /// 组顺序也是跨组结果的合并顺序。
    pub groups: Vec<SceneVegetationAuthorGroup>,
    /// UI顺序保存的独立R8作者画布。
    pub layers: Vec<SceneVegetationAuthorLayer>,
}

/// 一个作者图层组的组内竞争与跨组合并规则。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneVegetationAuthorGroup {
    /// 场景内稳定组ID。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 同组作者层如何解析为各自正式输出贡献。
    pub mode: SceneVegetationAuthorGroupMode,
    /// 本组结果如何与前面组已经产生的同profile结果合并。
    pub output_merge: SceneVegetationOutputMergeMode,
    /// 加权共享模式的总容量，范围为零至一。
    #[serde(default = "default_group_capacity")]
    pub capacity: f32,
    /// 覆盖模式认领所有权所需的最小归一化密度。
    #[serde(default)]
    pub claim_threshold: f32,
    /// 同优先级覆盖选择使用的稳定世界哈希种子。
    #[serde(default = "default_group_seed")]
    pub selection_seed: u64,
    /// 同优先级覆盖选择格的世界米制边长。
    #[serde(default = "default_selection_cell_meters")]
    pub selection_cell_meters: f32,
    /// 关闭整组时保留作者数据但不参与最终编译。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// 一张只供作者绘制的R8密度图。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneVegetationAuthorLayer {
    /// 场景内稳定作者层ID；它不进入GPU运行时ABI。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 所属作者组稳定ID。
    pub group_id: String,
    /// 编译结果写入的正式`SceneVegetationLayer::id`。
    pub output_layer_id: String,
    /// 作者可改写的无Mip R8源；正式运行时不加载该路径。
    pub density_source_path: String,
    /// 作者画布宽高；编译器允许不同图层具有不同分辨率。
    pub resolution: [u32; 2],
    /// false保留画布但不参与最终编译，等价于图层面板隐藏。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// true只禁止F12笔刷和参数修改，不改变编译结果。
    #[serde(default)]
    pub locked: bool,
    /// 图层密度进入组规则前的归一化不透明度。
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,
    /// 数值越大优先级越高；共享与覆盖模式均使用此字段。
    #[serde(default)]
    pub priority: i32,
    /// 同优先级共享或覆盖选择权重，必须是有限正数。
    #[serde(default = "default_layer_weight")]
    pub weight: f32,
}

/// 同组作者图层之间的解析模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneVegetationAuthorGroupMode {
    /// 每层保留自身需求；同一正式输出的贡献在组内相加并封顶。
    #[default]
    Additive,
    /// 同组共享固定容量，高优先级先占，同级按权重分配且不放大原需求。
    WeightedPartition,
    /// 只保留达到阈值的最高优先级层，同级使用稳定世界选择格分配。
    PriorityOverride,
}

impl SceneVegetationAuthorGroupMode {
    /// 返回面向F12图层面板的简短中文名称。
    pub fn label(self) -> &'static str {
        match self {
            Self::Additive => "共存",
            Self::WeightedPartition => "共享容量",
            Self::PriorityOverride => "上层覆盖",
        }
    }

    /// 按固定顺序循环，保证静态按钮无需弹出复杂菜单也可完整设置。
    pub fn next(self) -> Self {
        match self {
            Self::Additive => Self::WeightedPartition,
            Self::WeightedPartition => Self::PriorityOverride,
            Self::PriorityOverride => Self::Additive,
        }
    }
}

/// 不同作者组写向同一正式profile时的合并规则。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneVegetationOutputMergeMode {
    /// 每像素保留较大密度，适合南北区域或多块相离画布。
    #[default]
    Maximum,
    /// 每像素相加并封顶，适合多个稀疏装饰层累积密度。
    AddClamped,
    /// 本组非零像素替换前面组，适合局部人工修补。
    ReplaceNonZero,
}

impl SceneVegetationOutputMergeMode {
    /// 返回面向F12图层面板的简短中文名称。
    pub fn label(self) -> &'static str {
        match self {
            Self::Maximum => "取最大",
            Self::AddClamped => "相加封顶",
            Self::ReplaceNonZero => "非零替换",
        }
    }

    /// 按固定顺序循环全部受支持的确定性合并模式。
    pub fn next(self) -> Self {
        match self {
            Self::Maximum => Self::AddClamped,
            Self::AddClamped => Self::ReplaceNonZero,
            Self::ReplaceNonZero => Self::Maximum,
        }
    }
}

fn default_group_capacity() -> f32 {
    1.0 // 默认允许同组填满完整候选容量。
}

fn default_group_seed() -> u64 {
    0x4D41_505F_4752_4F55 // “SCENE_GROU”的稳定非零种子，具体新组会再混入序号。
}

fn default_selection_cell_meters() -> f32 {
    8.0 // 足够避免逐纹素椒盐分布，同时保持区域边界精度。
}

fn default_enabled() -> bool {
    true // 旧文件缺省时不能静默隐藏作者内容。
}

fn default_layer_opacity() -> f32 {
    1.0 // 默认完整保留R8画布密度。
}

fn default_layer_weight() -> f32 {
    1.0 // 同级默认一比一共享或稳定选择。
}

fn safe_asset_path(path: &str) -> bool {
    !path.is_empty() // 空路径没有可保存作者源。
        && !path.starts_with('/') // 绝对路径不能越过assets根。
        && !path.contains("..") // 父目录跳转一律拒绝。
        && !path.contains('\\') // 作者文件统一使用AssetServer正斜杠口径。
}

impl SceneVegetationAuthoring {
    /// 校验作者图层稳定引用、可寻址尺寸与全部数值；失败草稿不得进入编译或保存任务。
    pub fn validate(&self, outputs: &[SceneVegetationLayer]) -> Result<(), String> {
        if self.groups.is_empty() && !self.layers.is_empty() {
            return Err("植被作者图层存在，但没有任何图层组".to_owned()); // 每层必须显式声明交互关系。
        }
        let mut output_ids = BTreeMap::new(); // 同时提供正式ID存在性和目标分辨率。
        for output in outputs {
            if output.id.is_empty()
                || output_ids
                    .insert(output.id.as_str(), output.resolution)
                    .is_some()
            {
                return Err("植被正式输出包含空白或重复稳定ID".to_owned());
            }
            if output.resolution[0] < 2 || output.resolution[1] < 2 {
                return Err(format!("植被正式输出{}分辨率必须至少为2×2", output.id));
            }
            (output.resolution[0] as usize)
                .checked_mul(output.resolution[1] as usize)
                .ok_or_else(|| format!("植被正式输出{}分辨率乘法溢出", output.id))?;
        } // 每张画布只校验当前平台是否可寻址，不累计成人为总 MiB 配额。
        let mut group_ids = BTreeSet::new(); // 空白或重复组ID会让引用产生歧义。
        for group in &self.groups {
            if group.id.is_empty() || !group_ids.insert(group.id.as_str()) {
                return Err("植被作者组包含空白或重复稳定ID".to_owned());
            }
            if !group.capacity.is_finite()
                || !(0.0..=1.0).contains(&group.capacity)
                || !group.claim_threshold.is_finite()
                || !(0.0..=1.0).contains(&group.claim_threshold)
                || !group.selection_cell_meters.is_finite()
                || group.selection_cell_meters <= 0.0
            {
                return Err(format!("植被作者组{}包含无效容量、阈值或选择格", group.id));
            }
        }
        let mut layer_ids = BTreeSet::new(); // 作者层ID同时作为内存画布和撤销身份。
        let mut source_paths = BTreeSet::new(); // 两层不能写向同一作者源而互相覆盖。
        for layer in &self.layers {
            if layer.id.is_empty() || !layer_ids.insert(layer.id.as_str()) {
                return Err("植被作者层包含空白或重复稳定ID".to_owned());
            }
            if !group_ids.contains(layer.group_id.as_str()) {
                return Err(format!(
                    "植被作者层{}引用不存在的组{}",
                    layer.id, layer.group_id
                ));
            }
            if !output_ids.contains_key(layer.output_layer_id.as_str()) {
                return Err(format!(
                    "植被作者层{}引用不存在的正式输出{}",
                    layer.id, layer.output_layer_id
                ));
            }
            if !safe_asset_path(&layer.density_source_path)
                || !source_paths.insert(layer.density_source_path.as_str())
            {
                return Err(format!("植被作者层{}包含无效或重复作者源路径", layer.id));
            }
            if layer.resolution[0] < 2 || layer.resolution[1] < 2 {
                return Err(format!(
                    "植被作者层{}分辨率{}×{}必须至少为2×2",
                    layer.id, layer.resolution[0], layer.resolution[1]
                ));
            }
            (layer.resolution[0] as usize)
                .checked_mul(layer.resolution[1] as usize)
                .ok_or_else(|| format!("植被作者层{}分辨率乘法溢出", layer.id))?;
            if !layer.opacity.is_finite()
                || !(0.0..=1.0).contains(&layer.opacity)
                || !layer.weight.is_finite()
                || layer.weight <= 0.0
            {
                return Err(format!("植被作者层{}包含无效透明度或权重", layer.id));
            }
        }
        Ok(()) // 纹素长度在读取或编译边界使用声明分辨率再次严格校验。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 纯数据测试不依赖Bevy App或文件系统。
    use crate::SceneVegetationKind; // 构造一个最小正式输出供引用校验。

    fn output() -> SceneVegetationLayer {
        SceneVegetationLayer {
            id: "grass".to_owned(),                                  // 唯一正式输出ID。
            label: "普通草".to_owned(),                              // 仅用于完整结构。
            kind: SceneVegetationKind::Blade,                          // 数据校验不区分后端。
            runtime_profile: Some("hn01_ordinary_grass".to_owned()), // 保留正式profile示例。
            runtime_layer_id: Some(1), // 正式运行时身份不由作者层复制。
            prototype_id: None,        // Blade无需Mesh原型。
            asset_path: "grass.glb".to_owned(), // 本测试不校验正式源路径。
            density_source_path: "scenes/test/vegetation/grass.r8".to_owned(), // 编译输出作者源。
            density_texture_path: "scenes/test/vegetation/grass.ktx2".to_owned(), // 运行时派生纹理。
            resolution: [4, 4],        // 小型受检分辨率。
            spacing_meters: 1.0,       // 运行时预算不属于作者图层校验。
            scale_range: [1.0, 1.0],   // 保持合法正式层。
            max_instances: 16,         // 保持合法正式层。
            seed: 1,                   // 保持稳定正式层。
        }
    }

    fn authoring() -> SceneVegetationAuthoring {
        SceneVegetationAuthoring {
            groups: vec![SceneVegetationAuthorGroup {
                id: "ground".to_owned(),                               // 作者层引用的稳定组。
                label: "地被".to_owned(),                              // 图层面板显示名。
                mode: SceneVegetationAuthorGroupMode::WeightedPartition, // 覆盖非默认枚举反序列化。
                output_merge: SceneVegetationOutputMergeMode::Maximum,   // 南北组默认取最大合并。
                capacity: 1.0,                                         // 完整容量。
                claim_threshold: 0.0,                                  // 任意非零密度均可认领。
                selection_seed: 7,                                     // 稳定同级选择种子。
                selection_cell_meters: 8.0,                            // 合法世界选择格。
                enabled: true,                                         // 参与编译。
            }],
            layers: vec![SceneVegetationAuthorLayer {
                id: "north-grass".to_owned(),        // 作者层不复用正式层ID。
                label: "北部草".to_owned(),          // 面板显示名。
                group_id: "ground".to_owned(),       // 引用上方组。
                output_layer_id: "grass".to_owned(), // 输出到唯一正式层。
                density_source_path: "scenes/test/vegetation/author/north-grass.r8".to_owned(), // 独立R8源。
                resolution: [4, 4], // 合法作者分辨率。
                enabled: true,      // 参与编译。
                locked: false,      // 允许笔刷。
                opacity: 1.0,       // 不缩放密度。
                priority: 0,        // 默认优先级。
                weight: 1.0,        // 默认一比一权重。
            }],
        }
    }

    #[test]
    fn authoring_validates_stable_group_and_output_references() {
        let outputs = vec![output()]; // 正式输出集合固定存在。
        let mut value = authoring(); // 从完整合法作者图开始。
        assert!(value.validate(&outputs).is_ok()); // 合法图层可进入编译器。
        value.layers[0].group_id = "missing".to_owned(); // 制造悬空组引用。
        assert!(value.validate(&outputs).is_err()); // 悬空引用必须原子拒绝。
        value.layers[0].group_id = "ground".to_owned(); // 恢复组引用。
        value.layers[0].output_layer_id = "missing".to_owned(); // 制造未知正式profile输出。
        assert!(value.validate(&outputs).is_err()); // 作者层不得创建低配运行时替身。
    }

    #[test]
    fn authoring_defaults_keep_old_json_visible_and_one_to_one() {
        let json = r#"{
            "groups":[{"id":"base","label":"基础","mode":"additive","output_merge":"maximum"}],
            "layers":[{"id":"base-grass","label":"草","group_id":"base","output_layer_id":"grass","density_source_path":"scenes/test/base.r8","resolution":[4,4]}]
        }"#; // 省略所有兼容默认字段模拟早期作者文件。
        let value: SceneVegetationAuthoring = serde_json::from_str(json).expect("兼容字段必须可缺省"); // Serde默认不得隐藏内容。
        assert!(value.groups[0].enabled); // 旧组默认参与编译。
        assert_eq!(value.groups[0].capacity, 1.0); // 旧共享组默认完整容量。
        assert!(value.layers[0].enabled); // 旧层默认可见并参与编译。
        assert!(!value.layers[0].locked); // 旧层默认可编辑。
        assert_eq!(value.layers[0].weight, 1.0); // 旧层默认一比一。
    }

    #[test]
    fn authoring_does_not_apply_an_aggregate_r8_memory_quota() {
        let outputs = vec![output()];
        let mut value = authoring();
        value.layers = (0..9)
            .map(|index| {
                let mut layer = value.layers[0].clone();
                layer.id = format!("huge-{index}");
                layer.density_source_path = format!("scenes/test/author/huge-{index}.r8");
                layer.resolution = [4096, 4096]; // 九张共144MiB，用于证明内容层不猜测用户内存预算。
                layer
            })
            .collect();
        assert!(value.validate(&outputs).is_ok()); // 唯一身份与可寻址尺寸合法时完整保留全部作者层。
    }
}
