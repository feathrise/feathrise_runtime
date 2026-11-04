//! 水面与地形大候选的共享逐帧登记预算与RenderApp准备回执。
//!
//! 【功能设计】正式运行时与F12共享逐帧登记额度；单水面上传块8MiB，地形块16MiB。
//! 块数和完整水体总量不限制。完整批次提交不可变ticket后，RenderApp每帧至多推进128项
//! 资产/管线检查，两轮完整准备证据必须跨帧；下一Extract回传身份。候选期间不授权互动。

use bevy::{
    asset::{AssetId, UntypedAssetId},
    pbr::{PreparedMaterial, SpecializedMaterialPipelineCache},
    prelude::*,
    render::{
        ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems,
        erased_render_asset::ErasedRenderAssets,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        mesh::RenderMesh,
        render_asset::RenderAssets,
        render_resource::{CachedPipelineState, PipelineCache},
        sync_world::MainEntity,
    },
};

use super::painted::StagedWaterSurface;
use std::sync::Arc;

/// 单上传块的逐帧工作额度；完整水体可以包含任意数量的块。
pub const MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME: usize = 8 * 1024 * 1024;
/// Geometry Clipmap组合Mesh已经由作者编译器限制到该上限。
pub const MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME: usize = 16 * 1024 * 1024;
const REQUIRED_READY_FRAMES: u8 = 2; // 排除Modified资产恰好处在替换边沿的单帧旧准备证据。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaterGpuUploadTicket(u64);

#[derive(Clone, Debug)]
struct WaterGpuUploadRequest {
    ticket: WaterGpuUploadTicket,
    mesh_ids: Arc<[AssetId<Mesh>]>,
    material_ids: Arc<[UntypedAssetId]>,
    warmup_entities: Arc<[Entity]>,
}

/// 主世界唯一登记额度与全部完整候选批次；共享Arc身份表每帧提取，不复制完整ID数组或Mesh数据。
#[derive(Resource, Clone, Debug, ExtractResource)]
pub struct WaterGpuUploadQueue {
    next_ticket: u64,
    registration_available: bool,
    requests: Vec<WaterGpuUploadRequest>,
}

impl Default for WaterGpuUploadQueue {
    fn default() -> Self {
        Self {
            next_ticket: 0,
            registration_available: true,
            requests: Vec::new(),
        }
    }
}

impl WaterGpuUploadQueue {
    /// 离屏回归只安装正式材质/Shader/回执链，避免把水花、玩家物理与地图加载混入GPU预热验收。
    #[cfg(test)]
    pub(crate) fn install_test_support(app: &mut App) {
        super::shaders::register_dynamic_water_shaders(app);
        app.add_plugins(bevy::pbr::MaterialPlugin::<super::DynamicWaterMaterial>::default());
        install_water_gpu_upload_monitor(app);
    }

    /// runtime与editor总计每帧只允许移动一张受检Mesh进入Assets。
    pub fn try_reserve_registration(&mut self, estimated_bytes: usize) -> Result<bool, String> {
        if estimated_bytes > MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME {
            return Err(format!(
                "水面单上传块GPU估算{estimated_bytes}字节超过每帧预算{MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME}字节"
            ));
        }
        if !self.registration_available {
            return Ok(false);
        }
        self.registration_available = false;
        Ok(true)
    }

    /// 地形Mesh或整批高度图集与水面共享同一个“本帧大候选”令牌，但使用各自静态字节上限。
    pub fn try_reserve_terrain_registration(
        &mut self,
        estimated_bytes: usize,
    ) -> Result<bool, String> {
        if estimated_bytes > MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME {
            return Err(format!(
                "地形GPU上传估算{estimated_bytes}字节超过每帧预算{MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME}字节"
            ));
        }
        if !self.registration_available {
            return Ok(false);
        }
        self.registration_available = false;
        Ok(true)
    }

