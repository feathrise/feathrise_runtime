//! F12完整植被草稿快照：Blade窗口与Mesh分块共同消费同批内存密度。

use bevy::prelude::*; // 内存图片强句柄与共享资源。
use feathrise_scene_content::{SceneId, SceneVegetationAsset}; // 草稿与正式定义复用同一协议。
use std::collections::BTreeMap; // 稳定层ID绑定同一批完成的密度句柄。
use std::collections::BTreeSet; // 后台准备顺带记录非零Mesh层，帧线程不扫描整张R8。

#[derive(Clone)]
pub struct DynamicBladeDraftSnapshot {
    pub source_revision: u64, // 作者编译修订；用于追踪完整快照来源，不阻塞F12关闭。
    pub sources: BTreeMap<String, std::sync::Arc<[u8]>>, // 后台冻结的原始密度供建筑净空共享，帧线程不复制整图。
    pub definition: SceneVegetationAsset, // 包含当前草稿全部Blade与Mesh正式输出，包括合法空列表。
    pub densities: BTreeMap<String, Handle<Image>>, // 全部图片登记完成后才发布。
    pub nonzero_layers: BTreeSet<String>, // 同一worker批次扫描所得，不参与磁盘协议。
}

#[derive(Resource, Default, Clone)]
pub struct DynamicBladeDraft {
    pub(crate) revision: u64, // 完整像素发布修订；净空和CPU来源按此追上最新草地。
    pub(crate) layout_revision: u64, // 密度刷印只更新既有草窗，布局/模型变化才重建整根。
    snapshot: Option<std::sync::Arc<DynamicBladeDraftSnapshot>>, // 后台比较/净空只借共享快照，不在帧上复制全部图层描述。
}

impl DynamicBladeDraft {
    /// 根布局修订与像素修订分离，连续刷草不重建既有LOD窗口。
    pub fn layout_revision_for_scene(&self, scene_id: &SceneId) -> Option<u64> {
        self.for_scene(scene_id).map(|_| self.layout_revision)
    }

    pub fn revision_for_scene(&self, scene_id: &SceneId) -> Option<u64> {
        self.for_scene(scene_id).map(|_| self.revision) // 运行时采用回执使用发布代次，跨场景不匹配。
    }

    pub fn for_scene(&self, scene_id: &SceneId) -> Option<&DynamicBladeDraftSnapshot> {
        self.snapshot
            .as_deref()
            .filter(|value| value.definition.scene_id == *scene_id) // 跨场景禁止消费旧草稿。
    }

    /// 后台派生冻结完整同代输入，图层描述和像素均只增加共享引用。
    pub fn frozen_for_scene(&self, scene_id: &SceneId) -> Option<std::sync::Arc<DynamicBladeDraftSnapshot>> {
        self.snapshot.as_ref().filter(|value| value.definition.scene_id == *scene_id).cloned()
    }

    pub fn publish(&mut self, snapshot: DynamicBladeDraftSnapshot) {
        self.snapshot = Some(std::sync::Arc::new(snapshot)); // 完整定义和全部图片一次采用。
        self.revision = self.revision.wrapping_add(1); // 运行时观察身份后走原有异步原子建根流程。
        self.layout_revision = self.layout_revision.wrapping_add(1);
    }

    /// 调用方已核对二维地形无三维继承，并在后台确认正式层规格及Mesh密度不变；Blade复用草窗更新。
    pub fn publish_density(&mut self, snapshot: DynamicBladeDraftSnapshot) {
        self.snapshot = Some(std::sync::Arc::new(snapshot));
        self.revision = self.revision.wrapping_add(1); // 净空及CPU来源仍观察本次完整像素修订。
    }

    pub fn clear(&mut self) {
        if self.snapshot.take().is_some() {
            // 已空状态不反复请求运行时重建。
            self.revision = self.revision.wrapping_add(1); // 正式磁盘来源重新成为权威。
            self.layout_revision = self.layout_revision.wrapping_add(1);
        }
    }
}
