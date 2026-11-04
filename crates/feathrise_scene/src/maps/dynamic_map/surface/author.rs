//! 未保存作者地表作为正式源的内存覆盖；保存采用严格文件事件门，Solari继续拥有原租约。

use bevy::prelude::*; // 仅传递固定大小的强句柄及身份。
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial; // 与正式地表相同材质。
use feathrise_map_content::{MapId, SurfaceCraterResponse, SurfaceSourceSaveIdentity, TerrainId}; // 跨地图或同图另一个画布的结果不得承接。
use std::sync::Arc;

/// 编辑器发布完整GPU候选；资源不写文件，也不直接修改正式地形组件。
#[derive(Resource, Default, Clone)]
pub struct DynamicSurfaceAuthorPreview {
    pub map_id: Option<MapId>, // 当前内存覆盖所属地图。
    pub terrain_id: Option<TerrainId>, // None仅属于旧单地形；实例资源交换也必须复核对象身份。
    pub revision: u64,         // 草稿修订，关闭编辑器不改变此身份。
    pub material: Option<Handle<TerrainSurfaceMaterial>>, // 已完整预热材质。
    pub crater_response: Option<Arc<SurfaceCraterResponse>>, // 与活动材质同一画布/像素/预设修订，关闭F12仍保留。
    pub(crate) saving: bool,   // 保存后只等待本次文件修订，不回旧活动材质。
    pub(crate) pending_images: u8, // 位零索引、位一权重，各自必须看到保存后的事件。
    pub(crate) pending_sources: u8, // CPU四槽raw同样必须成对采用，不能GPU先交还而技能仍读旧文件。
    pub(crate) expected_source: Option<Arc<SurfaceSourceSaveIdentity>>, // 原保存worker冻结的完整内容；旧journal不是新保存的证据。
    pub(crate) pending_definition: bool, // 定义修改同样必须消费正式资产事件。
    pub(crate) save_ticket: u64, // 每次保存严格不同，旧Ready无法完成新交接。
    pub saved_revision: u64,   // 保存时草稿身份，允许末笔候选尚在计算但旧画面继续保留。
}

impl DynamicSurfaceAuthorPreview {
    pub(crate) fn matches_terrain(&self, terrain: Option<&TerrainId>) -> bool {
        self.terrain_id.as_ref() == terrain || match (self.terrain_id.as_ref(), terrain) {
            // 旧单地形迁移仅认领primary；不同显式对象绝不能借用旧资源。
            (Some(id), None) | (None, Some(id)) => id == &TerrainId::legacy(),
            _ => false,
        }
    }

    /// 同修订稳定帧保持只读，新的完整作者候选替换整个覆盖。
    pub fn publish(
        &mut self,
        map_id: MapId,
        revision: u64,
        material: Handle<TerrainSurfaceMaterial>,
    ) {
        self.publish_for_terrain(map_id, None, revision, material);
    }

    /// 显式对象发布时冻结TerrainId；同一画布的新笔可继续保留上一完整材质。
    pub fn publish_for_terrain(
        &mut self,
        map_id: MapId,
        terrain_id: Option<TerrainId>,
        revision: u64,
        material: Handle<TerrainSurfaceMaterial>,
    ) {
        self.saving = false; // 只有脏草稿调用publish；新笔即使仍用上一幅完整材质也须取消旧保存接管。
        if self.map_id.as_ref() == Some(&map_id)
            && self.terrain_id == terrain_id
            && self.revision == revision
            && self.material.as_ref() == Some(&material)
        {
            return; // 稳定作者不重复改动句柄。
        }
        self.map_id = Some(map_id); // 采用同一完整地图身份。
        self.terrain_id = terrain_id;
        self.revision = revision; // 采用同一作者修订。
        self.material = Some(material); // GPU证据由发布方保证。
        self.crater_response = None; // 旧CPU画布不能借新材质身份，联合发布会补上同版响应。
        self.saving = false; // 新笔划撤销旧保存交接。
        self.pending_images = 0; // 新修订不继承旧文件事件。
        self.pending_sources = 0;
        self.expected_source = None;
        self.pending_definition = false; // 等待下一次明确保存。
    }

    /// 作者画面与技能响应原子接管；新候选等待时继续发布上一整套。
    pub fn publish_for_terrain_with_crater_response(
        &mut self,
        map_id: MapId,
        terrain_id: Option<TerrainId>,
        revision: u64,
        material: Handle<TerrainSurfaceMaterial>,
        crater_response: Option<Arc<SurfaceCraterResponse>>,
    ) {
        self.publish_for_terrain(map_id, terrain_id, revision, material);
        self.crater_response = crater_response;
    }

