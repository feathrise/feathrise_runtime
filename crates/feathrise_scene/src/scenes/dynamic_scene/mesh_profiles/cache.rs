//! 多网格共享正式Mesh原型；加载单飞、版本租约及最后引用回收保持同一生命周期。

use std::{ops::Bound::{Excluded, Unbounded}, sync::Arc};

use bevy::prelude::*;
use feathrise_environment::{
    mesh_wind::{TreeWindMaterial, TreeWindNoiseTexture},
    vegetation::{VegetationMeshBillboardMaterial, VegetationMeshPrototypeRegistry, VegetationPrototypeId},
};
use feathrise_scene_content::VegetationPresetRegistry;

use super::{generic::{build_generic_mesh_profile, generic_mesh_profile_failed, generic_mesh_profile_failure_detail,
    generic_mesh_profile_finished, request_generic_mesh_profile},
    resources::{DynamicMeshProfileCache, DynamicMeshProfileCacheEntry,
        DynamicMeshProfileKey, DynamicMeshProfileLease}};

impl DynamicMeshProfileCacheEntry {
    pub(super) fn ready(&self, registry: &VegetationMeshPrototypeRegistry) -> bool {
        let Some(current) = registry.revision(self.prototype_id) else { return false; };
        // 同ID外部正式替换后仍可能持有旧版保活资源；只要本次借用版本精确匹配就可使用。
        self.loaded.as_ref().is_some_and(|(revision, _)| *revision == current)
            || self.borrowed_revision == Some(current)
    }
}

impl DynamicMeshProfileCache {
    pub(super) fn lease(&mut self, key: DynamicMeshProfileKey,
        prototype_id: VegetationPrototypeId) -> DynamicMeshProfileLease {
        let entry = self.entries.entry(key.clone()).or_insert_with(|| DynamicMeshProfileCacheEntry {
            token: Arc::new(()), prototype_id, loaded: None, borrowed_revision: None, failure: None,
        });
        DynamicMeshProfileLease { key, _token: entry.token.clone() }
    }

