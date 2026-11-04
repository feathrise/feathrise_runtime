//! 积水运行时只保存当前地图和一份候选/活动纹理，不存逐水洼实体。

use bevy::prelude::*; // 资源、强句柄和固定世界范围。
use feathrise_map_content::{MapId, MapPuddleDescriptor}; // 独立作者身份和受检文件规格。

#[derive(Resource, Default)] // 换图整体回到无积水，防止旧世界范围泄漏。
pub(crate) struct PuddleRuntimeState {
    // 一个活动地图、一个源候选和一个完整文件版本。
    pub(super) comparison: Option<bevy::tasks::Task<(Image, bool)>>, // 保存交接最多一个后台内容校验任务，像素所有权直接移入移出。
    pub(super) comparison_ticket: Option<(MapId, u64, String, MapPuddleDescriptor, Vec4)>, // 地图、保存修订、路径、规格与范围共同验证迟到结果。
    pub(super) map_id: Option<MapId>, // 当前渲染采用的真实地图身份。
    pub(super) requested: Option<MapPuddleDescriptor>, // 最近请求的受检源规格，变化时才加载。
    pub(super) source: Option<Handle<Image>>, // AssetServer专用源句柄，只接收下一完整热更。
    pub(super) file_mask: Option<Handle<Image>>, // 已移交到独立身份的完整正式版本。
    pub(super) file_descriptor: Option<MapPuddleDescriptor>, // 已采用版本的精确规格，用于区分仅范围变化与等待中的新图。
    pub(super) file_bounds: Vec4,                            // 与完整文件版本一起采用的世界范围。
    pub(super) bounds: Vec4,                                 // 当前已采用文件或作者草稿的世界范围。
    pub(super) preview: bool, // 作者潜力蓝色覆盖，与游戏天气状态分离。
    pub(super) error: Option<String>, // 相同失败只记录一次，不在稳定帧持续刷日志。
} // 完成固定规模候选状态。

/// 同源引用共享受检的完整纹理身份；缓存只保存AssetId，最后一个实例关闭后图片仍可正常卸载。
#[derive(Resource, Default)]
pub(crate) struct PuddleFileCache {
    pub(super) paths: std::collections::BTreeMap<String, (MapPuddleDescriptor, AssetId<Image>)>,
    pub(super) ids: std::collections::HashMap<AssetId<Image>, String>,
}
impl PuddleFileCache {
    pub(super) fn insert(&mut self, descriptor: &MapPuddleDescriptor, image: AssetId<Image>) {
        let path = descriptor.texture_path.clone();
        if let Some((_, old)) = self.paths.insert(path.clone(), (descriptor.clone(), image)) { self.ids.remove(&old); }
        self.ids.insert(image, path);
    }
}
impl PuddleRuntimeState {
    pub(crate) fn world_bounds(&self) -> Vec4 { self.bounds }
    pub(crate) fn ready(&self) -> Result<bool, String> {
        if let Some(error) = &self.error { return Err(error.clone()); }
        Ok(self.requested.is_none() || (self.file_descriptor == self.requested && self.file_mask.is_some()))
    }
    pub(crate) fn busy(&self, server: &AssetServer) -> bool {
        self.comparison.is_some() || self.source.as_ref().is_some_and(|source|
            matches!(server.get_load_state(source.id()), Some(bevy::asset::LoadState::Loading)))
    }
}