    /// 必须在成功保存值发布到AssetServer之前调用，冻结需要等待的正式事件。
    pub fn begin_saved_handoff(
        &mut self,
        map_id: &MapId,
        revision: u64,
        definition_changed: bool,
        images_changed: bool,
        expected_source: Option<Arc<SurfaceSourceSaveIdentity>>,
    ) {
        if self.map_id.as_ref() != Some(map_id) || self.material.is_none() {
            return; // 无作者覆盖不增加运行时等待。
        }
        self.saving = true; // 清脏不等于文件GPU就绪。
        self.saved_revision = revision; // 重载同图也不能继承旧保存责任。
        self.pending_definition = definition_changed; // 不接受保存前已Ready定义。
        self.pending_images = if images_changed { 3 } else { 0 }; // 两张图必须分别完成。
        self.pending_sources = if (images_changed || definition_changed) && self.crater_response.is_some() {
            3
        } else {
            0
        }; // 仅支持技能浅坑的Sparse候选需CPU门，旧GLB不能等待并不存在的稀疏页。
        self.expected_source = expected_source;
        self.save_ticket = self.save_ticket.wrapping_add(1).max(1); // 新保存强制独立候选。
    }

    /// 撤销干净、重新加载和换图明确撤销覆盖；关闭窗口不调用此入口。
    pub fn clear(&mut self) {
        self.map_id = None; // 保留单调保存票据，避免旧事务误认。
        self.terrain_id = None;
        self.material = None; // 句柄释放遵循Assets强引用生命周期。
        self.crater_response = None; // 正式接管/换图释放作者CPU覆盖，旧笔印自行保留施法快照。
        self.saving = false; // 丢弃未完成交接。
        self.pending_images = 0; // 清除事件等待。
        self.pending_sources = 0;
        self.expected_source = None;
        self.pending_definition = false; // 清除定义等待。
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    } // 发布方只读，禁止每帧重新塞回已交接材质。

    pub(super) fn file_inputs_ready(&self) -> bool {
        self.saving
            && self.pending_images == 0
            && self.pending_sources == 0
            && !self.pending_definition // CPU/GPU完整同次文件事件才允许正式候选。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 验证保存事件门与单调身份。
    #[test]
    fn surface_saved_handoff_waits_for_both_new_images_and_definition() {
        let map_id = MapId("AUTHOR-TEST".to_owned()); // 稳定地图身份。
        let mut author = DynamicSurfaceAuthorPreview::default(); // 首次无覆盖。
        let response = Arc::new(
            SurfaceCraterResponse::from_author_buffers(
                Vec2::ZERO,
                Vec2::ONE,
                [1, 1],
                Arc::new(vec![0; 16]),
                Arc::new(vec![255, 0, 0, 0]),
                vec![0.1],
            )
            .unwrap(),
        );
        author.publish_for_terrain_with_crater_response(
            map_id.clone(),
            None,
            3,
            Handle::default(),
            Some(response),
        ); // 模拟已就绪Sparse作者源。
        author.begin_saved_handoff(&map_id, 3, true, true, None); // 不接受旧文件Ready；此单测直接推进各门。
        let ticket = author.save_ticket; // 当前保存身份。
        assert!(!author.file_inputs_ready()); // 文件GPU旧Ready本身不能放行。
        author.pending_definition = false; // 只有定义更新还不够。
        author.pending_images &= !1; // 只有索引更新还不够。
        assert!(!author.file_inputs_ready()); // 权重未完成不能混用。
        author.pending_images &= !2; // 配对新权重已到。
        assert!(!author.file_inputs_ready()); // GPU齐全也不能忽略旧CPU作者源。
        author.pending_sources &= !1;
        assert!(!author.file_inputs_ready());
        author.pending_sources &= !2;
        assert!(author.file_inputs_ready()); // 才允许新正式GPU候选。
        author.begin_saved_handoff(&map_id, 3, false, true, None); // 再保存建立独立新票据。
        assert_ne!(author.save_ticket, ticket); // 旧候选不能消费新责任。
        assert!(!author.file_inputs_ready()); // 旧事件也不能继承。
        author.publish(map_id, 3, Handle::default()); // 新笔等待时仍是同一上一完整材质。
        assert!(!author.is_saving()); // 新笔先撤销旧保存接管。
    }

    #[test]
    fn legacy_author_surface_only_migrates_to_the_primary_canvas() {
        let mut author = DynamicSurfaceAuthorPreview::default();
        author.publish(MapId::parse("MAP-primary-surface").unwrap(), 3, Handle::default());
        assert!(author.matches_terrain(Some(&TerrainId::legacy())));
        assert!(!author.matches_terrain(Some(&TerrainId::parse("another").unwrap())));
        author.publish_for_terrain(MapId::parse("MAP-primary-surface").unwrap(), Some(TerrainId::legacy()), 3, Handle::default());
        assert!(author.matches_terrain(None));
        assert!(!author.matches_terrain(Some(&TerrainId::parse("another").unwrap())));
    }

    #[test]
    fn legacy_surface_without_sparse_cpu_response_keeps_existing_saved_handoff() {
        let map = MapId::parse("MAP-glb-surface").unwrap();
        let mut author = DynamicSurfaceAuthorPreview::default();
        author.publish(map.clone(), 2, Handle::default());
        author.begin_saved_handoff(&map, 2, false, true, None);
        assert_eq!(
            author.pending_sources, 0,
            "旧GLB或体素不支持浅坑，不能等待不存在的稀疏CPU源"
        );
        author.pending_images = 0;
        assert!(author.file_inputs_ready());
    }
}