    /// 只接受已经完整登记并统一重置材质时间的批次，后续不得向同ticket追加资产。
    pub fn submit(
        &mut self,
        surfaces: &[StagedWaterSurface],
        warmup_entities: &[Entity],
    ) -> WaterGpuUploadTicket {
        debug_assert_eq!(surfaces.len(), warmup_entities.len());
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        let ticket = WaterGpuUploadTicket(self.next_ticket);
        self.requests.push(WaterGpuUploadRequest {
            ticket,
            mesh_ids: surfaces
                .iter()
                .flat_map(|surface| surface.mesh_handles().map(Handle::id))
                .collect(),
            material_ids: surfaces
                .iter()
                .map(|surface| surface.material_handle().id().untyped())
                .collect(),
            warmup_entities: warmup_entities.into(),
        });
        ticket
    }

    /// 提交非水面大候选；调用前必须已经创建离屏承接实体并完成全部资产登记。
    pub fn submit_assets(
        &mut self,
        mesh_ids: Vec<AssetId<Mesh>>,
        material_ids: Vec<UntypedAssetId>,
        warmup_entities: Vec<Entity>,
    ) -> WaterGpuUploadTicket {
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        let ticket = WaterGpuUploadTicket(self.next_ticket);
        self.requests.push(WaterGpuUploadRequest {
            ticket,
            mesh_ids: mesh_ids.into(),
            material_ids: material_ids.into(),
            warmup_entities: warmup_entities.into(),
        });
        ticket
    }

    /// 采用、失败或换代都撤销目标；迟到回执因ticket永不复用而自然失效。
    pub fn retire(&mut self, ticket: WaterGpuUploadTicket) {
        self.requests.retain(|request| request.ticket != ticket);
    }
}

/// RenderApp只回传ticket集合；无RenderApp的纯ECS测试按资产登记完成立即放行。
#[derive(Resource, Default)]
pub struct WaterGpuUploadReceipts {
    render_monitor_available: bool,
    ready: Vec<WaterGpuUploadTicket>,
    failed: Vec<WaterGpuUploadTicket>,
}

impl WaterGpuUploadReceipts {
    pub fn is_ready(&self, ticket: WaterGpuUploadTicket) -> bool {
        !self.render_monitor_available || self.ready.contains(&ticket)
    }

    pub fn has_failed(&self, ticket: WaterGpuUploadTicket) -> bool {
        self.render_monitor_available && self.failed.contains(&ticket)
    }
}

#[derive(Clone, Copy, Debug)]
struct RenderTicketProgress {
    ticket: WaterGpuUploadTicket,
    ready_frames: u8,
    failed: bool,
    mesh_cursor: usize,
    material_cursor: usize,
    pipeline_cursor: usize,
}

#[derive(Resource, Default)]
struct WaterGpuUploadRenderState {
    progress: Vec<RenderTicketProgress>,
    ready: Vec<WaterGpuUploadTicket>,
    failed: Vec<WaterGpuUploadTicket>,
}

pub(super) fn install_water_gpu_upload_monitor(app: &mut App) {
    super::retirement::install(app);
    app.init_resource::<WaterGpuUploadQueue>()
        .init_resource::<WaterGpuUploadReceipts>()
        .add_plugins(ExtractResourcePlugin::<WaterGpuUploadQueue>::default())
        .add_systems(First, reset_registration_budget);
    if app.get_sub_app(RenderApp).is_none() {
        return; // MinimalPlugins测试没有GPU子应用，主世界回执按立即模式工作。
    }
    app.world_mut()
        .resource_mut::<WaterGpuUploadReceipts>()
        .render_monitor_available = true;
    let render_app = app
        .get_sub_app_mut(RenderApp)
        .expect("上方已确认RenderApp存在");
    render_app
        .init_resource::<WaterGpuUploadRenderState>()
        .add_systems(ExtractSchedule, publish_ready_receipts)
        .add_systems(
            Render,
            observe_prepared_water_assets.in_set(RenderSystems::Prepare),
        );
}

fn reset_registration_budget(mut queue: ResMut<WaterGpuUploadQueue>) {
    queue.registration_available = true;
}

/// Extract开始先回写上一完整Render帧；本帧目标随后由ExtractResourcePlugin正常提取。
fn publish_ready_receipts(
    mut main_world: ResMut<MainWorld>,
    render_state: Res<WaterGpuUploadRenderState>,
) {
    if let Some(mut receipts) = main_world.get_resource_mut::<WaterGpuUploadReceipts>() {
        receipts.ready.clone_from(&render_state.ready);
        receipts.failed.clone_from(&render_state.failed);
    }
}

