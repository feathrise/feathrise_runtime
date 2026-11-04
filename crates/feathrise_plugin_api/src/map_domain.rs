//! 地图外部作者的C ABI纯数据合同；大正文以工程内不可变文件传输，不跨动态库传Rust对象。
use serde::{Deserialize, Serialize};
mod visuals;
pub use visuals::*;

pub const MAX_DOMAIN_BLOB_BYTES: u64 = 64 * 1024 * 1024;
pub const DOMAIN_CACHE: &str = ".feathrise/native-domain";
/// 纯准备回执不得修改活动文档；旧地图插件必须显式拒绝，不能沿用有副作用的GroundPatch。
pub const GROUND_PATCH_PREPARATION_PROTOCOL: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MapDomainIdentity {
    pub domain: String,
    pub map_id: String,
    pub generation: u64,
    pub revision: u64,
    pub terrain_revision: u64,
    pub environment_revision: u64,
    pub ground_content: Option<[u64; 7]>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DomainBlob {
    /// 相对工程根，只允许DOMAIN_CACHE内、以SHA命名的不可变文件。
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DomainDependency {
    /// 相对当前资产根；保存前与最终替换前都重新比较原始字节版本。
    pub path: String,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDomainDocument {
    pub body: DomainBlob,
    pub scene: DomainBlob,
    pub source_hash: String,
    pub hash: String,
    pub catalog_hash: String,
    pub asset_path: String,
    /// 兼容地图目录的不透明附加文档槽；目前只支持已存在的characters_path。
    pub catalog_slot: String,
    pub dependencies: Vec<DomainDependency>,
    pub dirty: bool,
    pub registration_required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDomainRequest {
    pub identity: MapDomainIdentity,
    pub command: MapDomainCommand,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MapDomainCommand {
    /// 量化后的显示密度；只改变辅助显示，不修改地图正文和历史。
    Visuals {
        spacing: f32,
        visible_cells: u64,
    },
    Open,
    /// 重入、撤销、重做都采用完整同代版本；插件后台再次核对引用后才回执。
    Adopt {
        document: MapDomainDocument,
    },
    Viewport {
        input: MapDomainViewport,
    },
    Scene {
        input: MapDomainScene,
    },
    /// 仅用于当前写侧候选的真实投射收口；请求身份必须仍与活动文档完整相等。
    GroundPatch {
        document: MapDomainDocument,
        patches: DomainBlob,
    },
    /// 地形跟随只准备不可变候选，不修改活动正文、身份、手势或引用租约；由宿主完整采用后Adopt。
    PrepareGroundPatch {
        document: MapDomainDocument,
        patches: DomainBlob,
    },
    Saved {
        hash: String,
    },
    Close,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DomainPointerPhase {
    Begin,
    Update,
    End,
    Cancel,
    Click,
    Gap,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDomainViewport {
    pub phase: DomainPointerPhase,
    /// 已由当前显示进程确认的真实承托XYZ；空样本不能用旧悬停位置补写。
    pub positions: Vec<[f32; 3]>,
    pub breaks: Vec<u32>,
    pub selected: Option<String>,
    pub layer: Option<String>,
    pub point: Option<String>,
    pub selection_ticket: u64,
    pub shift: bool,
    /// 单次明确点击的通用地形关联；领域拥有保存字段，公共层不解释作者正文。
    pub terrain_binding: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDomainSnapshot {
    pub domain: String,
    /// 缺省0代表旧插件；宿主要求明确支持纯准备，避免静默退回后台活动写入。
    #[serde(default)]
    pub ground_patch_preparation: u32,
    /// 初始能力公告没有地图；任何真实文档回执必须携带完整身份。
    pub identity: Option<MapDomainIdentity>,
    pub document: Option<MapDomainDocument>,
    pub selected: Option<String>,
    pub layer: Option<String>,
    pub tool: String,
    pub continuous: bool,
    pub selection_ticket: u64,
    /// 同一完整写操作只递增一次，宿主据此压入一条原地图历史。
    pub edit_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<MapDomainPointer>,
    /// 本次完整写侧尚待真实地形投射的候选；宿主后台全部成功后才能原子采用正文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_requests: Option<DomainBlob>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visuals: Option<MapDomainVisuals>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDomainPointer {
    pub radius: f32,
    pub spacing: f32,
    /// 插件冻结自己的预设、随机和子对象选择；宿主只比较此键，不解释其内容。
    pub target: String,
    pub point: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DomainSceneKind {
    Layer,
    Object,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DomainSceneAction {
    Rename,
    Copy,
    Delete,
    Visibility,
    Lock,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDomainScene {
    pub kind: DomainSceneKind,
    pub id: String,
    pub action: DomainSceneAction,
    pub name: String,
}

/// 此文件可很大，只在宿主后台验证并建索引；主帧只持有Arc与O(1)窗口查询。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainScene {
    pub layers: Vec<DomainSceneRow>,
    pub objects: Vec<DomainSceneRow>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSceneRow {
    pub id: String,
    pub name: String,
    pub label: String,
    pub layer: Option<String>,
    pub hidden: bool,
    pub locked: bool,
    pub bounds: Option<[[f32; 3]; 2]>,
    pub binding: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainGroundPatch {
    pub id: String,
    pub position: [f32; 3],
    pub yaw_degrees: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainGroundProbe {
    pub id: String,
    pub position: [f32; 3],
    pub yaw_degrees: f32,
}

fn short(value: &str) -> bool {
    value.len() <= 1024 && !value.contains('\0')
}
pub fn relative_path(value: &str) -> bool {
    !value.is_empty()
        && short(value)
        && !value.contains(['\\', ':'])
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
impl DomainBlob {
    pub fn validate(&self) -> Result<(), String> {
        if !relative_path(&self.path)
            || !self.path.starts_with(&format!("{DOMAIN_CACHE}/"))
            || !hash(&self.sha256)
            || self.bytes > MAX_DOMAIN_BLOB_BYTES
            || !self.path.ends_with(&format!("/{}.json", self.sha256))
        {
            return Err("地图领域不可变文件的路径、摘要或大小无效".into());
        }
        Ok(())
    }
}
impl MapDomainIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if self.domain.is_empty()
            || !short(&self.domain)
            || self.map_id.is_empty()
            || !short(&self.map_id)
        {
            return Err("地图领域或地图身份无效".into());
        }
        Ok(())
    }
}
impl MapDomainDocument {
    pub fn validate(&self) -> Result<(), String> {
        self.body.validate()?;
        self.scene.validate()?;
        if !relative_path(&self.asset_path)
            || self.catalog_slot != "characters_path"
            || !hash(&self.hash)
            || !(hash(&self.source_hash) || self.source_hash == "missing")
            || !hash(&self.catalog_hash)
            || self.dependencies.len() > 32
            || self
                .dependencies
                .iter()
                .any(|d| !relative_path(&d.path) || !(hash(&d.hash) || d.hash == "missing"))
        {
            return Err("地图领域正文、依赖或目录槽无效".into());
        }
        Ok(())
    }
}
impl MapDomainRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.identity.validate()?;
        match &self.command {
            MapDomainCommand::Visuals {
                spacing,
                visible_cells,
            } if !spacing.is_finite() || *spacing <= 0. || *visible_cells == 0 => {
                return Err("作者显示密度无效".into());
            }
            MapDomainCommand::Scene { input }
                if !short(&input.id) || input.id.is_empty() || !short(&input.name) =>
            {
                return Err("地图场景操作身份或名称无效".into());
            }
            MapDomainCommand::Adopt { document } => document.validate()?,
            MapDomainCommand::GroundPatch { document, patches }
            | MapDomainCommand::PrepareGroundPatch { document, patches } => {
                document.validate()?;
                patches.validate()?;
            }
            MapDomainCommand::Saved { hash: saved } if !hash(saved) => {
                return Err("地图领域保存回执摘要无效".into());
            }
            MapDomainCommand::Viewport { input } => {
                if input.positions.len() > 4096
                    || input.breaks.len() > 4096
                    || input.positions.iter().flatten().any(|n| !n.is_finite())
                    || input.breaks.windows(2).any(|w| w[0] >= w[1])
                    || input
                        .breaks
                        .iter()
                        .any(|i| *i == 0 || *i as usize >= input.positions.len())
                    || input
                        .selected
                        .iter()
                        .chain(input.layer.iter())
                        .chain(input.point.iter())
                        .any(|v| !short(v))
                {
                    return Err("地图领域视口样本或稳定选择身份无效".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
}
impl MapDomainSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(visuals) = &self.visuals {
            visuals.validate()?;
        }
        if let Some(requests) = &self.ground_requests {
            requests.validate()?;
        }
        if self.domain.is_empty()
            || !short(&self.domain)
            || !short(&self.tool)
            || self
                .selected
                .iter()
                .chain(self.layer.iter())
                .any(|v| !short(v))
        {
            return Err("地图领域回执工具或选择身份无效".into());
        }
        if let Some(identity) = &self.identity {
            identity.validate()?;
            if identity.domain != self.domain {
                return Err("地图领域回执身份不一致".into());
            }
        }
        if let Some(document) = &self.document {
            if self.identity.is_none() {
                return Err("地图领域正文缺少采用身份".into());
            }
            document.validate()?;
        }
        if let Some(pointer) = &self.pointer {
            if !pointer.radius.is_finite()
                || pointer.radius < 0.
                || !pointer.spacing.is_finite()
                || pointer.spacing <= 0.
                || !short(&pointer.target)
                || pointer.point.as_deref().is_some_and(|s| !short(s))
            {
                return Err("地图指针工具参数无效".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blob_contract_rejects_traversal_and_unbounded_documents() {
        let hash = "0".repeat(64);
        let mut blob = DomainBlob {
            path: format!("{DOMAIN_CACHE}/fixture/{hash}.json"),
            sha256: hash,
            bytes: 8,
        };
        blob.validate().unwrap();
        blob.path.insert_str(DOMAIN_CACHE.len() + 1, "../");
        assert!(blob.validate().is_err());
        blob.path = format!("{DOMAIN_CACHE}/fixture/{}.json", blob.sha256);
        blob.bytes = MAX_DOMAIN_BLOB_BYTES + 1;
        assert!(blob.validate().is_err());
    }
}

/// 后台不可变文件传输的可选实现；纯表单插件无需启用。
#[cfg(feature = "blob-io")]
pub mod blob_io;