    /// 每次只检查一个来源；请求集合数量不会变成稳定帧的全目录遍历。
    fn next_request(&mut self) -> Option<DynamicMeshProfileKey> {
        let next = self.request_cursor.as_ref().and_then(|cursor| {
            self.entries.range((Excluded(cursor), Unbounded)).next().map(|(key, _)| key.clone())
        }).or_else(|| self.entries.keys().next().cloned());
        self.request_cursor = next.clone();
        next
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn advance(&mut self, frame: u32, server: &AssetServer,
        presets: &VegetationPresetRegistry, registry: &mut VegetationMeshPrototypeRegistry,
        meshes: &mut Assets<Mesh>, standards: &mut Assets<StandardMaterial>,
        winds: &mut Assets<TreeWindMaterial>, billboards: &mut Assets<VegetationMeshBillboardMaterial>,
        noise: &TreeWindNoiseTexture) {
        if self.last_drive_frame == Some(frame) { return; }
        self.last_drive_frame = Some(frame); // 父图和多个实例在同帧调用也只推进一次共享装配。
        if let Some(load) = self.current_generic.as_ref() {
            let failed = generic_mesh_profile_failed(server, load);
            if !generic_mesh_profile_finished(server, load) { return; }
            let load = self.current_generic.take().expect("当前加载仍占唯一槽");
            let key = DynamicMeshProfileKey { catalog_revision: load.catalog_revision,
                preset_id: load.preset_id.clone() };
            let Some(entry) = self.entries.get_mut(&key) else { return; };
            if key.catalog_revision != presets.revision || Arc::strong_count(&entry.token) == 1 {
                return; // 换代/退役仍等真实加载结束，绝不采用旧版或提前腾出并发槽。
            }
            let built = if failed {
                let detail = generic_mesh_profile_failure_detail(server, &load)
                    .unwrap_or_else(|| "资产服务器未提供具体错误".to_owned());
                Err(format!("通用Mesh预设{}资产加载失败：{detail}", key.preset_id))
            } else {
                build_generic_mesh_profile(&load, meshes, standards, winds, billboards, noise)
                    .map_err(|error| format!("通用Mesh预设{}装配失败：{error}", key.preset_id))
            };
            match built {
                Ok(built) => {
                    registry.register(built.prototype.clone());
                    let revision = registry.revision(entry.prototype_id).expect("完整原型已登记");
                    entry.loaded = Some((revision, built));
                    entry.borrowed_revision = None;
                    entry.failure = None;
                    self.registered.insert(entry.prototype_id, (key, revision));
                    self.settle_frames = 1; // 新材质先进入Assets，再允许下一个大GLTF进入加载槽。
                }
                Err(error) => { error!("{error}"); entry.failure = Some(error); }
            }
            return;
        }
        if self.settle_frames > 0 { self.settle_frames -= 1; return; }
        let Some(key) = self.next_request() else { return; };
        let entry = self.entries.get_mut(&key).expect("游标只返回已登记键");
        if key.catalog_revision != presets.revision || Arc::strong_count(&entry.token) == 1
            || entry.ready(registry) || entry.failure.is_some() { return; }
        let current_revision = registry.revision(entry.prototype_id);
        let owned_revision = self.registered.get(&entry.prototype_id).map(|(_, revision)| *revision);
        if current_revision.is_some() && current_revision != owned_revision {
            entry.borrowed_revision = current_revision;
            return; // 其他正式所有者提供的原型只借用，不取得删除权。
        }
        let Some(preset) = presets.resolve(&key.preset_id) else { return; };
        match request_generic_mesh_profile(server, preset, key.catalog_revision) {
            Ok(load) => self.current_generic = Some(load),
            Err(error) => entry.failure = Some(error),
        }
    }

    /// 菜单里只结清已无人持有的加载，不创建原型、不请求新资产；打开文档的睡眠快照不在活动槽中。
    pub(super) fn reap_abandoned_load(&mut self, server: &AssetServer) {
        let Some(load) = self.current_generic.as_ref() else { return; };
        let key = DynamicMeshProfileKey { catalog_revision: load.catalog_revision,
            preset_id: load.preset_id.clone() };
        let abandoned = self.entries.get(&key).is_none_or(|entry| Arc::strong_count(&entry.token) == 1);
        if abandoned && generic_mesh_profile_finished(server, load) {
            self.current_generic = None; // 只有全部实际根句柄成功/失败后归还唯一并发槽。
        }
    }

    /// 每帧固定检查一条缓存；实例后台退役drop租约后不需扫描全部实例表。
    pub(super) fn collect_one(&mut self, registry: &mut VegetationMeshPrototypeRegistry) {
        let next = self.garbage_cursor.as_ref().and_then(|cursor| {
            self.entries.range((Excluded(cursor), Unbounded)).next().map(|(key, _)| key.clone())
        }).or_else(|| self.entries.keys().next().cloned());
        self.garbage_cursor = next.clone();
        let Some(key) = next else { return; };
        if self.current_generic.as_ref().is_some_and(|load| load.catalog_revision == key.catalog_revision
            && load.preset_id == key.preset_id) { return; }
        if self.entries.get(&key).is_none_or(|entry| Arc::strong_count(&entry.token) != 1) { return; }
        let entry = self.entries.remove(&key).expect("已检查最后租约");
        if let Some((revision, _)) = &entry.loaded {
            if registry.revision(entry.prototype_id) == Some(*revision) {
                registry.remove(entry.prototype_id); // 旧缓存不能移除同ID已被热更的新版本。
            }
            if self.registered.get(&entry.prototype_id) == Some(&(key, *revision)) {
                self.registered.remove(&entry.prototype_id);
            }
        }
        // 只drop共享强Handle；旧完整渲染根、编辑预览或兄弟仍持有的资产由Bevy最后引用回收。
        // Assets::remove会无视这些真实引用，不能用它强行释放旧材质或派生Mesh。
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
