//! 模型植被属于场景容器；独立发布槽不随检查器当前地形的作者Owner切换。
use bevy::prelude::*;
use feathrise_scene_content::{SceneId,SceneVegetationAsset};
use std::sync::Arc;

#[derive(Resource,Default,Clone)]
pub struct DynamicModelVegetationDraft { snapshot: Option<(SceneId,u64,Arc<SceneVegetationAsset>)> }
impl DynamicModelVegetationDraft {
    /// 小型布局与稀疏样本均浅克隆；二维密度不进入模型发布协议。
    pub fn publish(&mut self, scene_id:SceneId, revision:u64, definition:Arc<SceneVegetationAsset>) {
        if definition.scene_id == scene_id { self.snapshot=Some((scene_id,revision,definition)); }
    }
    pub fn clear(&mut self) { self.snapshot=None; }
    pub fn frozen_for_scene(&self, scene_id:&SceneId)->Option<(u64,Arc<SceneVegetationAsset>)> {
        self.snapshot.as_ref().filter(|(id,_,_)|id==scene_id).map(|(_,r,d)|(*r,d.clone()))
    }
}
