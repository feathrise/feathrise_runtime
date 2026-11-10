//! 动态场景地表纹理组和唯一会话状态。

use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;
use feathrise_scene_content::{SceneId, SceneSurfaceAsset};

/// 索引、权重与三张全局 PBR 数组组成不可拆分纹理组。
#[derive(Clone, Debug)]
pub(super) struct DynamicSurfaceTextures {
    pub(super) neutral_arrays: bool, // 空作者表绑定内部图片，不等待不存在的磁盘加载状态。
    pub(super) weight: Handle<Image>,
    pub(super) index: Handle<Image>,
    pub(super) base_color_array: Handle<Image>,
    pub(super) normal_array: Handle<Image>,
    pub(super) arm_height_array: Handle<Image>,
}

impl DynamicSurfaceTextures {
    pub(super) fn images(&self) -> [Handle<Image>; 5] {
        [
            self.weight.clone(),
            self.index.clone(),
            self.base_color_array.clone(),
            self.normal_array.clone(),
            self.arm_height_array.clone(),
        ]
    }

    pub(super) fn matches(&self, id: AssetId<Image>) -> bool {
        self.weight.id() == id
            || self.index.id() == id
            || self.base_color_array.id() == id
            || self.normal_array.id() == id
            || self.arm_height_array.id() == id
    }
}

/// 当前DynamicSceneRoot唯一地表会话。
#[derive(Resource, Default)]
pub(crate) struct DynamicSurfaceState {
    pub(super) scene_id: Option<SceneId>,
    pub(super) definition: Option<SceneSurfaceAsset>,
    pub(super) definition_handle: Option<Handle<SceneSurfaceAsset>>,
    pub(super) world_min: Vec2,
    pub(super) world_max: Vec2,
    pub(super) textures: Option<DynamicSurfaceTextures>,
    pub(super) texture_probe: Option<Entity>,
    pub(super) source_entity: Option<Entity>,
    pub(super) white_material: Option<Handle<StandardMaterial>>,
    pub(super) author_material: Option<Handle<TerrainSurfaceMaterial>>, // 作者源优先于文件源，资产仍由编辑器持有。
    pub(super) author_save_ticket: u64,                                 // 当前保存候选身份。
    pub(super) active_material: Option<Handle<TerrainSurfaceMaterial>>,
    pub(super) candidate_material: Option<Handle<TerrainSurfaceMaterial>>,
    pub(super) active_layer_table: Option<Handle<ShaderBuffer>>, // 正式材质当前绑定的局部层表。
    pub(super) candidate_layer_table: Option<Handle<ShaderBuffer>>, // 与唯一预热候选同生共死。
    pub(super) warmup_entity: Option<Entity>,
    pub(super) warmup_source_entity: Option<Entity>, // 统一取消时即使正式源已换代也能移除旧源single-flight标记。
    pub(super) generation: u32,
    pub(super) revision: u32,
    pub(super) catalog_revision: u64,
    pub(super) preset_catalog_revision: u64, // 全局预设参数或数组修订同样使当前场景失效。
    pub(super) definition_dirty: bool,
    pub(super) reload_requested: bool,
    /// 目录热更新移除地表声明时，由安装系统用完整材质权限恢复白模。
    pub(super) white_requested: bool,
    /// JSON加载或定义校验失败的具体原因；安装系统采用同一失败白模事务。
    pub(super) definition_failure_detail: Option<String>,
    pub(super) failure_logged_revision: Option<u32>,
}

impl DynamicSurfaceState {
    pub(in crate::scenes::dynamic_scene) fn effective_material(&self) -> Option<&Handle<TerrainSurfaceMaterial>> {
        self.author_material
            .as_ref()
            .or(self.active_material.as_ref()) // 正式源及Solari租约核对同一有效源。
    }

    pub(super) fn begin(&mut self, generation: u32) {
        *self = Self {
            generation: generation.max(1),
            revision: 1,
            definition_dirty: true,
            ..default()
        };
    }

    pub(super) fn request_reload(&mut self) {
        if !self.reload_requested {
            self.revision = self.revision.wrapping_add(1).max(1);
            self.reload_requested = true;
            self.failure_logged_revision = None;
        }
    }

    pub(super) fn current(&self, generation: u32, revision: u32) -> bool {
        self.generation == generation && self.revision == revision
    }
}