fn observe_prepared_water_assets(
    queue: Res<WaterGpuUploadQueue>,
    meshes: Res<RenderAssets<RenderMesh>>,
    materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    specialized_pipelines: Res<SpecializedMaterialPipelineCache>,
    pipeline_cache: Res<PipelineCache>,
    mut state: ResMut<WaterGpuUploadRenderState>,
) {
    state.progress.retain(|progress| {
        queue
            .requests
            .iter()
            .any(|request| request.ticket == progress.ticket)
    });
    for request in &queue.requests {
        let progress = if let Some(index) = state
            .progress
            .iter()
            .position(|p| p.ticket == request.ticket)
        {
            &mut state.progress[index]
        } else {
            state.progress.push(RenderTicketProgress {
                ticket: request.ticket,
                ready_frames: 0,
                failed: false,
                mesh_cursor: 0,
                material_cursor: 0,
                pipeline_cursor: 0,
            });
            state.progress.last_mut().expect("GPU批次游标已建立")
        };
        if progress.failed || progress.ready_frames >= REQUIRED_READY_FRAMES {
            continue;
        }
        if request.mesh_ids.is_empty() || request.warmup_entities.is_empty() {
            continue;
        }
        // 两轮完整证明按固定小批推进；无限作者块数不能变成每帧全表扫描。
        for _ in 0..128 {
            if let Some(id) = request.mesh_ids.get(progress.mesh_cursor) {
                if meshes.get(*id).is_none() {
                    progress.ready_frames = 0;
                    break;
                }
                progress.mesh_cursor += 1;
            } else if let Some(id) = request.material_ids.get(progress.material_cursor) {
                if materials.get(*id).is_none() {
                    progress.ready_frames = 0;
                    break;
                }
                progress.material_cursor += 1;
            } else if let Some(entity) = request.warmup_entities.get(progress.pipeline_cursor) {
                let main_entity = MainEntity::from(*entity);
                let mut found = false;
                let mut ready = true;
                for view_cache in specialized_pipelines.values() {
                    let Some(pipeline) = view_cache.get(&main_entity) else {
                        continue;
                    };
                    found = true;
                    match pipeline_cache.get_render_pipeline_state(*pipeline) {
                        CachedPipelineState::Ok(_) => {}
                        CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
                            ready = false
                        }
                        CachedPipelineState::Err(_) => {
                            progress.failed = true;
                            ready = false;
                        }
                    }
                }
                if !found || !ready {
                    progress.ready_frames = 0;
                    break;
                }
                progress.pipeline_cursor += 1;
            } else {
                progress.ready_frames += 1;
                progress.mesh_cursor = 0;
                progress.material_cursor = 0;
                progress.pipeline_cursor = 0;
                break; // 下一完整轮必须从下一Render帧开始，不能同帧重复取得信用。
            }
        }
    }
    let ready = state
        .progress
        .iter()
        .filter(|progress| progress.ready_frames >= REQUIRED_READY_FRAMES)
        .map(|progress| progress.ticket)
        .collect();
    state.ready = ready;
    state.failed = state
        .progress
        .iter()
        .filter(|progress| progress.failed)
        .map(|progress| progress.ticket)
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_budget_is_shared_and_resets_explicitly() {
        let mut queue = WaterGpuUploadQueue::default();
        assert!(queue.try_reserve_registration(1024).unwrap());
        assert!(!queue.try_reserve_registration(1024).unwrap());
        queue.registration_available = true;
        assert!(queue.try_reserve_registration(1024).unwrap());
        queue.registration_available = true;
        assert!(queue.try_reserve_terrain_registration(16 * 1024).unwrap());
        assert!(!queue.try_reserve_registration(1024).unwrap()); // 地形候选占用后，同帧水面不得再登记。
        queue.registration_available = true;
        assert!(queue.try_reserve_registration(1024).unwrap());
        assert!(!queue.try_reserve_terrain_registration(16 * 1024).unwrap()); // 水面同样阻止高度页或Mesh抢占第二份额度。
        assert!(
            queue
                .try_reserve_registration(MAX_WATER_GPU_UPLOAD_BYTES_PER_FRAME + 1)
                .is_err()
        );
        assert!(
            queue
                .try_reserve_terrain_registration(MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME + 1)
                .is_err()
        );
    }
}
