//! 动态场景正式Mesh profile请求、诊断与唯一加载事务资源。

use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

use bevy::prelude::*;
use feathrise_environment::vegetation::{VegetationMeshPrototypeRegistry, VegetationPrototypeId};
use feathrise_scene_content::{VegetationPresetRegistry, VegetationPresetRuntime};

use super::generic::{BuiltGenericMeshProfile, GenericMeshProfileLoad};

/// 动态场景正式Mesh原型按需加载阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicMeshProfilePhase {
    #[default]
    Idle,
    Loading,
    Ready,
    Failed,
}

/// F12按钮和动态植被运行时共同写入的幂等请求集合。
#[derive(Resource, Default)]
pub struct DynamicMeshProfileRequests {
    requested: BTreeSet<String>,
    revision: u64,
}

impl DynamicMeshProfileRequests {
    /// 请求一个非空稳定 profile；目录与后端类别由加载系统同修订校验。
    pub fn request(&mut self, profile: &str) -> Result<bool, String> {
        if profile.trim().is_empty() {
            return Err("Mesh植被profile不能为空".to_owned());
        }
        let inserted = self.requested.insert(profile.to_owned());
        // 集合本身幂等，但每次显式请求都推进修订；加载失败后作者再次点选同一画布即可重试。
        self.revision = self.revision.wrapping_add(1).max(1);
        Ok(inserted)
    }

    pub fn contains(&self, profile: &str) -> bool {
        self.requested.contains(profile)
    }

    pub fn requested_count(&self) -> usize {
        self.requested.len()
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &str> {
        self.requested.iter().map(String::as_str) // 有序集合保证不同机器请求顺序一致。
    }

    pub(super) fn clear(&mut self) {
        self.requested.clear();
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

/// 编辑器和F3可读取的正式原型加载证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicMeshProfileStatus {
    pub phase: DynamicMeshProfilePhase,
    pub requested_profiles: usize,
    pub ready_profiles: usize,
    pub detail: String,
}

/// 各场景/独立网格只持自己的请求回执和共享租约，不拥有兄弟仍在使用的原型。
#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct DynamicMeshProfileState {
    /// 当前请求中已经能从公共注册表取得的profile；可能由本事务拥有，也可能由同场景既有所有者提供。
    pub(super) available_profiles: BTreeSet<String>,
    /// 实例退役仅drop本租约；真正资源回收由场景共享缓存按最后引用执行。
    pub(super) leases: BTreeMap<String, DynamicMeshProfileLease>,
    /// 通用预设当前已登记原型对应的目录修订。
    pub(super) generic_catalog_revisions: BTreeMap<String, u64>,
    pub(super) failed_request_revision: Option<u64>,
}

/// 预设热更必须使用新键；同一规范ID的不同目录修订不能互借ready信用。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DynamicMeshProfileKey {
    pub catalog_revision: u64,
    pub preset_id: String,
}

pub(super) struct DynamicMeshProfileLease {
    pub key: DynamicMeshProfileKey,
    pub _token: Arc<()>, // 只持有引用信用；业务读取ready不需要访问token内容。
}

pub(super) struct DynamicMeshProfileCacheEntry {
    pub token: Arc<()>,
    pub prototype_id: VegetationPrototypeId,
    /// 完整原型与派生资产句柄共同保活，旧渲染根的强句柄仍可继续使用其旧版本。
    pub loaded: Option<(u64, BuiltGenericMeshProfile)>,
    pub borrowed_revision: Option<u64>,
    pub failure: Option<String>,
}

/// 全场景（父图、嵌套子场景、独立网格）共享一个加载槽与每份预设的资产所有权。
/// 与注册表同属scene resident资源，驻留暂停/恢复不能丢掉引用信用。
#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct DynamicMeshProfileCache {
    pub(super) entries: BTreeMap<DynamicMeshProfileKey, DynamicMeshProfileCacheEntry>,
    pub(super) current_generic: Option<GenericMeshProfileLoad>,
    pub(super) registered: BTreeMap<VegetationPrototypeId, (DynamicMeshProfileKey, u64)>,
    pub(super) last_drive_frame: Option<u32>,
    pub(super) settle_frames: u8,
    pub(super) request_cursor: Option<DynamicMeshProfileKey>,
    pub(super) garbage_cursor: Option<DynamicMeshProfileKey>,
}

impl DynamicMeshProfileCache {
    /// 回归读取真实single-flight槽，不能预先伪造实例ready来掩盖调度漏接。
    #[cfg(test)]
    pub(in crate::scenes::dynamic_scene) fn loading_profile(&self) -> Option<&str> {
        self.current_generic.as_ref().map(|load| load.preset_id.as_str())
    }
}

impl DynamicMeshProfileState {
    /// 仅传播本scope已请求、当前目录下该具体预设的错误，不借其他预览或旧目录的失败回执。
    pub(in crate::scenes::dynamic_scene) fn failure_for<'a>(
        &self,
        profile: &str,
        presets: &VegetationPresetRegistry,
        cache: &'a DynamicMeshProfileCache,
    ) -> Option<&'a str> {
        let preset = presets.resolve(profile)?;
        let lease = self.leases.get(&preset.id)?;
        if lease.key.catalog_revision != presets.revision || lease.key.preset_id != preset.id {
            return None;
        }
        cache.entries.get(&lease.key)?.failure.as_deref()
    }

    /// 当前 profile 是否已经为本份目录修订登记完整原型。
    pub(in crate::scenes::dynamic_scene) fn ready_for(
        &self,
        profile: &str,
        presets: &VegetationPresetRegistry,
        prototypes: &VegetationMeshPrototypeRegistry,
    ) -> bool {
        let Some(preset) = presets.resolve(profile) else {
            return false;
        };
        let Some(prototype_id) = preset.resolved_prototype_id() else {
            return false;
        };
        if !self.available_profiles.contains(&preset.id)
            || !prototypes.contains(VegetationPrototypeId(prototype_id))
        {
            return false;
        }
        matches!(&preset.runtime, VegetationPresetRuntime::Mesh { .. })
            && self.generic_catalog_revisions.get(&preset.id) == Some(&presets.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_formal_profile_request_keeps_one_entry_but_opens_retry_revision() {
        let mut requests = DynamicMeshProfileRequests::default();
        assert!(requests.request("hn01_tree").unwrap());
        let first_revision = requests.revision();
        assert!(!requests.request("hn01_tree").unwrap());
        assert_eq!(requests.requested_count(), 1);
        assert!(requests.revision() > first_revision);
        assert!(requests.request("developer_mesh").unwrap()); // 内容身份不再由 Rust 固定枚举裁决。
        assert!(requests.request("").is_err());
    }
}
