//! 已校验开发者预设目录的稳定查询、技术身份和场景输出派生。

use std::collections::BTreeMap;

use crate::{SceneVegetationKind, SceneVegetationLayer};

use super::{
    VegetationPresetCatalogAsset, VegetationPresetDescriptor, validation::validate_catalog,
};

/// 已通过完整校验、可按稳定 ID 和历史别名查询的目录快照。
#[derive(Clone, Debug, Default)]
pub struct ResolvedVegetationPresetCatalog {
    presets: Vec<VegetationPresetDescriptor>, // 展示顺序与开发者 JSON 一致。
    lookup: BTreeMap<String, usize>,          // 当前 ID 与全部历史别名共同映射到规范项。
}

impl ResolvedVegetationPresetCatalog {
    /// 原子校验并解析一个目录资产。
    pub fn build(asset: &VegetationPresetCatalogAsset) -> Result<Self, String> {
        validate_catalog(asset)?; // 任一项无效时拒绝整份新修订，调用方继续保留旧快照。
        let mut lookup = BTreeMap::new(); // 已校验无重复，插入不会覆盖另一项。
        for (index, preset) in asset.presets.iter().enumerate() {
            lookup.insert(preset.id.clone(), index); // 规范 ID 始终可直接查询。
            for alias in &preset.aliases {
                lookup.insert(alias.clone(), index); // 历史 ID 只解析到当前规范项。
            }
        }
        Ok(Self {
            presets: asset.presets.clone(), // 完整快照一次复制，稳定帧只读不重新反序列化。
            lookup,                         // 有序索引支持确定性热更和测试。
        })
    }

    /// 按开发者声明顺序返回全部规范预设。
    pub fn presets(&self) -> &[VegetationPresetDescriptor] {
        &self.presets // F12连续列表直接借用，不复制任意数量标签或运行时大描述。
    }

    /// 通过规范 ID 或历史别名解析预设。
    pub fn resolve(&self, id: &str) -> Option<&VegetationPresetDescriptor> {
        self.lookup.get(id).map(|index| &self.presets[*index]) // 返回规范项，保存时可迁移到当前 ID。
    }
}

impl VegetationPresetDescriptor {
    /// 返回预设的正式后端类别。
    pub const fn kind(&self) -> SceneVegetationKind {
        self.runtime.kind() // 类别由运行时枚举唯一决定，JSON 不维护重复字段。
    }

    /// 返回显式或确定性派生的正式运行时层 ID。
    pub fn resolved_runtime_layer_id(&self) -> u64 {
        self.runtime_layer_id
            .unwrap_or_else(|| stable_vegetation_identity("layer", &self.id)) // 开发者未手填数字也能稳定跨进程复现。
    }

    /// 返回 Mesh 显式或确定性派生的正式原型 ID。
    pub fn resolved_prototype_id(&self) -> Option<u64> {
        (self.kind() == SceneVegetationKind::Mesh).then(|| {
            self.prototype_id
                .unwrap_or_else(|| stable_vegetation_identity("prototype", &self.id)) // Blade 不制造无意义原型身份。
        })
    }

    /// 返回显式或确定性派生的作者散布种子。
    pub fn resolved_seed(&self) -> u64 {
        self.seed
            .unwrap_or_else(|| stable_vegetation_identity("seed", &self.id)) // 目录重排不会改变同一预设的分布。
    }

    /// 为一个场景构造尚未绘制的正式输出描述。
    pub fn create_scene_layer(&self, folder_relative: &str) -> SceneVegetationLayer {
        let folder = folder_relative.trim_end_matches('/'); // 统一避免生成双斜杠资产路径。
        SceneVegetationLayer {
            id: self.output_id.clone(), // 单图冲突由调用方在原子插入前处理。
            label: self.label.clone(),  // 面板与正式状态使用同一开发者名称。
            kind: self.kind(),          // Blade/Mesh 来自唯一运行时枚举。
            runtime_profile: Some(self.id.clone()), // 场景只持久化稳定规范 ID，不保存运行时批次或槽号。
            runtime_layer_id: Some(self.resolved_runtime_layer_id()), // 公共植被竞争与诊断共享同一稳定身份。
            prototype_id: self.resolved_prototype_id(), // 仅 Mesh 层登记通用原型身份。
            asset_path: self.author_asset_path().to_owned(), // 作者追踪使用近档或适配器声明路径。
            density_source_path: format!("{folder}/vegetation/{}.r8", self.output_id), // F12 写无 Mip R8 作者源。
            density_texture_path: format!("{folder}/vegetation/{}.ktx2", self.output_id), // 运行时读取完整 Mip KTX2。
            resolution: self.resolution, // 新输出使用预设默认画布尺寸。
            spacing_meters: self.spacing_meters, // Mesh 散布和 Blade 作者摘要共享预设格距。
            scale_range: self.scale_range, // 确定性随机缩放来自开发者预设。
            max_instances: u32::MAX,     // 新动态内容不以兼容字段截断作者密度。
            seed: self.resolved_seed(),  // 同一预设跨场景仍可由场景层显式改写。
        }
    }

    /// 返回作者资产追踪使用的近档资源路径。
    pub fn author_asset_path(&self) -> &str {
        &self.author_asset_path // 显式路径让所有数据预设使用同一场景字段。
    }
}

/// 使用固定 FNV-1a 命名空间哈希派生非零稳定技术身份。
pub fn stable_vegetation_identity(namespace: &str, id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64; // FNV-1a 六十四位固定偏移基，不依赖 Rust HashMap 随机种子。
    for byte in namespace.bytes().chain([0xff]).chain(id.bytes()) {
        hash ^= u64::from(byte); // 每个 UTF-8 字节先进入当前状态。
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // 固定素数乘法保持跨平台逐位一致。
    }
    hash.max(1) // 零保留给未声明身份，极端哈希结果确定性映射为一。
}
