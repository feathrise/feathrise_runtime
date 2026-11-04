//! Geometry Clipmap 的分层稀疏高度页常驻与正式地表材质桥。
//!
//! 每个采样Mip按编译后的真实环带范围、64格驻留保护带和法线邻点选择世界固定页；
//! 全部层级高度数组、紧凑页表和材质绑定在后台结果通过地图、根实体、规格、显示计划、
//! 清单修订和票据身份核对后一次采用。旧完整页组会保留到新组就绪，主线程只轮询
//! AssetServer 与后台任务。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use std::collections::{BTreeMap, BTreeSet}; // 固定页身份排序让依赖和测试结果保持确定。
use std::sync::Arc; // AssetServer 块在碰撞与 GPU 页任务之间零深拷贝共享。
use std::time::{Duration, Instant}; // GPU候选登记、两帧预热和30秒超时使用单调墙钟。
#[cfg(test)]
#[path = "height_domain_tests.rs"]
mod height_domain_tests; // 浮点作者高度超出旧量程时，完整页、拾取与草地仍同版。
#[cfg(test)]
#[path = "surface_frame_tests.rs"]
mod surface_frame_tests; // 地表包装与保存热替换必须继承当前高度页来源框架。

use bevy::asset::{AssetEvent, AssetId, LoadState, RenderAssetUsages, UntypedAssetId}; // 候选来源、热更、身份与程序图片用途。
use bevy::camera::visibility::NoFrustumCulling; // 离屏微缩预热仍需进入实际管线专门化。
use bevy::ecs::system::SystemParam; // 把同属高度分页域的资产访问合并为一个系统参数，避免帧系统参数膨胀。
use bevy::image::ImageSampler; // 数值高度在页内使用线性过滤。
use bevy::pbr::ExtendedMaterial; // 把现有 TerrainSurfaceMaterial 包装为唯一正式材质。
use bevy::prelude::*; // ECS、图片、相机与数学类型。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 两张 GPU 纹理的固定格式。
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready}; // 页编码在有界后台任务完成。
use feathrise_environment::terrain_surface::{
    TerrainSurfaceCarrier, TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride,
}; // 内层作者地表先完成既有 GPU 预热事务。
use feathrise_map_content::{
    ActiveDynamicMap, CompiledDisplayLodPlan, DISPLAY_LOD_RESIDENCY_GUARD_CELLS, MapId,
    MapTerrainDisplayLodPhase, MapTerrainDisplayLodStatus, SparseCraterTiles,
    SparseHeightLiveOverride, SparseHeightLiveSnapshot, SparseHeightManifest, SparseHeightManifestAsset,
    SparseHeightSessionCrater, SparseHeightSpec, SparseHeightTile, SparseHeightTileAsset,
    SparseHeightTileKey, sample_crater_height,

}; // 清单、内容寻址块与世界格点规则保持同源。

use super::{
    CLIPMAP_TILE_CELLS, ClipmapBounds, ClipmapFlatHeightPage, ClipmapHeightAtlas,
    ClipmapHeightExtension, ClipmapHeightMaterial, SparseClipmapSurfaceLease,
    SparseClipmapSurfaceMaterial, encode_page_table_entry,
}; // 材质绑定和 GPU 页协议由同一模块定义。
use crate::maps::dynamic_map::resources::{
    DynamicMapRuntimePhase, DynamicMapRuntimeState, DynamicMapRuntimeStatus,
}; // 页失败与当前地图加载门共享可读状态。
use crate::maps::dynamic_map::sparse_runtime::{
    SparseClipmapDisplay, // 高度页以实际显示网格为唯一视图真值。
    SparseClipmapRuntime,
}; // F12和玩法相机已经由几何域统一交接，本域读取实际显示位置。
use crate::{DynamicSparseClipmapPrimitive, WaterGpuUploadQueue, WaterGpuUploadReceipts}; // 高度页与水面共享大候选登记额度及RenderApp回执。 // F12轨道焦点与显示编译计划共同决定真实常驻窗口。 // 角色承托门使用唯一已采用形状。

const PAGE_POINTS: u32 = CLIPMAP_TILE_CELLS + 1; // 每层复制右、上共享边，始终为 257×257。
const MAX_PAGE_UPLOAD_BYTES: usize = 8 * 1024 * 1024; // 设计预算：一次采用的高度数组加页表不超过八 MiB。
const MAX_PAGE_IO_TASKS: usize = 3; // 显示高度流送保留一个槽给植被射线；IO和页编码不同时启动。

/// 一张 GPU 页的世界固定层级和页坐标；层级零是作者页，其余是持久父页。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ResidentPageKey {
    level: u8,                // Shader 按环带请求选择相同层级。
    key: SparseHeightTileKey, // 当前层页表坐标。
}

/// 一份正式内容地址依赖；文件路径只由受检身份重新构造。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PageDependency {
    page: ResidentPageKey, // 基础页或父页的完整身份。
    sha256: String,        // 清单当前修订声明的不可变内容。
}

/// 当前相机窗口的一份不可混用请求身份。
#[derive(Clone, Debug, PartialEq)]
struct PageSetIdentity {
    map_id: MapId,                        // 换图后旧任务必定失效。
    root: Entity,                         // 同图重建根也不复用旧结果。
    manifest_revision: u64,               // 清单热更严格换代。
    live_revision: Option<u64>,           // F12 未保存块变化同样使迟到页任务失效。
    crater_revision: Option<u64>,         // 技能破坏与作者修订分别换代，不能复活旧GPU页。
    has_live_pages: bool,                 // 旧内容全图是否已有实时/浅坑页；只存标量，不持有旧高度或全图占用表。
    height_bounds: [f32; 2],              // 当前完整源的保守界随身份冻结；等待新页时不得借新高程修改旧显示。
    display_plan: CompiledDisplayLodPlan, // 网格、页范围和候选材质必须来自同一显示配置身份。
    pages: Vec<ResidentPageKey>,          // 各层正式非平地、边界邻域或显式实时页。
    dependencies: Vec<PageDependency>,    // 还包含每页右、上和右上共享边的正式所有者页。
}

/// AssetServer 正在为一个冻结请求持有的强块句柄。
struct PageLoadSet {
    identity: PageSetIdentity, // 句柄只可服务这一清单和窗口。
    handles: Vec<(PageDependency, Handle<SparseHeightTileAsset>)>, // 全层正式依赖仍受固定页预算限制。
    crater_tiles: SparseCraterTiles, // 会话破坏同样在任务启动前冻结Arc快照。
}

/// 后台任务完成的纯 CPU 上传字节；主线程只登记两份 Image。
struct BuiltPageSet {
    identity: PageSetIdentity, // 采用前再次与当前 desired 精确比较。
    height_bytes: Vec<u8>,     // 逐层堆叠的 R16_UNORM 数据。
    layer_count: u32,          // texture_2d_array 的实际层数，至少二。
    page_table_bytes: Vec<u8>, // RGBA32_UINT，未驻留入口全零。
    table_size: UVec2,         // 各层纵向堆叠，最大地图为 256×511。
    max_mip: u32,              // 紧凑页表实际包含的最高父页层级。
    cpu: Arc<super::ground::GroundPageBytes>, // 后台冻结同一量化字节，主线程和查询只共享Arc。
}

/// 已登记GPU图片但尚未与匹配Mesh和材质通过预热门的完整页候选。
struct RegisteredPageCandidate {
    identity: PageSetIdentity, // 完整地图、根、修订、焦点窗口与显示计划身份。
    extension: ClipmapHeightExtension, // 只绑定候选图集，不改当前正式材质扩展。
    atlas: Option<ClipmapHeightAtlas>, // 全平候选为None，正式采用时清除旧图集。
    cpu: Option<Arc<super::ground::GroundPageBytes>>, // 与候选图集同一量化结果；全平候选没有页字节。
    registered_at: Instant,                           // 上传等待诊断从高度页批次登记开始。
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayMaterialSource {
    Terrain(AssetId<TerrainSurfaceMaterial>), // 已完成自身预热的作者地表来源。
    Standard(AssetId<StandardMaterial>),      // 首载白模或地表失败回退来源。
    Fallback(AssetId<ClipmapHeightMaterial>), // 当前已经包装高度的基础PBR来源。
}

enum DisplayCandidateMaterial {
    Fallback(Handle<ClipmapHeightMaterial>),
    Surface {
        source: Handle<TerrainSurfaceMaterial>,
        material: Handle<SparseClipmapSurfaceMaterial>,
    },
}

impl DisplayCandidateMaterial {
    fn untyped_id(&self) -> UntypedAssetId {
        match self {
            Self::Fallback(material) => material.id().untyped(),
            Self::Surface { material, .. } => material.id().untyped(),
        }
    }
}

/// Mesh、完整页图集和匹配地表来源的材质已经登记，等待RenderApp连续两帧证据。
struct SparseDisplayGpuTransaction {
    display_identity: crate::maps::dynamic_map::sparse_runtime::DisplayBuildIdentity,
    page_identity: PageSetIdentity,
    mesh: Handle<Mesh>,
    material: DisplayCandidateMaterial,
    source: DisplayMaterialSource,
    warmup_entity: Entity,
    ticket: Option<crate::WaterGpuUploadTicket>, // 无RenderApp纯ECS测试使用None并走两主帧门。
    observed_frames: u8,
    started_at: Instant,
    build_milliseconds: Option<f32>, // 新网格采用时发布；页窗口换代复用旧诊断。
}

/// 新正式版本至少再完成一次提取后才从Assets退役的旧显示资产。
struct RetiredSparseDisplayAssets {
    frames_remaining: u8,
    mesh: Option<Handle<Mesh>>,
    atlas: Option<ClipmapHeightAtlas>,
    fallback: Option<Handle<ClipmapHeightMaterial>>,
    surface: Option<Handle<SparseClipmapSurfaceMaterial>>,
    cpu: Option<Arc<super::ground::GroundPageBytes>>, // 与旧图集同代，至少保留到下一次提取完成。
}

/// 新图高度页与外层材质的唯一会话状态。
#[derive(Resource, Default)]
pub(crate) struct SparseHeightPageRuntime {
    pub(super) stitches: Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,
    pub(super) author_stitches: Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,
    stitch_bounds: Option<Vec2>,
    map_id: Option<MapId>,                                       // 当前目标地图。
    root: Option<Entity>,                                        // 当前目标根实体。
    path: Option<String>,                                        // 总清单声明的稀疏高度路径。
    manifest: Option<Handle<SparseHeightManifestAsset>>,         // 强句柄保持清单常驻。
    manifest_revision: u64,                                      // 每次真实清单事件递增。
    manifest_dirty: bool,                                        // 只在首载或事件边界重新验证。
    spec: Option<SparseHeightSpec>,                              // 当前 GPU 世界映射规格。
    pub(super) extension: Option<ClipmapHeightExtension>,        // Clipmap线框只读当前已采用绑定。
    pub(super) extension_revision: u64,                          // 页组或规格每次原子采用后递增。
    hole_revision: u64, // 洞口身份独立于页身份，不让权威页缓存误判为高度换代。
    desired: Option<PageSetIdentity>, // 当前相机真正需要的窗口。
    loading: Option<PageLoadSet>, // 等待 AssetServer 的局部块集合。
    pending_io: Vec<Handle<SparseHeightTileAsset>>, // 换窗后未真正完成的IO仍持有槽，最多三个。
    task: Option<Task<Result<BuiltPageSet, String>>>, // 至多一个 CPU 页编码任务。
    building: Option<PageSetIdentity>, // 在途任务身份独立于最新期望，迟到错误不得污染新窗口。
    release_task: Option<Task<()>>, // 至多一个后台析构任务，避免8MiB迟到上传字节在主线程释放。
    prepared: Option<BuiltPageSet>, // 当前完整GPU页组候选，只核验显示页身份后采用。
    candidate: Option<RegisteredPageCandidate>, // 已登记图片等待同票据Mesh、材质和RenderApp预热。
    atlas: Option<ClipmapHeightAtlas>, // 当前完整 GPU 页组的强句柄。
    resident_identity: Option<PageSetIdentity>, // 相机远离浅坑后仍可确认当前GPU视图已采用哪一修订。
    failed_identity: Option<PageSetIdentity>, // GPU或页构建失败后等待新窗口/修订，不稳定帧重试。
    pub(super) cpu: Option<Arc<super::ground::GroundPageBytes>>, // CPU与GPU在完整采用边界一起换代。
    retired_cpu: Vec<Arc<super::ground::GroundPageBytes>>, // 候选或旧正式量化页排队进入唯一后台释放槽。
    fallback_material: Option<Handle<ClipmapHeightMaterial>>, // 地表PBR尚未就绪时仍必须显示GPU高度位移。
    fallback_extension_revision: u64, // 基础高度材质已经采用的页修订，稳定帧不得反复触发资产换代。
    wrapped_source: Option<Handle<TerrainSurfaceMaterial>>, // 当前被包装的内层作者材质。
    wrapped_material: Option<Handle<SparseClipmapSurfaceMaterial>>, // 唯一外层材质资产。
    wrapped_extension_revision: u64,  // 防止稳定帧重复触发 Asset::Modified。
    transaction: Option<SparseDisplayGpuTransaction>, // 同时最多一个离屏预热候选。
    retired: Vec<RetiredSparseDisplayAssets>, // 通常至多一项，延迟一轮提取后回收。
}

/// 宿主只把并发席位归还给真正结清后台工作的实例。
impl SparseHeightPageRuntime {
    pub(in crate::maps::dynamic_map) fn seed_stitches(&mut self,profiles:&[feathrise_map_content::ClipmapEdgeProfile]) {
        let author=super::stitches::collision_constraints(profiles);let mut display=author;
        // 隐藏候选尚未取得另一侧LOD时采用最粗共同边；所有实例就绪前由发布器协调回实际精度。
        for edge in &mut display.edges {if edge.x>=2.0{edge.w=edge.x-1.0;}}
        self.stitch_bounds=display.height_range();self.stitches=Arc::new(display);self.author_stitches=Arc::new(author);
    }
    pub(super) fn synchronize_stitches(&mut self, data:Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,author:Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,white:&mut Assets<ClipmapHeightMaterial>,surfaces:&mut Assets<SparseClipmapSurfaceMaterial>) {
        if *self.stitches==*data && *self.author_stitches==*author {return;}
        self.author_stitches=author;
        self.stitch_bounds=data.height_range();
        self.stitches=data.clone();
        if let Some(extension)=&mut self.extension {extension.params.stitches=*data;}
        if let Some(candidate)=&mut self.candidate {candidate.extension.params.stitches=*data;}
        let mut fallback=[self.fallback_material.as_ref(),None];let mut wrapped=[self.wrapped_material.as_ref(),None];
        if let Some(transaction)=&self.transaction {match &transaction.material {
            DisplayCandidateMaterial::Fallback(handle)=>fallback[1]=Some(handle),
            DisplayCandidateMaterial::Surface{material,..}=>wrapped[1]=Some(material),
        }}
        for handle in fallback.into_iter().flatten(){if let Some(mut m)=white.get_mut(handle){m.extension.params.stitches=*data;}}
        for handle in wrapped.into_iter().flatten(){if let Some(mut m)=surfaces.get_mut(handle){m.extension.params.stitches=*data;}}
    }
    pub(in crate::maps::dynamic_map) fn set_source_frame(&mut self, frame: bevy::math::Mat4) {
        if let Some(extension) = &mut self.extension { extension.set_source_frame(frame); }
    }
    pub(in crate::maps::dynamic_map) fn background_busy(&self) -> bool {
        self.task.is_some() || self.release_task.is_some() || self.loading.is_some() || !self.pending_io.is_empty()
    }
    pub(in crate::maps::dynamic_map) fn background_finished(&self, server: &AssetServer) -> bool {
        let complete = |handle: &Handle<SparseHeightTileAsset>| !matches!(server.load_state(handle.id()),
            bevy::asset::LoadState::Loading | bevy::asset::LoadState::NotLoaded);
        self.task.as_ref().is_none_or(|task| task.is_finished()) && self.release_task.as_ref().is_none_or(|task| task.is_finished())
            && self.pending_io.iter().all(complete)
            && self.loading.as_ref().is_none_or(|load| load.handles.iter().all(|(_, handle)| complete(handle)))
    }
}

impl SparseHeightPageRuntime {
    /// 固定数量的正式/候选扩展共同裁切；稳定帧先读再写，不制造材质Modified。
    pub(crate) fn synchronize_holes(
        &mut self,
        holes: super::material::ClipmapHoleUniform,
        revision: u64,
        white: &mut Assets<ClipmapHeightMaterial>,
        surfaces: &mut Assets<SparseClipmapSurfaceMaterial>,
    ) {
        self.hole_revision = revision;
        if let Some(extension) = self.extension.as_mut() {
            extension.holes = holes;
        }
        if let Some(candidate) = self.candidate.as_mut() {
            candidate.extension.holes = holes;
        }
        let mut fallback = [self.fallback_material.as_ref(), None];
        let mut wrapped = [self.wrapped_material.as_ref(), None];
        if let Some(transaction) = self.transaction.as_ref() {
            match &transaction.material {
                DisplayCandidateMaterial::Fallback(handle) => fallback[1] = Some(handle),
                DisplayCandidateMaterial::Surface { material, .. } => wrapped[1] = Some(material),
            }
        }
        for handle in fallback.into_iter().flatten() {
            if white
                .get(handle)
                .is_some_and(|material| material.extension.holes != holes)
            {
                if let Some(mut material) = white.get_mut(handle) {
                    material.extension.holes = holes;
                }
            }
        }
        for handle in wrapped.into_iter().flatten() {
            if surfaces
                .get(handle)
                .is_some_and(|material| material.extension.holes != holes)
            {
                if let Some(mut material) = surfaces.get_mut(handle) {
                    material.extension.holes = holes;
                }
            }
        }
    }
    pub(super) fn ground_content_revision(
        &self,
    ) -> Option<super::ground::ClipmapGroundContentRevision> {
        let resident = self.resident_identity.as_ref()?; // CPU查询只能描述当前正式采用页组。
        Some(super::ground::ClipmapGroundContentRevision {
            root: resident.root.to_bits(),
            manifest: resident.manifest_revision,
            live: resident.live_revision,
            crater: resident.crater_revision,
            topology: resident.display_plan.authored_lods.iter().fold(
                0xcbf2_9ce4_8422_2325_u64,
                |mut hash, lod| {
                    for value in [
                        lod.spacing_multiplier,
                        lod.outer_half_extent_meters.map_or(u32::MAX, f32::to_bits),
                    ] {
                        hash ^= u64::from(value);
                        hash = hash.wrapping_mul(0x100_0000_01b3);
                    }
                    hash
                },
            ), // 只编码作者设定；相机派生精度/覆盖属于查询身份，不能触发树木重建和作者页失效。
        })
    }

    /// 当前完整CPU/GPU版本的保守界；新实时根或候选页尚未采用时继续返回旧版范围。
    pub(crate) fn adopted_height_bounds(&self, map: &MapId) -> Option<[f32; 2]> {
        let resident = self.resident_identity.as_ref()?;
        if self.map_id.as_ref() != Some(map) || resident.map_id != *map || self.root != Some(resident.root) {
            return None;
        }
        let mut bounds = resident.height_bounds;
        if let Some(cpu) = &self.cpu {
            bounds = [bounds[0].min(cpu.height_bounds[0]), bounds[1].max(cpu.height_bounds[1])];
        }
        if let Some(stitch) = self.stitch_bounds {
            bounds = [bounds[0].min(stitch.x), bounds[1].max(stitch.y)];
        }
        Some(bounds)
    }

    pub(crate) fn blade_height(
        &self,
        map: &MapId,
        plan: &CompiledDisplayLodPlan,
        center: Vec2,
    ) -> Option<feathrise_environment::vegetation::blade::GpuBladeClipmapHeight> {
        // 同代页表和真实网格快照。
        if self.map_id.as_ref() != Some(map) {
            return None;
        } // 拒绝旧地图。
        let spec = self.spec.as_ref()?; // 受检规格。
        let resident = self.resident_identity.as_ref()?;
        if self.root != Some(resident.root) || resident.map_id != *map
            || resident.display_plan != *plan || plan.bands.is_empty() || plan.bands.len() > 16 {
            return None; // 正式几何不得搭配另一份作者计划或不可表达的环带列表。
        }
        let mut bands = [feathrise_environment::vegetation::blade::GpuBladeClipmapBand::default(); 16];
        let mut previous_mip = None;
        for (target, source) in bands.iter_mut().zip(&plan.bands) {
            let geometry_mip = source.spacing_multiplier.trailing_zeros();
            if previous_mip.is_some_and(|previous| geometry_mip != previous + 1) {
                return None; // 编译后的相邻环带必须保持2:1，GPU据此用槽序恢复Mip。
            }
            *target = feathrise_environment::vegetation::blade::GpuBladeClipmapBand {
                inner_half_cells: UVec2::from_array(source.inner_half_cells),
                outer_half_cells: UVec2::from_array(source.outer_half_cells),
                geometry_mip,
            };
            previous_mip = Some(geometry_mip);
        }
        let (atlas, page_table) = self.extension.as_ref()?.blade_textures(); // 共用地表资源。
        let height_bounds = self.adopted_height_bounds(map)?;
        Some(feathrise_environment::vegetation::blade::GpuBladeClipmapHeight {
            stitches:self.stitches.clone(),
            // 完整绑定。
            atlas, // 唯一图集。
            holes: self.extension.as_ref()?.holes.rects,
            hole_count: self.extension.as_ref()?.holes.info.x as u32,
            page_table,                                  // 同代页表。
            world_min: Vec2::from_array(spec.world_min), // 世界原点。
            world_max: Vec2::from_array(spec.world_max), // 两轴边界。
            cell_size: Vec2::new(
                spec.actual_spacing_meters[0] as f32,
                spec.actual_spacing_meters[1] as f32,
            ), // 真实格距。
            logical_cells: UVec2::new(spec.cells[0] as u32, spec.cells[1] as u32), // 受检逻辑尺寸。
            max_mip: u32::from(spec.max_mip_level()), // 全平旧图集也按真实页级检查缺页掩码，不能把新父页需求错误钳到零级。
            fine_center: center,                      // 正式显示中心。
            bands,                                    // 已采用可变矩形环带的固定快照。
            band_count: plan.bands.len() as u32,      // 编译器和上方硬门保证不超过十六。
            height_min: height_bounds[0],
            height_max: height_bounds[1],
            revision: [
                self.root?.to_bits(),
                self.extension_revision ^ self.hole_revision.rotate_left(32),
                ((u64::from(center.x.to_bits()) << 32) | u64::from(center.y.to_bits())) ^ self.stitches.revision(),
            ], // 地图/高度/视图身份。
            ready: self.resident_content_is_current(), // 同内容旧图集继续服务已有区域，新增缺页仅局部跳过候选。
            missing_pages: self.blade_missing_pages(), // 明确区分未编辑平地与尚未到达的非平页，避免全域关草。
        }) // 固定大小快照。
    } // 不反向修改地形。

    /// 草地显示跟随已经实际绘制的完整地形，实时雕刻的新候选不能提前关闭上一整片草。
    /// 作者查询继续使用blade_height的严格内容门；这里保留的旧版绝不冒充最新作者修订。
    pub(crate) fn blade_render_height(
        &self,
        map: &MapId,
        plan: &CompiledDisplayLodPlan,
        center: Vec2,
    ) -> Option<feathrise_environment::vegetation::blade::GpuBladeClipmapHeight> {
        let mut height = self.blade_height(map, plan, center)?;
        if height.ready { return Some(height); }
        let (Some(desired), Some(resident)) = (&self.desired, &self.resident_identity) else { return Some(height); };
        let same_source = desired.map_id == resident.map_id && desired.map_id == *map
            && desired.root == resident.root && self.root == Some(resident.root)
            && desired.manifest_revision == resident.manifest_revision
            && (desired.live_revision != resident.live_revision || desired.crater_revision != resident.crater_revision)
            && desired.dependencies.iter().all(|dependency| resident.dependencies.binary_search_by_key(&dependency.page, |entry| entry.page)
                .map_or(true, |index| resident.dependencies[index].sha256 == dependency.sha256));
        if same_source {
            // extension与resident只有在Mesh/页组/材质完整GPU预热后才共同更换，所以仍是真正屏上的旧表面。
            // 正式清单的非平缺页仍须裁剪；只有旧版确定没有实时页且缺少正式依赖时，才可证明它原是平地。
            // 旧版已有实时页时，对局部身份无法判定的新窗口缺页保守等待，不能把未知页误当平地。
            height.ready = true;
            height.missing_pages = self.blade_missing_pages_for(true);
        }
        Some(height)
    }

    fn resident_content_is_current(&self) -> bool {
        // 页请求变动不等于已采用高度内容失效。
        let (Some(desired), Some(resident)) =
            (self.desired.as_ref(), self.resident_identity.as_ref())
        else {
            return false;
        }; // 未采用首份快照不能显示草。
        if desired.map_id != resident.map_id
            || desired.root != resident.root
            || desired.manifest_revision != resident.manifest_revision
            || desired.live_revision != resident.live_revision
            || desired.crater_revision != resident.crater_revision
        {
            // 地图、文件、雕刻与技能代次严格相等。
            return false; // 不把旧内容重新解释成当前地形。
        } // 相同内容下只允许请求窗口不同。
        desired.dependencies.iter().all(|dependency| {
            // 防御同页同修订却不同hash的非法身份。
            resident
                .dependencies
                .binary_search_by_key(&dependency.page, |entry| entry.page)
                .map_or(true, |index| {
                    resident.dependencies[index].sha256 == dependency.sha256
                }) // 新页依赖缺席由局部掩码负责。
        }) // 各层最多四页的依赖数量受既有预算约束。
    } // 不要求整张请求页集合完全相同。

    fn blade_missing_pages(&self) -> [UVec4; 16] {
        self.blade_missing_pages_for(false)
    }

    fn blade_missing_pages_for(&self, retained_content: bool) -> [UVec4; 16] {
        // 每级四个非平缺页编码，GPU按实际采样Mip固定索引。
        let mut missing = [UVec4::ZERO; 16]; // 零编码保留给空槽，隐式平地不会误标缺页。
        let (Some(desired), Some(resident)) =
            (self.desired.as_ref(), self.resident_identity.as_ref())
        else {
            return missing;
        }; // 未就绪由独立ready门拦截。
        let mut counts = [0_usize; 16]; // 最大Map格式目前九级，固定十六级兼容几何上限。
        for page in &desired.pages {
            // 每层至多四页，与全图面积无关。
            if resident.pages.binary_search(page).is_ok() {
                continue;
            } // 已采用完整页始终可采样。
            if retained_content && !resident.has_live_pages {
                let has_formal_source = (0..=1).any(|z| (0..=1).any(|x| {
                    let neighbor = ResidentPageKey { level: page.level,
                        key: SparseHeightTileKey { x: page.key.x + x, z: page.key.z + z } };
                    desired.dependencies.binary_search_by_key(&neighbor, |dependency| dependency.page).is_ok()
                })); // 页的右/上共享边同样能让旧清单中的平页需要实际高度数据。
                if !has_formal_source { continue; } // 仅未来新编辑页：旧视觉版已证明此处隐式平地。
            }
            let level = usize::from(page.level); // 页级别独立于草叶LOD。
            let lane = counts[level]; // level_window_pages保证不会超过四个lane。
            missing[level][lane] = (page.key.x | (page.key.z << 16)) + 1; // 页坐标编码+1，(0,0)也不会与空槽冲突。
            counts[level] += 1; // 同层后续缺页写入下一槽。
        } // 新增页全部显式标记，未编辑的零页表仍合法表示Y=0。
        missing // 快照携带固定256字节，既有图集与页表零复制。
    } // 只抑制顶点需要缺页的局部候选，不全域熄灭。

    pub(in crate::maps::dynamic_map) fn resident_window_is_current(&self) -> bool {
        self.desired
            .as_ref()
            .is_some_and(|desired| self.resident_identity.as_ref() == Some(desired)) // 只有完整采用的同一窗口才能跳过后续页编码。
    }

    fn register_prepared_pages(
        &mut self,
        images: &mut Assets<Image>,
        uploads: Option<&mut WaterGpuUploadQueue>,
    ) -> Result<bool, String> {
        let built = self
            .prepared
            .as_ref()
            .ok_or("Clipmap GPU高度页尚未完整准备")?;
        if self.desired.as_ref() != Some(&built.identity) {
            return Err("Clipmap GPU高度页窗口身份已经变化".into());
        }
        let upload_bytes = built
            .height_bytes
            .len()
            .checked_add(built.page_table_bytes.len())
            .ok_or("Clipmap GPU高度页上传字节数溢出")?;
        if let Some(uploads) = uploads
            && !uploads.try_reserve_terrain_registration(upload_bytes)?
        {
            return Ok(false); // 水面或Mesh已占本帧额度，保留CPU候选下帧重试。
        }
        let built = self.prepared.take().expect("上方已确认完整高度页候选存在"); // 获得额度后才消费CPU字节。
        let cpu = Arc::clone(&built.cpu); // 只复制Arc，量化数据已经在后台冻结。
        let (identity, atlas) = register_page_images(built, images)?; // 先准备隐藏的完整GPU页组。
        if self.desired.as_ref() != Some(&identity) {
            return Err("Clipmap GPU高度页身份已经过期".into());
        } // 不覆盖新修订。
        let mut extension = self.extension.clone().ok_or("Clipmap高度材质尚未就绪")?;
        extension
            .set_atlas(atlas.clone())
            .map_err(|error| format!("Clipmap高度页候选绑定失败：{error:?}"))?;
        self.candidate = Some(RegisteredPageCandidate {
            identity,
            extension,
            atlas: Some(atlas),
            cpu: Some(cpu),
            registered_at: Instant::now(),
        }); // 正式扩展、图集与resident身份仍保持旧值。
        Ok(true)
    }

    fn discard_candidate(&mut self, images: &mut Assets<Image>) {
        if let Some(candidate) = self.candidate.take() {
            if let Some(atlas) = candidate.atlas {
                images.remove(atlas.heights.id());
                images.remove(atlas.page_table.id());
            }
            if let Some(cpu) = candidate.cpu {
                self.retired_cpu.push(cpu); // 由统一有界释放槽逐份归还，避免主线程析构或隐藏并发。
            }
        } // 当前正式atlas由self.atlas继续持有，不在失败路径误删。
    }
    pub(crate) fn spec_for(&self, map_id: &MapId) -> Option<&SparseHeightSpec> {
        (self.map_id.as_ref() == Some(map_id))
            .then_some(self.spec.as_ref())
            .flatten() // 地形清单准备前不接收技能破坏。
    }

    pub(super) fn raytracing_material(&self) -> Option<Handle<TerrainSurfaceMaterial>> {
        self.wrapped_source.clone() // 光追锚点始终绑定作者内层，实际显示租约另由 PostUpdate 同步。
    }

    /// 体积面只借用当前地图已经包装到可见Clipmap的内层，不接收预热中的地表候选。
    pub(crate) fn volume_surface_source(
        &self,
        map: &MapId,
        root: Entity,
    ) -> Option<Handle<TerrainSurfaceMaterial>> {
        (self.map_id.as_ref() == Some(map) && self.root == Some(root))
            .then(|| self.wrapped_source.clone())
            .flatten()
    }

    #[cfg(test)]
    pub(crate) fn adopt_volume_surface_for_test(
        &mut self,
        map: MapId,
        root: Entity,
        source: Handle<TerrainSurfaceMaterial>,
    ) {
        self.map_id = Some(map);
        self.root = Some(root);
        self.wrapped_source = Some(source);
    }

    pub(super) fn raytracing_io_available(&self) -> bool {
        self.pending_io.len() < MAX_PAGE_IO_TASKS // 代理最多从显示域借一个真实 IO 槽。
    }

    #[cfg(test)]
    pub(super) fn set_raytracing_material_for_test(
        &mut self,
        source: Handle<TerrainSurfaceMaterial>,
    ) {
        self.wrapped_source = Some(source);
        self.extension_revision += 1;
    }

    #[cfg(test)]
    pub(crate) fn adopt_raytracing_source_for_test(
        &mut self,
        root: Entity,
        handle: Handle<SparseHeightManifestAsset>,
        manifest: &SparseHeightManifest,
        source: Handle<TerrainSurfaceMaterial>,
        revision: u64,
        live: Option<u64>,
        crater: Option<u64>,
    ) {
        let lods = feathrise_map_content::default_display_lods_for_spec(&manifest.spec).unwrap();
        let identity = PageSetIdentity {
            map_id: manifest.map_id.clone(),
            root,
            manifest_revision: revision,
            live_revision: live,
            crater_revision: crater,
            has_live_pages: live.is_some() || crater.is_some(), // 测试未提供全图占用时保持保守，不能证明未知页原为平地。
            height_bounds: manifest.height_bounds(),
            display_plan: feathrise_map_content::compile_display_lod_plan(&manifest.spec, &lods).unwrap(),
            pages: Vec::new(),
            dependencies: Vec::new(),
        };
        self.map_id = Some(manifest.map_id.clone());
        self.root = Some(root);
        self.manifest = Some(handle);
        self.spec = Some(manifest.spec.clone());
        self.desired = Some(identity.clone());
        self.resident_identity = Some(identity);
        self.set_raytracing_material_for_test(source);
    }

    /// 权威 Mip 0 查询只能读取已经与正式显示共同采用的内容修订。
    pub(super) fn authority_source(
        &self,
    ) -> Option<(
        MapId,
        Handle<SparseHeightManifestAsset>,
        super::ground::ClipmapGroundContentRevision,
    )> {
        if !self.resident_content_is_current() {
            return None; // 作者雕刻或清单换代期间不能把未来局部页配给旧正式地形。
        }
        Some((
            self.map_id.clone()?,
            self.manifest.clone()?,
            self.ground_content_revision()?,
        ))
    }
    /// 换图只取消高度请求；外层资产保留到新内层到达后原地改写，避免孤立泄漏。
    fn begin_target(&mut self, map_id: MapId, root: Entity, path: String) {
        self.map_id = Some(map_id);
        self.root = Some(root);
        self.path = Some(path);
        self.manifest = None;
        self.manifest_revision = self.manifest_revision.wrapping_add(1).max(1);
        self.manifest_dirty = true;
        self.spec = None;
        self.extension = None;
        self.extension_revision = self.extension_revision.wrapping_add(1).max(1);
        self.desired = None;
        self.loading = None;
        // 在途任务继续占用唯一槽位，完成后按building身份拒收；调用方已把prepared交给后台释放。
        self.candidate = None; // 已登记但未采用图集的强句柄随候选一起释放。
        self.atlas = None;
        if let Some(cpu) = self.cpu.take() {
            self.retired_cpu.push(cpu); // 切图不允许复用旧只读页，但大Vec仍经唯一后台槽释放。
        }
        self.resident_identity = None; // 新地图不得继承旧GPU页修订。
        self.failed_identity = None;
        self.fallback_extension_revision = 0;
        self.wrapped_source = None;
        self.wrapped_extension_revision = 0;
    }
}

/// 非阻塞轮询唯一高度页CPU结果析构；归还前不得启动下一编码任务。
fn poll_page_set_release(pages: &mut SparseHeightPageRuntime) {
    let completed = pages
        .release_task
        .as_mut()
        .is_some_and(|task| check_ready(task).is_some());
    if completed {
        pages.release_task = None;
    }
}

/// 把已失效的完整高度上传字节交给单一后台槽位释放。
fn release_page_set_in_background(pages: &mut SparseHeightPageRuntime, built: BuiltPageSet) {
    debug_assert!(
        pages.release_task.is_none(),
        "页编码单飞和释放背压保证同一时刻最多一份待释放结果"
    );
    pages.release_task = Some(AsyncComputeTaskPool::get().spawn(async move {
        drop(built); // 两个Vec的合计容量受8MiB门限制，但析构仍不得进入帧关键路径。
    }));
}

/// 世界焦点转换为基础作者格坐标；窗口身份只在跨过实际所需页边界时变化。
fn focus_grid(spec: &SparseHeightSpec, focus: Vec2) -> [u64; 2] {
    let logical_x = ((f64::from(focus.x) - f64::from(spec.world_min[0]))
        / spec.actual_spacing_meters[0])
        .floor()
        .clamp(0.0, spec.cells[0] as f64) as u64;
    let logical_z = ((f64::from(focus.y) - f64::from(spec.world_min[1]))
        / spec.actual_spacing_meters[1])
        .floor()
        .clamp(0.0, spec.cells[1] as f64) as u64;
    [logical_x, logical_z]
}

/// 查询一个层级页在当前清单中的正式内容地址；缺席表示整页为隐式平地。
fn manifest_page_hash(manifest: &SparseHeightManifest, page: ResidentPageKey) -> Option<&str> {
    if page.level == 0 {
        manifest
            .tile_ref(page.key)
            .map(|entry| entry.sha256.as_str())
    } else {
        manifest
            .mip_tile_ref(page.level, page.key)
            .map(|entry| entry.sha256.as_str())
    }
}

/// 当前采样Mip按真实可见环带和固定64格保护带选择矩形页窗口。
fn level_window_pages(
    spec: &SparseHeightSpec,
    focus: [u64; 2],
    level: u8,
    visible_half_cells: [u64; 2], // 编译计划导出的当前Mip两轴可见半径。
) -> Result<Vec<SparseHeightTileKey>, String> {
    let cells = spec.mip_cells(level).ok_or("Clipmap 请求了无效高度层级")?;
    let counts = spec
        .mip_tile_counts(level)
        .ok_or("Clipmap 高度层级缺少页数量")?;
    let scale = 1_u64 << level;
    let center = [focus[0] / scale, focus[1] / scale];
    let radius = [
        visible_half_cells[0]
            .checked_add(DISPLAY_LOD_RESIDENCY_GUARD_CELLS)
            .ok_or("Clipmap显示LOD X驻留范围溢出")?,
        visible_half_cells[1]
            .checked_add(DISPLAY_LOD_RESIDENCY_GUARD_CELLS)
            .ok_or("Clipmap显示LOD Z驻留范围溢出")?,
    ]; // 保护带包含可见边缘法线邻点，碰撞范围不读取该值。
    let minimum = [
        center[0].saturating_sub(radius[0]),
        center[1].saturating_sub(radius[1]),
    ];
    let maximum = [
        center[0].saturating_add(radius[0]).min(cells[0]),
        center[1].saturating_add(radius[1]).min(cells[1]),
    ];
    let page_min = [
        (minimum[0] / u64::from(CLIPMAP_TILE_CELLS)).min(u64::from(counts[0] - 1)) as u32,
        (minimum[1] / u64::from(CLIPMAP_TILE_CELLS)).min(u64::from(counts[1] - 1)) as u32,
    ];
    let page_max = [
        (maximum[0] / u64::from(CLIPMAP_TILE_CELLS)).min(u64::from(counts[0] - 1)) as u32,
        (maximum[1] / u64::from(CLIPMAP_TILE_CELLS)).min(u64::from(counts[1] - 1)) as u32,
    ];
    let page_capacity = u64::from(page_max[0] - page_min[0] + 1)
        .checked_mul(u64::from(page_max[1] - page_min[1] + 1))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or("Clipmap显示LOD页窗口数量无法由当前平台表达")?; // 保存预检已在8MiB门前限制该值。
    let mut pages = Vec::new(); // 主线程只枚举受检小页身份，不读取像素。
    pages
        .try_reserve_exact(page_capacity)
        .map_err(|_| "Clipmap显示LOD页窗口身份分配失败")?; // 运行时内存压力显式失败并保留旧页集。
    for z in page_min[1]..=page_max[1] {
        for x in page_min[0]..=page_max[0] {
            pages.push(SparseHeightTileKey { x, z });
        }
    }
    Ok(pages)
}

/// 冻结所有真实环带范围的页身份和正式依赖；页数只受8MiB保存预检预算约束。
fn desired_identity(
    manifest: &SparseHeightManifest,
    root: Entity,
    revision: u64,
    focus: [u64; 2],
    display_plan: &CompiledDisplayLodPlan, // 与候选Mesh完全相同的编译计划。
    live: Option<&SparseHeightLiveOverride>,
    craters: Option<&SparseHeightSessionCrater>, // 普通技能只在当前会话改变局部格点。
) -> Result<PageSetIdentity, String> {
    let live = live.filter(|live| live.map_id() == Some(&manifest.map_id));
    // 作者块数不设上限，祖先占用由发布侧增量维护；本帧只查受检窗口内的最多四个共享边邻点。
    let mut crater_ancestors = BTreeSet::new();
    let craters = craters.filter(|craters| {
        craters.map_id() == Some(&manifest.map_id) && !craters.tiles().is_empty()
    }); // 空会话沿用原有平地快速路径。
    if let Some(craters) = craters {
        for base_key in craters.tiles().keys() {
            for level in 0..=manifest.spec.max_mip_level() {
                if let Some(key) = manifest.spec.mip_ancestor_key(*base_key, level) {
                    crater_ancestors.insert(ResidentPageKey { level, key }); // 浅坑至多256块，固定上界；不跟随作者全图规模增长。
                }
            }
        }
    }
    let mut pages = BTreeSet::new();
    let mut dependencies = BTreeMap::<ResidentPageKey, PageDependency>::new();
    for range in &display_plan.sample_ranges {
        let level = range.mip_level; // 只为真实被几何或接缝采样的持久Mip选页。
        let counts = manifest
            .spec
            .mip_tile_counts(level)
            .ok_or("Clipmap 高度清单层级页数无效")?;
        for key in level_window_pages(&manifest.spec, focus, level, range.half_cells)? {
            let page = ResidentPageKey { level, key };
            let mut needed = false;
            for dz in 0..=1 {
                for dx in 0..=1 {
                    let neighbor = ResidentPageKey {
                        level,
                        key: SparseHeightTileKey {
                            x: key.x + dx,
                            z: key.z + dz,
                        },
                    };
                    if neighbor.key.x >= counts[0] || neighbor.key.z >= counts[1] {
                        continue;
                    }
                    needed |= live.is_some_and(|live| live.has_mip_tiles(level, neighbor.key))
                        || crater_ancestors.contains(&neighbor);
                    if let Some(sha256) = manifest_page_hash(manifest, neighbor) {
                        needed = true;
                        dependencies
                            .entry(neighbor)
                            .or_insert_with(|| PageDependency {
                                page: neighbor,
                                sha256: sha256.to_owned(),
                            });
                    }
                }
            }
            if needed {
                pages.insert(page); // 平地页只有在边界邻页或实时覆盖需要插值时才上传。
            }
        }
    }
    let mut height_bounds = manifest.height_bounds();
    if let Some(bounds) = live.and_then(SparseHeightLiveOverride::height_bounds) {
        height_bounds = [height_bounds[0].min(bounds[0]), height_bounds[1].max(bounds[1])];
    }
    if let Some(craters) = craters {
        height_bounds[0] = (f64::from(height_bounds[0]) - f64::from(craters.depth_bound()))
            .max(-f64::from(f32::MAX)) as f32;
    } // 只读取持久根缓存的标量，不在输入帧枚举作者全图；实际页界仍由worker核实。
    Ok(PageSetIdentity {
        map_id: manifest.map_id.clone(),
        root,
        manifest_revision: revision,
        live_revision: live.map(SparseHeightLiveOverride::revision),
        crater_revision: craters.map(SparseHeightSessionCrater::revision), // 每次施放独立失效旧任务。
        has_live_pages: live.is_some_and(|live| live.tile_count() > 0) || !crater_ancestors.is_empty(),
        height_bounds,
        display_plan: display_plan.clone(), // 页候选不得与旧范围Mesh混合采用。
        pages: pages.into_iter().collect(),
        dependencies: dependencies.into_values().collect(),
    })
}

/// 在原页编码worker内从持久快照收集实际相交块；主帧捕获根Arc，不枚举作者全图。
fn live_tiles_for_pages(
    spec: &SparseHeightSpec,
    identity: &PageSetIdentity,
    live: &SparseHeightLiveSnapshot,
) -> Result<BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>, String> {
    if identity.live_revision.is_none() {
        return Ok(BTreeMap::new()); // 当前来源没有声明实时覆盖，全部依赖正式清单。
    }
    if live.map_id() != Some(&identity.map_id) || Some(live.revision()) != identity.live_revision {
        return Err("编辑器实时高度快照的地图或修订已变化".to_owned()); // 不能把错误代次的空集当作隐式平地。
    }
    let mut sampled_ancestors = BTreeSet::new();
    for page in &identity.pages {
        let counts = spec
            .mip_tile_counts(page.level)
            .ok_or("编辑器实时高度请求包含无效 Mip 层级")?;
        for dz in 0..=1 {
            for dx in 0..=1 {
                let key = SparseHeightTileKey {
                    x: page.key.x + dx,
                    z: page.key.z + dz,
                };
                if key.x < counts[0] && key.z < counts[1] {
                    sampled_ancestors.insert(ResidentPageKey {
                        level: page.level,
                        key,
                    });
                }
            }
        }
    }
    let mut tiles = BTreeMap::new();
    let mut invalid = false;
    for page in sampled_ancestors {
        live.visit_mip_tiles(page.level, page.key, |key, tile| {
            if tile.key != key || spec.tile_shape(key) != Some(tile.shape) {
                invalid = true;
                return;
            }
            tiles.entry(key).or_insert_with(|| Arc::clone(tile)); // 多级/共享边重叠只保留同一份不可变块句柄。
        });
    }
    if invalid { return Err("编辑器稀疏高度实时块身份或形状与当前地图不一致".to_owned()); }
    Ok(tiles)
}

/// 已有页worker的完整入口；持久根、选中块和编码临时值均在本worker内释放。
fn build_page_set_with_live(
    identity: PageSetIdentity,
    spec: SparseHeightSpec,
    formal_pages: BTreeMap<ResidentPageKey, Arc<SparseHeightTile>>,
    live: SparseHeightLiveSnapshot,
    crater_tiles: SparseCraterTiles,
) -> Result<BuiltPageSet, String> {
    let live_tiles = live_tiles_for_pages(&spec, &identity, &live)?;
    drop(live); // 完整旧索引不得随结果回到主线程；错误路径同样在worker归还快照。
    build_page_set(identity, spec, formal_pages, live_tiles, crater_tiles)
}

/// 读取一个层级格点；未保存基础页先覆盖正式父页，缺页严格解释为世界 Y=0。
fn sample_page_grid(
    spec: &SparseHeightSpec,
    formal_pages: &BTreeMap<ResidentPageKey, Arc<SparseHeightTile>>,
    live_tiles: &BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>,
    crater_tiles: &SparseCraterTiles, // 和碰撞取同一会话笔印快照。
    level: u8,
    x: u64,
    z: u64,
) -> Result<f32, String> {
    let base = spec
        .mip_grid_point_to_base(level, x, z)
        .ok_or("GPU 高度页采样格点超出当前层地图")?;
    let (base_key, base_local) = spec
        .tile_for_grid_point(base[0], base[1])
        .ok_or("GPU 高度页无法映射回基础作者格点")?;
    let base_height = if let Some(tile) = live_tiles.get(&base_key) {
        tile.get_height(spec, base_local)
            .map(|code| spec.decode_height(code))
            .ok_or_else(|| "GPU 实时高度页块内坐标与清单形状不一致".to_owned())?
    } else {
        let (key, local) = spec
            .mip_tile_for_grid_point(level, x, z)
            .ok_or("GPU 高度页采样格点超出当前层页表")?;
        formal_pages
            .get(&ResidentPageKey { level, key })
            .map_or(Ok(0.0), |tile| {
                tile.get_height(spec, local)
                    .map(|code| spec.decode_height(code))
                    .ok_or_else(|| "GPU 高度父页块内坐标与清单形状不一致".to_owned())
            })?
    }; // 优先正式作者高度，再叠加独立会话层。
    Ok(sample_crater_height(spec, crater_tiles, base, base_height)) // 块边与物理读取相同世界格点。
}

/// 计算紧凑多层页表布局；每层只保留自身实际行数，不复制 256² 空表。
fn page_table_layout(spec: &SparseHeightSpec) -> Result<(UVec2, Vec<u32>), String> {
    let width = spec.tile_counts()[0];
    let mut offsets = Vec::with_capacity(usize::from(spec.max_mip_level()) + 1);
    let mut height = 0_u32;
    for level in 0..=spec.max_mip_level() {
        offsets.push(height);
        let rows = spec
            .mip_tile_counts(level)
            .ok_or("GPU 高度页表层级超出地图规格")?[1];
        height = height.checked_add(rows).ok_or("GPU 高度页表总行数溢出")?;
    }
    Ok((UVec2::new(width, height), offsets))
}

/// 在后台把冻结的全层窗口编码为固定层数组和紧凑整数页表。
fn build_page_set(
    identity: PageSetIdentity,
    spec: SparseHeightSpec,
    formal_pages: BTreeMap<ResidentPageKey, Arc<SparseHeightTile>>,
    live_tiles: BTreeMap<SparseHeightTileKey, Arc<SparseHeightTile>>,
    crater_tiles: SparseCraterTiles, // 和碰撞构建读取同一会话修订。
) -> Result<BuiltPageSet, String> {
    encode_page_table_entry(0, identity.height_bounds[0], identity.height_bounds[1])
        .map_err(|error| format!("GPU 高度源范围无法有限表达：{error:?}"))?;
    let (table_size, table_offsets) = page_table_layout(&spec)?;
    let table_texels = usize::try_from(u64::from(table_size.x) * u64::from(table_size.y))
        .map_err(|_| "GPU 高度页表尺寸无法由当前平台表达")?;
    let mut page_table_bytes = vec![0_u8; table_texels * 16];
    let actual_layers = identity.pages.len();
    let layer_count = actual_layers.max(2) as u32; // 回退与正式数组都使用合法非空 array 绑定。
    let points_per_page = usize::try_from(u64::from(PAGE_POINTS) * u64::from(PAGE_POINTS))
        .expect("257² 在所有支持平台均可表达");
    let mut height_bytes = Vec::with_capacity(points_per_page * 2 * layer_count as usize);
    let mut decoded = Vec::with_capacity(points_per_page);
    let mut height_bounds = [0.0_f32; 2]; // 隐式平地也是当前地形的一部分。
    for (layer, page) in identity.pages.iter().enumerate() {
        decoded.clear();
        let level_cells = spec.mip_cells(page.level).ok_or("GPU 高度页包含无效层级")?;
        if spec.mip_tile_shape(page.level, page.key).is_none() {
            return Err("GPU 高度页坐标超出当前层地图".to_owned());
        }
        let start_x = u64::from(page.key.x) * u64::from(CLIPMAP_TILE_CELLS);
        let start_z = u64::from(page.key.z) * u64::from(CLIPMAP_TILE_CELLS);
        for local_z in 0..PAGE_POINTS {
            for local_x in 0..PAGE_POINTS {
                // GPU 数组每层固定为257²；有限地图和不整除的父层尾页用最外侧格点
                // 填满剩余 texel。Shader只会访问真实范围，复制边缘同时避免过滤读到未定义值。
                let sample_x = (start_x + u64::from(local_x)).min(level_cells[0]);
                let sample_z = (start_z + u64::from(local_z)).min(level_cells[1]);
                decoded.push(sample_page_grid(
                    &spec,
                    &formal_pages,
                    &live_tiles,
                    &crater_tiles,
                    page.level,
                    sample_x,
                    sample_z,
                )?);
            }
        }
        let min_height = decoded.iter().copied().fold(f32::INFINITY, f32::min);
        let max_height = decoded.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let span = max_height - min_height;
        let entry = encode_page_table_entry(layer as u32, min_height, max_height)
            .map_err(|error| format!("GPU 高度页范围无效：{error:?}"))?;
        height_bounds = [height_bounds[0].min(min_height), height_bounds[1].max(max_height)];
        for height in &decoded {
            let encoded = if span <= f32::EPSILON {
                0_u16
            } else {
                (((*height - min_height) / span).clamp(0.0, 1.0) * 65535.0).round() as u16
            };
            height_bytes.extend_from_slice(&encoded.to_le_bytes());
        }
        let table_y = table_offsets[usize::from(page.level)] + page.key.z;
        let table_index = (table_y as usize * table_size.x as usize + page.key.x as usize) * 16;
        for (offset, value) in entry.into_iter().enumerate() {
            page_table_bytes[table_index + offset * 4..table_index + offset * 4 + 4]
                .copy_from_slice(&value.to_le_bytes());
        }
    }
    height_bytes.resize(points_per_page * 2 * layer_count as usize, 0); // 少于两页时追加空层。
    encode_page_table_entry(0, identity.height_bounds[0].min(height_bounds[0]),
        identity.height_bounds[1].max(height_bounds[1]))
        .map_err(|error| format!("GPU 完整高度页范围无法有限表达：{error:?}"))?;
    if height_bytes.len() + page_table_bytes.len() > MAX_PAGE_UPLOAD_BYTES {
        return Err("GPU 高度页窗口超过单次 8 MiB 上传预算".to_owned());
    }
    let max_mip = u32::from(spec.max_mip_level()); // CPU/GPU采样上限冻结成相同标量。
    let cpu = Arc::new(super::ground::GroundPageBytes {
        heights: height_bytes.clone(),
        table: page_table_bytes.clone(),
        width: table_size.x,
        height_bounds,
    }); // 后台一次复制小于8MiB量化页，不在采用帧复制。
    Ok(BuiltPageSet {
        cpu, // CPU查询输入与GPU上传字节完全一致。
        identity,
        height_bytes,
        layer_count,
        page_table_bytes,
        table_size,
        max_mip,
    })
}

/// 把完整 CPU 页组登记为两份 GPU Image；两句柄随后由材质一次切换。
fn register_page_images(
    built: BuiltPageSet,
    images: &mut Assets<Image>,
) -> Result<(PageSetIdentity, ClipmapHeightAtlas), String> {
    let mut heights = Image::new(
        Extent3d {
            width: PAGE_POINTS,
            height: PAGE_POINTS * built.layer_count,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        built.height_bytes,
        TextureFormat::R16Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    heights
        .reinterpret_stacked_2d_as_array(built.layer_count)
        .map_err(|error| format!("建立 GPU 高度数组失败：{error}"))?;
    heights.sampler = ImageSampler::linear();
    let page_table = Image::new(
        Extent3d {
            width: built.table_size.x,
            height: built.table_size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        built.page_table_bytes,
        TextureFormat::Rgba32Uint,
        RenderAssetUsages::RENDER_WORLD,
    );
    Ok((
        built.identity,
        ClipmapHeightAtlas {
            heights: images.add(heights),
            page_table: images.add(page_table),
            max_mip: built.max_mip, // Shader 与紧凑页表共同钳制到已持久化的最高父页。
        },
    ))
}

/// 高度分页域共享的资产访问。它只合并 ECS 借用，不改变网格域、分页域各自的任务和采用状态。
#[derive(SystemParam)]
pub(crate) struct SparsePageAssets<'w> {
    manifests: Res<'w, Assets<SparseHeightManifestAsset>>,
    tiles: Res<'w, Assets<SparseHeightTileAsset>>,
    fallback: Res<'w, ClipmapFlatHeightPage>,
    images: ResMut<'w, Assets<Image>>,
    uploads: Option<ResMut<'w, WaterGpuUploadQueue>>,
}

/// 观察清单、请求窗口块并准备完整GPU页组；真正发布由共享碰撞屏障完成。
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_sparse_height_pages(
    asset_server: Res<AssetServer>,
    active_map: InstanceRes<ActiveDynamicMap>,
    live: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<SparseHeightLiveOverride>,
    craters: InstanceRes<SparseHeightSessionCrater>, // 技能浅坑绝不覆盖F12草稿本体。
    mut manifest_events: feathrise_scene_core::residency::ResidentAssetEventReader<SparseHeightManifestAsset>,
    assets: SparsePageAssets, // 候选图集登记与共享大上传额度属于同一个分页资产域。
    runtime: InstanceRes<DynamicMapRuntimeState>,
    mut status: InstanceResMut<DynamicMapRuntimeStatus>,
    clipmap_runtime: InstanceRes<SparseClipmapRuntime>, // 读取网格域最新计划，不共享任务所有权。
    mut display_status: InstanceResMut<MapTerrainDisplayLodStatus>, // 发布页候选阶段与正式页数。
    mut pages: InstanceResMut<SparseHeightPageRuntime>,
    displays: InstanceQuery<&Transform, With<SparseClipmapDisplay>>, // 不再次读取可能在同帧改变的F12相机焦点。
    raytracing: Option<Res<super::ClipmapRaytracingRuntime>>, // 禁用/未装配时显示仍可使用原有三个 IO 槽。
) {
    pages.pending_io.retain(|handle| {
        matches!(
            asset_server.load_state(handle.id()),
            LoadState::NotLoaded | LoadState::Loading
        )
    }); // 仅真实完成/失败才释放IO槽。
    let SparsePageAssets {
        manifests,
        tiles,
        fallback,
        mut images,
        mut uploads,
    } = assets;
    poll_page_set_release(&mut pages); // 清单未就绪时也持续归还有界释放槽位。
    let (Some(map_id), Some(root), Some(path)) = (
        active_map.map_id.as_ref(),
        runtime.root,
        runtime.sparse_height_path.clone(),
    ) else {
        return;
    };
    if pages.map_id.as_ref() != Some(map_id)
        || pages.root != Some(root)
        || pages.path.as_deref() != Some(path.as_str())
    {
        pages.discard_candidate(&mut images); // 换图不得把旧候选图片留在Assets等待误用。
        if let Some(prepared) = pages.prepared.take() {
            release_page_set_in_background(&mut pages, prepared); // 换图不在主线程析构完整上传缓冲。
        }
        pages.begin_target(map_id.clone(), root, path.clone());
        pages.manifest = Some(asset_server.load(path));
    }
    let Some(manifest_handle) = pages.manifest.clone() else {
        return;
    };
    if manifest_events.read().any(|event| match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id } => *id == manifest_handle.id(),
        AssetEvent::Unused { .. } => false,
    }) {
        pages.manifest_revision = pages.manifest_revision.wrapping_add(1).max(1);
        pages.manifest_dirty = true;
        pages.loading = None;
    }
    if matches!(
        asset_server.load_state(manifest_handle.id()),
        LoadState::Failed(_)
    ) {
        status.phase = DynamicMapRuntimePhase::Failed;
        status.detail = format!("地图{map_id}的 Clipmap 高度清单加载失败");
        return;
    }
    let Some(manifest_asset) = manifests.get(&manifest_handle) else {
        return;
    };
    let manifest = &manifest_asset.manifest;
    if pages
        .spec
        .as_ref()
        .is_some_and(|current| current != &manifest.spec)
    {
        status.detail =
            format!("地图{map_id}的Clipmap格距或边界热更需要重新载入地图，保留上一完整地形");
        return; // 规格变化不得把新页表绑定到旧拓扑/Collider。
    }
    if pages.manifest_dirty {
        if manifest.map_id != *map_id || manifest.validate().is_err() {
            status.phase = DynamicMapRuntimePhase::Failed;
            status.detail = format!("地图{map_id}的 Clipmap 高度清单身份或格式无效");
            pages.manifest_dirty = false;
            return;
        }
        if pages.spec.as_ref() != Some(&manifest.spec) {
            let bounds = match ClipmapBounds::new(
                Vec2::from_array(manifest.spec.world_min),
                Vec2::from_array(manifest.spec.world_max),
            ) {
                Ok(bounds) => bounds,
                Err(error) => {
                    status.phase = DynamicMapRuntimePhase::Failed;
                    status.detail = format!("地图{map_id}的 Clipmap 世界边界无效：{error:?}");
                    return;
                }
            };
            let grid_cells =
                UVec2::new(manifest.spec.cells[0] as u32, manifest.spec.cells[1] as u32);
            match ClipmapHeightExtension::new_flat(bounds, grid_cells, &fallback) {
                Ok(mut extension) => {
                    extension.params.stitches=*pages.stitches;
                    pages.extension = Some(extension);
                    pages.spec = Some(manifest.spec.clone());
                    pages.atlas = None;
                    pages.extension_revision = pages.extension_revision.wrapping_add(1).max(1);
                }
                Err(error) => {
                    status.phase = DynamicMapRuntimePhase::Failed;
                    status.detail = format!("地图{map_id}的 Clipmap 高度参数无效：{error:?}");
                    return;
                }
            }
        }
        pages.manifest_dirty = false;
    }
    let Some(spec) = pages.spec.clone() else {
        return;
    };
    let Some(display_plan) = clipmap_runtime.desired_display_plan(map_id, root) else {
        return; // 网格域未完成清单和预算预检前不得按旧固定窗口先行构建。
    };
    let Ok(display_transform) = displays.single() else {
        return; // 组合Mesh尚未登记或旧根已离场时非阻塞重试。
    };
    let focus = clipmap_runtime.requested_focus.unwrap_or(display_transform.translation.xz()); // 候选先准备新视图页，完整采用后显示才移动。
    let desired = match desired_identity(
        manifest,
        root,
        pages.manifest_revision,
        focus_grid(&spec, focus),
        display_plan,
        Some(&live),
        Some(&craters),
    ) {
        Ok(desired) => desired,
        Err(error) => {
            status.phase = DynamicMapRuntimePhase::Failed;
            status.detail = format!("地图{map_id}规划 Clipmap 分层高度页失败：{error}");
            return;
        }
    };
    if pages.desired.as_ref() != Some(&desired) {
        pages.discard_candidate(&mut images); // 传送、保存或高度修订淘汰旧页候选，正式页集保持不变。
        pages.desired = Some(desired.clone());
        pages.failed_identity = None; // 新焦点、破坏或显示配置身份允许重试。
        pages.loading = None;
        if let Some(prepared) = pages.prepared.take() {
            release_page_set_in_background(&mut pages, prepared); // 新视图或新破坏修订使旧待提交页失效。
        }
        display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::LoadingHeightPages;
        display_status.detail = "显示LOD正在加载真实环带所需高度页".into(); // 旧页组仍保留为正式画面。
    }
    if desired.pages.is_empty()
        && !pages.resident_window_is_current()
        && pages
            .candidate
            .as_ref()
            .is_none_or(|candidate| candidate.identity != desired)
        && let Some(mut extension) = pages.extension.clone()
    {
        extension.clear_atlas(&fallback);
        pages.candidate = Some(RegisteredPageCandidate {
            identity: desired.clone(),
            extension,
            atlas: None,
            cpu: None,
            registered_at: Instant::now(),
        }); // 全平页也等待匹配Mesh与候选材质预热，不直接改写正式扩展。
        display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Uploading;
        display_status.detail = "显示LOD全平高度候选已准备，等待GPU预热".into();
    }
    if let Some(task) = pages.task.as_mut()
        && let Some(result) = check_ready(task)
    {
        pages.task = None;
        let building = pages.building.take(); // 错误结果同样必须与启动时身份绑定。
        match result {
            Ok(built) if pages.desired.as_ref() == Some(&built.identity) => {
                pages.prepared = Some(built); // 完整页组先验证身份，再一次切换材质。
                display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Uploading;
                display_status.detail = "显示LOD高度页已编码，等待共享GPU登记额度".into();
            }
            Ok(built) => {
                release_page_set_in_background(&mut pages, built); // 迟到窗口结果不得替换当前相机请求或在主线程析构。
            }
            Err(error) => {
                if building.as_ref() == pages.desired.as_ref() {
                    if pages.atlas.is_none()
                        && pages
                            .desired
                            .as_ref()
                            .is_some_and(|desired| !desired.pages.is_empty())
                    {
                        status.phase = DynamicMapRuntimePhase::Failed;
                    }
                    status.detail = format!("地图{map_id}构建 Clipmap 高度页失败：{error}");
                    display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Failed;
                    display_status.detail = format!("{error}；保留上一完整显示");
                    pages.failed_identity = building; // 同一完整窗口等待新身份再试。
                }
            } // 迟到旧任务错误不污染新目标诊断或失败身份。
        }
    }
    if pages.candidate.is_none() && pages.prepared.is_some() {
        let upload_bytes = pages.prepared.as_ref().map_or(0, |built|
            (built.height_bytes.len() as u64).saturating_add(built.page_table_bytes.len() as u64));
        match pages.register_prepared_pages(&mut images, uploads.as_deref_mut()) {
            Ok(true) => {
                display_status.work.page_uploads = display_status.work.page_uploads.saturating_add(1);
                display_status.work.page_upload_bytes = display_status.work.page_upload_bytes.saturating_add(upload_bytes);
                display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Uploading;
                display_status.detail = "显示LOD高度图集已登记，等待匹配Mesh与材质".into();
            }
            Ok(false) => {} // 本帧额度被水面或地形Mesh占用，完整CPU字节保留到下帧。
            Err(error) => {
                status.detail = format!("地图{map_id}登记Clipmap高度页失败：{error}");
                display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Failed;
                display_status.detail = format!("{error}；保留上一完整显示");
                if let Some(prepared) = pages.prepared.take() {
                    release_page_set_in_background(&mut pages, prepared); // 登记前错误不在主线程释放大缓冲。
                }
                pages.failed_identity = pages.desired.clone();
            }
        }
    }
    if pages.task.is_some()
        || pages.release_task.is_some() // 迟到结果析构完毕前不启动下一大编码，保持CPU峰值有界。
        || pages.prepared.as_ref().is_some_and(|built| pages.desired.as_ref() == Some(&built.identity)) // 完整页候选尚未采用时不重复编码。
        || pages.candidate.as_ref().is_some_and(|candidate| pages.desired.as_ref() == Some(&candidate.identity)) // 已登记GPU候选等待联合预热时不重复加载。
        || pages.resident_window_is_current() // 当前窗口已采用时不再每帧重新编码同一页组。
        || pages.desired == pages.failed_identity // 明确失败的同一身份不在稳定帧无限重试。
        || pages
            .desired
            .as_ref()
            .is_none_or(|desired| desired.pages.is_empty())
    {
        return;
    }
    let desired = pages
        .desired
        .clone()
        .expect("非空 desired 已由前置条件确认");
    if pages
        .loading
        .as_ref()
        .is_none_or(|loading| loading.identity != desired)
    {
        pages.loading = Some(PageLoadSet {
            identity: desired.clone(),
            handles: Vec::new(), // 只建立需求描述，下面逐帧按真实空闲IO槽提出请求。
            crater_tiles: if craters.map_id() == Some(&desired.map_id) {
                craters.tiles().clone()
            } else {
                BTreeMap::new()
            }, // 只Arc浅克隆当前地图会话。
        });
    }
    let page_io_limit = MAX_PAGE_IO_TASKS
        - usize::from(raytracing.as_ref().is_some_and(|rt| rt.occupies_io_slot()));
    while pages.pending_io.len() < page_io_limit {
        // 整个显示页域最多三个未完成加载，旧窗口也计入。
        let Some(loading) = pages.loading.as_ref() else {
            break;
        }; // 无需求即停止。
        let Some(reference) = loading
            .identity
            .dependencies
            .get(loading.handles.len())
            .cloned()
        else {
            break;
        }; // 每个依赖只请求一次。
        let path = if reference.page.level == 0 {
            // 内容地址路径不变。
            manifest.tile_asset_path(reference.page.key, &reference.sha256) // 基础页。
        } else {
            // 父高度页。
            manifest.mip_tile_asset_path(reference.page.level, reference.page.key, &reference.sha256) // 分页层级。
        }; // 请求仅由受检清单构造。
        let handle = asset_server.load(path); // AssetServer负责共享同一内容地址的实际IO。
        pages.pending_io.push(handle.clone()); // 即使换需求也保留这个真实在途槽。
        pages
            .loading
            .as_mut()
            .expect("加载集合已存在")
            .handles
            .push((reference, handle)); // 保存请求顺序用于下帧继续。
    } // 不在主线程等待IO。
    let Some(loading) = pages.loading.as_ref() else {
        return;
    };
    if loading
        .handles
        .iter()
        .any(|(_, handle)| matches!(asset_server.load_state(handle.id()), LoadState::Failed(_)))
    {
        if pages.atlas.is_none() {
            status.phase = DynamicMapRuntimePhase::Failed;
        }
        status.detail = format!("地图{map_id}的当前 Clipmap 高度块加载失败，保留上一完整页组");
        display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Failed;
        display_status.detail = "显示LOD高度页加载失败；保留上一完整显示".into();
        pages.failed_identity = pages.desired.clone(); // 同一坏依赖不在稳定帧重复建立句柄集合。
        pages.loading = None;
        return;
    }
    if loading.handles.len() != loading.identity.dependencies.len() || !pages.pending_io.is_empty()
    {
        return;
    } // 所有依赖已提出且旧IO真实结束后才允许编码。
    if !loading
        .handles
        .iter()
        .all(|(_, handle)| asset_server.is_loaded_with_dependencies(handle.id()))
    {
        return;
    }
    let mut ready_pages = BTreeMap::new(); // 正式基础页和父页按完整层级身份冻结。
    for (reference, handle) in &loading.handles {
        let Some(asset) = tiles.get(handle) else {
            return;
        };
        if let Err(error) = asset.validate_page_against(
            manifest,
            reference.page.level,
            reference.page.key,
            &reference.sha256,
        ) {
            status.detail = format!("地图{map_id}的当前 Clipmap 高度块版本无效：{error}");
            display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::Failed;
            display_status.detail = format!("{error}；保留上一完整显示");
            pages.failed_identity = pages.desired.clone(); // 等待清单、显示配置或焦点身份变化。
            pages.loading = None;
            return;
        }
        ready_pages.insert(reference.page, asset.tile.clone());
    }
    let identity = loading.identity.clone();
    let crater_tiles = loading.crater_tiles.clone(); // 本次页编码保留同一破坏修订的不可变笔印。
    let spec = manifest.spec.clone();
    let live = live.snapshot(); // 常数捕获完整持久根；加载等待不持有大索引，取消IO也不引入主帧全树析构。
    pages.loading = None;
    pages.building = Some(identity.clone()); // 先占身份槽，再把唯一任务交给计算池。
    pages.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        build_page_set_with_live(identity, spec, ready_pages, live, crater_tiles)
    }));
    display_status.phase = feathrise_map_content::MapTerrainDisplayLodPhase::EncodingHeightPages;
    display_status.detail = "显示LOD正在后台编码完整高度图集".into(); // 主线程后续帧只非阻塞轮询。
}

/// 始终安装带高度位移的材质；地表PBR尚未完成时先使用基础PBR，完成后原地升级。
pub(crate) fn sync_sparse_clipmap_surface(
    mut commands: Commands,
    base_materials: Res<Assets<StandardMaterial>>,
    terrain_materials: Res<Assets<TerrainSurfaceMaterial>>,
    mut fallback_materials: ResMut<Assets<ClipmapHeightMaterial>>,
    mut clipmap_materials: ResMut<Assets<SparseClipmapSurfaceMaterial>>,
    mut terrain_events: feathrise_scene_core::residency::ResidentAssetEventReader<TerrainSurfaceMaterial>,
    mut pages: InstanceResMut<SparseHeightPageRuntime>,
    displays: InstanceQuery<
        (
            Entity,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
            Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
            Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
        ),
        With<DynamicSparseClipmapPrimitive>,
    >,
) {
    let source_modified = pages.wrapped_source.as_ref().is_some_and(|source| {
        terrain_events.read().any(|event| match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::LoadedWithDependencies { id } => *id == source.id(),
            AssetEvent::Unused { .. } => false,
        })
    });
    if pages.wrapped_source.is_none() {
        terrain_events.read().for_each(drop); // 未包装时仍消费本 reader，避免后续旧事件误触发。
    }
    let Ok((entity, direct_standard, direct_surface, installed_fallback, installed_clipmap)) =
        displays.single()
    else {
        return;
    };
    let Some(extension) = pages.extension.clone() else {
        return;
    };
    let source = direct_surface.map(|surface| surface.0.clone()).or_else(|| {
        installed_clipmap
            .is_some()
            .then(|| pages.wrapped_source.clone())
            .flatten()
    }); // 只有实体仍安装正式外层时，才允许沿用被包装的内层句柄。
    if let Some(source) = source
        && let Some(mut base) = terrain_materials.get(&source).cloned()
    {
        let extension_changed = pages.wrapped_extension_revision != pages.extension_revision;
        let source_changed = pages.wrapped_source.as_ref().map(Handle::id) != Some(source.id());
        let installed_matches = installed_clipmap.is_some_and(|installed| {
            pages
                .wrapped_material
                .as_ref()
                .is_some_and(|retained| installed.0.id() == retained.id())
        });
        if !source_modified && !extension_changed && !source_changed && installed_matches {
            return;
        }
        // 新作者材质的默认矩阵是单位阵；包装时即与高度页绑定同一源框架，避免采用帧漏用缩放。
        base.extension.set_source_frame(extension.params.source_to_world);
        let fallback_base = base.base.clone(); // 即使完整地表先于高度页就绪，也预留可独立显示高度的同外观退路。
        let fallback = match pages.fallback_material.clone() {
            Some(handle) if fallback_materials.get(&handle).is_some() => {
                if let Some(mut material) = fallback_materials.get_mut(&handle) {
                    material.base = fallback_base;
                    material.extension = extension.clone();
                }
                handle
            }
            _ => fallback_materials.add(ExtendedMaterial {
                base: fallback_base,
                extension: extension.clone(),
            }),
        };
        let wrapper = match pages.wrapped_material.clone() {
            Some(handle) if clipmap_materials.get(&handle).is_some() => {
                if let Some(mut material) = clipmap_materials.get_mut(&handle) {
                    material.base = base;
                    material.extension = extension;
                }
                handle
            }
            _ => clipmap_materials.add(ExtendedMaterial { base, extension }),
        };
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .remove::<TerrainSurfaceMaterialOverride>()
            .remove::<TerrainSurfaceCarrier>()
            .insert((
                MeshMaterial3d(wrapper.clone()),
                SparseClipmapSurfaceLease::new(&source, &wrapper),
            )); // 一个实体只保留一个材质draw；正式PBR仍包装同一高度页扩展。
        pages.wrapped_source = Some(source);
        pages.wrapped_material = Some(wrapper);
        pages.fallback_material = Some(fallback); // 地表资产稍后失效时无需依赖已经被移除的普通白模组件。
        pages.fallback_extension_revision = pages.extension_revision;
        pages.wrapped_extension_revision = pages.extension_revision;
        return;
    }

    // 地表纹理、作者候选或组合管线失败时也不能退回普通平面；基础PBR只替代着色，
    // 高度页、边界和所有预通道仍由同一Clipmap扩展负责。
    let retained_fallback = pages
        .fallback_material
        .clone()
        .or_else(|| installed_fallback.map(|installed| installed.0.clone()));
    let installed_matches = retained_fallback.as_ref().is_some_and(|retained| {
        installed_fallback.is_some_and(|installed| installed.0.id() == retained.id())
    });
    if pages.fallback_extension_revision == pages.extension_revision && installed_matches {
        return;
    }
    let base = direct_standard.and_then(|standard| base_materials.get(&standard.0).cloned());
    let fallback = match retained_fallback {
        Some(handle) if fallback_materials.get(&handle).is_some() => {
            if let Some(mut material) = fallback_materials.get_mut(&handle) {
                if let Some(base) = base {
                    material.base = base; // 新地图白模颜色仍作为基础PBR外观。
                }
                material.extension = extension;
            }
            handle
        }
        _ => {
            let Some(base) = base else {
                return; // 首帧必须先从正式源取得白模参数，不能凭空制造另一套外观。
            };
            fallback_materials.add(ExtendedMaterial { base, extension })
        }
    };
    commands
        .entity(entity)
        .remove::<MeshMaterial3d<StandardMaterial>>()
        .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
        .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
        .remove::<TerrainSurfaceMaterialOverride>()
        .remove::<TerrainSurfaceCarrier>()
        .remove::<SparseClipmapSurfaceLease>()
        .insert(MeshMaterial3d(fallback.clone()));
    pages.fallback_material = Some(fallback);
    pages.fallback_extension_revision = pages.extension_revision;
    pages.wrapped_source = None; // 普通平面恢复代表正式地表已放弃，旧内层不得被下一帧复活。
    pages.wrapped_extension_revision = 0;
}

/// 离场后继续非阻塞归还真实完成的 IO、编码和大缓冲释放槽。
pub(crate) fn reap_retired_sparse_height_pages(
    asset_server: Res<AssetServer>,
    mut pages: InstanceResMut<SparseHeightPageRuntime>,
) {
    pages.pending_io.retain(|handle| {
        matches!(
            asset_server.load_state(handle.id()),
            LoadState::NotLoaded | LoadState::Loading
        )
    }); // 最多三个旧IO租约。
    poll_page_set_release(&mut pages); // 大缓冲必须真实析构完成后才归还单飞槽。
    if pages.release_task.is_none()
        && let Some(prepared) = pages.prepared.take()
    {
        release_page_set_in_background(&mut pages, prepared); // 离场取消的已编码结果优先归还有界大缓冲。
    } else if pages.release_task.is_none()
        && let Some(cpu) = pages.retired_cpu.pop()
    {
        pages.release_task = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(cpu); // 每次只释放一份量化页，稳定保持单飞。
        }));
    }
    let retired_result = (pages.map_id.is_none())
        .then(|| pages.task.as_mut().and_then(check_ready))
        .flatten(); // 活动地图结果仍由正常采用系统核验和处理。
    if let Some(result) = retired_result {
        pages.task = None;
        pages.building = None;
        if let Ok(built) = result {
            if pages.release_task.is_none() {
                release_page_set_in_background(&mut pages, built); // 离场完成值也不在主帧释放最多两份8MiB字节。
            } else {
                AsyncComputeTaskPool::get()
                    .spawn(async move { drop(built) })
                    .detach(); // 防御旧释放任务尚未结束；离场不再启动任何新构建。
            }
        }
    }
}

fn display_material_source(
    direct_standard: Option<&MeshMaterial3d<StandardMaterial>>,
    direct_surface: Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
    installed_fallback: Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
    installed_clipmap: Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
    pages: &SparseHeightPageRuntime,
) -> Option<DisplayMaterialSource> {
    direct_surface
        .map(|source| DisplayMaterialSource::Terrain(source.0.id()))
        .or_else(|| {
            installed_clipmap
                .and_then(|_| pages.wrapped_source.as_ref())
                .map(|source| DisplayMaterialSource::Terrain(source.id()))
        })
        .or_else(|| direct_standard.map(|source| DisplayMaterialSource::Standard(source.0.id())))
        .or_else(|| installed_fallback.map(|source| DisplayMaterialSource::Fallback(source.0.id())))
}

/// 将同计划Mesh、完整页图集和当前地表来源组成一个离屏候选；只有RenderApp连续两帧
/// 证明资产与实际管线完整后，才在同一主世界提交中替换正式实体的Mesh与材质。
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_sparse_display_gpu_transaction(
    mut commands: Commands,
    mut runtime: InstanceResMut<DynamicMapRuntimeState>,
    mut display_status: InstanceResMut<MapTerrainDisplayLodStatus>,
    mut clipmap: InstanceResMut<SparseClipmapRuntime>,
    mut pages: InstanceResMut<SparseHeightPageRuntime>,
    mut uploads: Option<ResMut<WaterGpuUploadQueue>>,
    receipts: Option<Res<WaterGpuUploadReceipts>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    base_materials: Res<Assets<StandardMaterial>>,
    terrain_materials: Res<Assets<TerrainSurfaceMaterial>>,
    mut fallback_materials: ResMut<Assets<ClipmapHeightMaterial>>,
    mut surface_materials: ResMut<Assets<SparseClipmapSurfaceMaterial>>,
    mut fall_boundary: Option<InstanceResMut<feathrise_scene_core::state::SceneFallBoundary>>,
    displays: InstanceQuery<
        (
            Entity,
            &Mesh3d,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
            Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
            Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
        ),
        With<DynamicSparseClipmapPrimitive>,
    >,
) {
    // 一次完整提取后再退役旧资产；候选失败永远不进入这个槽。
    let mut retained = Vec::with_capacity(pages.retired.len());
    for mut retired in std::mem::take(&mut pages.retired) {
        if retired.frames_remaining > 0 {
            retired.frames_remaining -= 1;
        }
        if retired.frames_remaining > 0 {
            retained.push(retired);
            continue;
        }
        if let Some(mesh) = retired.mesh {
            meshes.remove(mesh.id());
        }
        if let Some(atlas) = retired.atlas {
            images.remove(atlas.heights.id());
            images.remove(atlas.page_table.id());
        }
        if let Some(material) = retired.fallback {
            fallback_materials.remove(material.id());
        }
        if let Some(material) = retired.surface {
            surface_materials.remove(material.id());
        }
        if let Some(cpu) = retired.cpu {
            pages.retired_cpu.push(cpu); // 下一帧由唯一后台释放槽处理。
        }
    }
    pages.retired = retained;

    let Ok((
        display,
        installed_mesh,
        direct_standard,
        direct_surface,
        installed_fallback,
        installed_clipmap,
    )) = displays.single()
    else {
        return;
    };
    let Some(source) = display_material_source(
        direct_standard,
        direct_surface,
        installed_fallback,
        installed_clipmap,
        &pages,
    ) else {
        return; // 地表自身仍在准备；保留已登记Mesh/页候选等待同源材质。
    };

    if let Some(mut transaction) = pages.transaction.take() {
        let identities_current = clipmap.desired.as_ref() == Some(&transaction.display_identity)
            && pages.desired.as_ref() == Some(&transaction.page_identity)
            && pages
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.identity == transaction.page_identity)
            && (clipmap.applied.as_ref() == Some(&transaction.display_identity)
                || clipmap
                    .prepared
                    .as_ref()
                    .is_some_and(|prepared| prepared.identity == transaction.display_identity));
        if !identities_current || source != transaction.source {
            if let Some(ticket) = transaction.ticket {
                if let Some(uploads) = uploads.as_deref_mut() {
                    uploads.retire(ticket);
                }
            }
            commands.entity(transaction.warmup_entity).try_despawn();
            match transaction.material {
                DisplayCandidateMaterial::Fallback(material) => {
                    fallback_materials.remove(material.id());
                }
                DisplayCandidateMaterial::Surface { material, .. } => {
                    surface_materials.remove(material.id());
                }
            } // 地表换代只淘汰材质预热；匹配Mesh与页候选可由新来源重新组合。
            return;
        }
        transaction.observed_frames = transaction.observed_frames.saturating_add(1);
        let pipeline_failed = transaction.ticket.is_some_and(|ticket| {
            receipts
                .as_deref()
                .is_some_and(|receipts| receipts.has_failed(ticket))
        });
        let timed_out = transaction.started_at.elapsed() >= Duration::from_secs(30);
        if pipeline_failed || timed_out {
            if let Some(ticket) = transaction.ticket {
                if let Some(uploads) = uploads.as_deref_mut() {
                    uploads.retire(ticket);
                }
            }
            commands.entity(transaction.warmup_entity).try_despawn();
            match transaction.material {
                DisplayCandidateMaterial::Fallback(material) => {
                    fallback_materials.remove(material.id());
                }
                DisplayCandidateMaterial::Surface { material, .. } => {
                    surface_materials.remove(material.id());
                }
            }
            pages.discard_candidate(&mut images);
            pages.failed_identity = Some(transaction.page_identity);
            if let Some(prepared) = clipmap.prepared.take() {
                if installed_mesh.0.id() == prepared.mesh.id() && clipmap.applied.is_none() {
                    commands.entity(display).remove::<Mesh3d>(); // 首载隐藏源不得引用已退役失败候选。
                }
                meshes.remove(prepared.mesh.id());
            }
            clipmap.failed = Some(transaction.display_identity);
            display_status.phase = MapTerrainDisplayLodPhase::Failed;
            display_status.detail = if pipeline_failed {
                "显示LOD GPU管线预热失败；保留上一完整显示".into()
            } else {
                "显示LOD GPU预热超过30秒；保留上一完整显示".into()
            };
            return;
        }
        let ready = transaction
            .ticket
            .map_or(transaction.observed_frames >= 2, |ticket| {
                receipts
                    .as_deref()
                    .is_some_and(|receipts| receipts.is_ready(ticket))
            });
        if !ready {
            display_status.phase = MapTerrainDisplayLodPhase::WarmingGpu;
            display_status.detail = "显示LOD候选正在验证Mesh、图集、材质和渲染管线".into();
            pages.transaction = Some(transaction);
            return;
        }

        let page_candidate = pages
            .candidate
            .take()
            .expect("事务身份已确认匹配的页候选存在");
        let old_cpu = std::mem::replace(&mut pages.cpu, page_candidate.cpu);
        let old_atlas = std::mem::replace(&mut pages.atlas, page_candidate.atlas.clone());
        pages.extension = Some(page_candidate.extension);
        pages.resident_identity = Some(page_candidate.identity.clone());
        if let Some(boundary) = fall_boundary.as_deref_mut()
            && let Some(bounds) = pages.adopted_height_bounds(&page_candidate.identity.map_id) {
            let threshold = (f64::from(bounds[0]) - 16.0).max(-f64::from(f32::MAX)) as f32;
            boundary.set_for_scene(feathrise_scene_core::state::ScenePhase::Active, threshold);
        } // 深谷与地表同一次采用后才拓展合法落下空间，等待候选时不改旧版玩法界。
        pages.extension_revision = pages.extension_revision.wrapping_add(1).max(1);
        pages.failed_identity = None;
        let old_mesh =
            (installed_mesh.0.id() != transaction.mesh.id()).then(|| installed_mesh.0.clone());
        let old_fallback = installed_fallback
            .filter(|installed| {
                !matches!(
                    &transaction.material,
                    DisplayCandidateMaterial::Fallback(candidate)
                        if candidate.id() == installed.0.id()
                )
            })
            .map(|installed| installed.0.clone());
        let old_surface = installed_clipmap
            .filter(|installed| {
                !matches!(
                    &transaction.material,
                    DisplayCandidateMaterial::Surface { material, .. }
                        if material.id() == installed.0.id()
                )
            })
            .map(|installed| installed.0.clone());
        let mut entity = commands.entity(display);
        entity
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
            .remove::<TerrainSurfaceMaterialOverride>()
            .remove::<TerrainSurfaceCarrier>()
            .remove::<SparseClipmapSurfaceLease>()
            .insert((Mesh3d(transaction.mesh.clone()), Visibility::Inherited));
        match transaction.material {
            DisplayCandidateMaterial::Fallback(material) => {
                entity.insert(MeshMaterial3d(material.clone()));
                pages.fallback_material = Some(material);
                pages.fallback_extension_revision = pages.extension_revision;
                pages.wrapped_source = None;
                pages.wrapped_material = None;
                pages.wrapped_extension_revision = 0;
            }
            DisplayCandidateMaterial::Surface { source, material } => {
                entity.insert((
                    MeshMaterial3d(material.clone()),
                    SparseClipmapSurfaceLease::new(&source, &material),
                ));
                pages.wrapped_source = Some(source);
                pages.wrapped_material = Some(material);
                pages.wrapped_extension_revision = pages.extension_revision;
            }
        }
        commands.entity(transaction.warmup_entity).try_despawn();
        if let Some(ticket) = transaction.ticket
            && let Some(uploads) = uploads.as_deref_mut()
        {
            uploads.retire(ticket);
        }
        pages.retired.push(RetiredSparseDisplayAssets {
            frames_remaining: 2, // 本次提交后的下一次提取仍可安全读取旧句柄。
            mesh: old_mesh,
            atlas: old_atlas,
            fallback: old_fallback,
            surface: old_surface,
            cpu: old_cpu,
        });
        clipmap.applied = Some(transaction.display_identity.clone());
        clipmap.failed = None;
        if clipmap
            .prepared
            .as_ref()
            .is_some_and(|prepared| prepared.mesh.id() == transaction.mesh.id())
        {
            clipmap.prepared = None; // 句柄已经转交正式实体，不从Assets移除。
        }
        runtime.sparse_clipmap_ready = true;
        runtime.sparse_height_pages_ready = true;
        display_status.applied_lods = transaction.display_identity.plan.authored_lods.clone();
        display_status.actual_half_extents_meters = transaction
            .display_identity
            .plan
            .actual_half_extents_meters
            .clone();
        display_status.effective_band_count = transaction.display_identity.plan.bands.len();
        display_status.resident_height_pages = page_candidate.identity.pages.len();
        display_status.vertex_count =
            usize::try_from(transaction.display_identity.plan.vertex_count).unwrap_or(usize::MAX);
        display_status.triangle_count =
            usize::try_from(transaction.display_identity.plan.index_count / 3)
                .unwrap_or(usize::MAX);
        display_status.mesh_bytes = transaction.display_identity.plan.mesh_bytes;
        if let Some(build_milliseconds) = transaction.build_milliseconds {
            display_status.build_milliseconds = build_milliseconds;
        } // 页窗口换代复用正式Mesh时保留上一次拓扑构建时间。
        display_status.upload_wait_milliseconds =
            transaction.started_at.elapsed().as_secs_f32() * 1000.0;
        display_status.work.display_adoptions = display_status.work.display_adoptions.saturating_add(1);
        display_status.phase = if display_status.saved_lods.is_some() {
            MapTerrainDisplayLodPhase::Applied
        } else {
            MapTerrainDisplayLodPhase::LegacyAutomatic
        };
        display_status.detail = if display_status.saved_lods.is_some() {
            "显示LOD候选已完成GPU预热并原子采用".into()
        } else {
            "正在使用兼容自动显示LOD（64半格逐级翻倍）".into()
        };
        return;
    }

    let Some(display_identity) = clipmap.desired.clone() else {
        return;
    };
    let Some(page_candidate) = pages.candidate.as_ref() else {
        return;
    };
    if pages.desired.as_ref() != Some(&page_candidate.identity)
        || page_candidate.identity.display_plan != display_identity.plan
    {
        return;
    }
    let (mesh, mesh_registered_at, build_milliseconds) =
        if clipmap.applied.as_ref() == Some(&display_identity) {
            (installed_mesh.0.clone(), page_candidate.registered_at, None)
        } else {
            let Some(prepared) = clipmap
                .prepared
                .as_ref()
                .filter(|prepared| prepared.identity == display_identity)
            else {
                return;
            };
            (
                prepared.mesh.clone(),
                prepared.registered_at,
                Some(prepared.build_milliseconds),
            )
        };
    let extension = page_candidate.extension.clone();
    let material = match source {
        DisplayMaterialSource::Terrain(id) => {
            let Some(source_handle) = pages
                .wrapped_source
                .clone()
                .filter(|source| source.id() == id)
                .or_else(|| direct_surface.map(|source| source.0.clone()))
            else {
                return;
            };
            let Some(mut base) = terrain_materials.get(&source_handle).cloned() else {
                return;
            };
            // 离屏候选与正式包装遵守同一坐标合同；预热成功不能发布高度/地表两个不同来源框架。
            base.extension.set_source_frame(extension.params.source_to_world);
            let material = surface_materials.add(ExtendedMaterial { base, extension });
            DisplayCandidateMaterial::Surface {
                source: source_handle,
                material,
            }
        }
        DisplayMaterialSource::Standard(id) => {
            let Some(base) = base_materials.get(id).cloned() else {
                return;
            };
            DisplayCandidateMaterial::Fallback(
                fallback_materials.add(ExtendedMaterial { base, extension }),
            )
        }
        DisplayMaterialSource::Fallback(id) => {
            let Some(base) = fallback_materials
                .get(id)
                .map(|material| material.base.clone())
            else {
                return;
            };
            DisplayCandidateMaterial::Fallback(
                fallback_materials.add(ExtendedMaterial { base, extension }),
            )
        }
    };
    let mut warmup = commands.spawn((
        feathrise_scene_core::residency::MapInstanceDetached,
        Name::new("SparseClipmap显示LOD离屏GPU预热"),
        Mesh3d(mesh.clone()),
        Transform::from_xyz(0.0, -100_000.0, 0.0).with_scale(Vec3::splat(0.000_001)),
        Visibility::Inherited,
        NoFrustumCulling,
    ));
    match &material {
        DisplayCandidateMaterial::Fallback(material) => {
            warmup.insert(MeshMaterial3d(material.clone()));
        }
        DisplayCandidateMaterial::Surface { material, .. } => {
            warmup.insert(MeshMaterial3d(material.clone()));
        }
    }
    let warmup_entity = warmup.id();
    let ticket = uploads.as_deref_mut().map(|uploads| {
        uploads.submit_assets(
            vec![mesh.id()],
            vec![material.untyped_id()],
            vec![warmup_entity],
        )
    });
    let started_at = if mesh_registered_at < page_candidate.registered_at {
        mesh_registered_at
    } else {
        page_candidate.registered_at
    };
    pages.transaction = Some(SparseDisplayGpuTransaction {
        display_identity,
        page_identity: page_candidate.identity.clone(),
        mesh,
        material,
        source,
        warmup_entity,
        ticket,
        observed_frames: 0,
        started_at,
        build_milliseconds,
    });
    display_status.phase = MapTerrainDisplayLodPhase::WarmingGpu;
    display_status.detail = "显示LOD候选已登记，正在离屏验证Forward/Deferred/深度/阴影管线".into();
}

/// 离场释放任务、块句柄、程序图片句柄和外层材质资产。
pub(crate) fn clear_sparse_height_pages(
    mut commands: Commands,
    mut pages: InstanceResMut<SparseHeightPageRuntime>,
    mut uploads: Option<ResMut<WaterGpuUploadQueue>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut fallback_materials: ResMut<Assets<ClipmapHeightMaterial>>,
    mut surface_materials: ResMut<Assets<SparseClipmapSurfaceMaterial>>,
    displays: InstanceQuery<&Mesh3d, With<DynamicSparseClipmapPrimitive>>,
) {
    if let Some(transaction) = pages.transaction.take() {
        commands.entity(transaction.warmup_entity).try_despawn();
        if let Some(ticket) = transaction.ticket
            && let Some(uploads) = uploads.as_deref_mut()
        {
            uploads.retire(ticket);
        }
        match transaction.material {
            DisplayCandidateMaterial::Fallback(material) => {
                fallback_materials.remove(material.id());
            }
            DisplayCandidateMaterial::Surface { material, .. } => {
                surface_materials.remove(material.id());
            }
        }
    }
    pages.discard_candidate(&mut images);
    if let Some(atlas) = pages.atlas.take() {
        images.remove(atlas.heights.id());
        images.remove(atlas.page_table.id());
    }
    for retired in std::mem::take(&mut pages.retired) {
        if let Some(mesh) = retired.mesh {
            meshes.remove(mesh.id());
        }
        if let Some(atlas) = retired.atlas {
            images.remove(atlas.heights.id());
            images.remove(atlas.page_table.id());
        }
        if let Some(material) = retired.fallback {
            fallback_materials.remove(material.id());
        }
        if let Some(material) = retired.surface {
            surface_materials.remove(material.id());
        }
        if let Some(cpu) = retired.cpu {
            pages.retired_cpu.push(cpu); // 离场收尾系统继续逐份后台归还。
        }
    }
    if let Ok(mesh) = displays.single() {
        meshes.remove(mesh.0.id());
    }
    if let Some(material) = pages.fallback_material.take() {
        fallback_materials.remove(material.id());
    }
    if let Some(material) = pages.wrapped_material.take() {
        surface_materials.remove(material.id());
    }
    if let Some(cpu) = pages.cpu.take() {
        pages.retired_cpu.push(cpu); // 当前正式量化页同样留给离场收尾系统后台释放。
    }
    let pending_io = std::mem::take(&mut pages.pending_io); // 离场仍保留尚未完成的IO槽，防快速切图无限派发。
    let task = pages.task.take(); // CPU任务真正返回前仍占据唯一槽。
    let building = pages.building.take(); // 旧错误仍需识别为失效请求。
    let release_task = pages.release_task.take(); // 已开始的后台析构继续由收尾系统轮询。
    let prepared = pages.prepared.take(); // 尚未登记的完成结果等待收尾系统转交释放槽。
    let retired_cpu = std::mem::take(&mut pages.retired_cpu); // 候选和旧正式CPU页继续顺序释放。
    *pages = SparseHeightPageRuntime {
        pending_io,
        task,
        building,
        release_task,
        prepared,
        retired_cpu,
        ..Default::default()
    }; // 清除地图内容但保留所有真实在途背压。
}

#[cfg(test)]
mod tests {
    use super::*; // 纯 CPU 覆盖全层窗口、共享边、实时覆盖和硬上传预算。

    #[test] // 快速换图不得把仍运行的旧任务当成已经取消。
    fn clipmap_retarget_keeps_pending_io_and_cpu_backpressure() {
        // 无窗口验证真实任务句柄生命周期。
        let pool = bevy::tasks::TaskPool::new(); // 独立池不干扰全局Bevy任务配置。
        let mut pages = SparseHeightPageRuntime::default(); // 初始分页会话。
        pages.pending_io = vec![Handle::default(); MAX_PAGE_IO_TASKS]; // 模拟三个仍在AssetServer加载的旧页。
        pages.task = Some(pool.spawn(async { Err("旧页任务结果".to_owned()) })); // 即使很快完成也必须由轮询归还槽。
        pages.begin_target(
            MapId::parse("MAP-ground-retarget").unwrap(),
            Entity::from_bits(91),
            "new-height.json".into(),
        ); // 新地图来源不能清除旧工作。
        assert_eq!(pages.pending_io.len(), MAX_PAGE_IO_TASKS); // 未真实完成的IO仍占满预算。
        assert!(pages.task.is_some()); // CPU在途句柄保留，不会启动第二个隐藏worker。
        assert!(pages.loading.is_none() && pages.prepared.is_none() && pages.cpu.is_none()); // 旧内容不得作为新地图结果。
    } // 新地图等待真正结束后正常复用同一有界槽。

    use bevy::prelude::Vec2; // 会话技能浅坑由世界坐标指定。
    use feathrise_environment::terrain_surface::{
        TerrainSurfaceDescriptor, TerrainSurfaceExtension, TerrainSurfaceLayer,
    }; // 完整地表先到达的回归场景必须使用正式固定布局。
    use feathrise_map_content::{
        SPARSE_HEIGHT_SCHEMA_VERSION, SparseHeightMipTileRef, SparseHeightTileRef,
    }; // 测试清单与正式第二版父页索引使用同一类型。

    fn empty_manifest() -> SparseHeightManifest {
        SparseHeightManifest::empty(
            MapId::parse("MAP-clipmap-pages").unwrap(),
            SparseHeightSpec::new([0.0, 0.0], [8192.0, 8192.0], 0.125, -1024.0, 2048.0).unwrap(),
        )
        .unwrap()
    }

    /// 测试夹具把旧缺字段地图转换为只驻留内存的等价显示计划。
    fn legacy_display_plan(spec: &SparseHeightSpec) -> CompiledDisplayLodPlan {
        let lods = feathrise_map_content::default_display_lods_for_spec(spec).unwrap(); // 64半格逐级翻倍。
        feathrise_map_content::compile_display_lod_plan(spec, &lods).unwrap() // 与正式运行时同一编译器。
    }

    /// 旧分页回归统一通过默认计划，避免测试继续隐式依赖固定128格常量。
    fn legacy_desired_identity(
        manifest: &SparseHeightManifest,
        root: Entity,
        revision: u64,
        focus: [u64; 2],
        live: Option<&SparseHeightLiveOverride>,
        craters: Option<&SparseHeightSessionCrater>,
    ) -> Result<PageSetIdentity, String> {
        desired_identity(
            manifest,
            root,
            revision,
            focus,
            &legacy_display_plan(&manifest.spec),
            live,
            craters,
        ) // 临时计划在函数调用期间完整有效，结果持有自己的身份副本。
    }

    /// 返回兼容计划在指定持久Mip的真实可见半径。
    fn legacy_visible_half_cells(spec: &SparseHeightSpec, level: u8) -> [u64; 2] {
        legacy_display_plan(spec)
            .sample_ranges
            .into_iter()
            .find(|range| range.mip_level == level)
            .expect("兼容计划必须采样每个持久Mip")
            .half_cells // 默认远层钳制到最高Mip时可能大于64格。
    }

    #[test]
    fn adopted_crater_window_does_not_rebuild_until_identity_changes() {
        let manifest = empty_manifest(); // 构造当前稀疏地图的真实页身份。
        let identity =
            legacy_desired_identity(&manifest, Entity::from_bits(5), 1, [80, 80], None, None)
                .unwrap();
        let mut runtime = SparseHeightPageRuntime::default(); // 未采用的初始窗口仍应允许编码。
        runtime.desired = Some(identity.clone());
        assert!(!runtime.resident_window_is_current());
        runtime.resident_identity = Some(identity); // GPU页与碰撞共同提交后，稳定帧无需再次编码。
        assert!(runtime.resident_window_is_current());
        runtime.desired.as_mut().unwrap().crater_revision = Some(2); // 同图追加技能破坏必须重新构建。
        assert!(!runtime.resident_window_is_current());
        runtime.resident_identity = runtime.desired.clone();
        runtime.desired.as_mut().unwrap().live_revision = Some(3); // F12草稿变化也不能沿用旧页。
        assert!(!runtime.resident_window_is_current());
    }

    #[test]
    fn crater_creates_gpu_pages_on_flat_map_and_matches_parent_level() {
        let manifest = empty_manifest(); // 未保存过作者块的大图原先只有零高度。
        let mut craters = SparseHeightSessionCrater::default();
        craters.clear(Some(manifest.map_id.clone())); // 当前地图会话单独持有技能覆盖。
        craters
            .apply(
                &manifest.spec,
                Vec2::new(10.0, 10.0),
                4.0,
                0.5,
                crate::maps::dynamic_map::session_crater::soil_surface_for_test(&manifest.spec),
            )
            .unwrap();
        let identity = legacy_desired_identity(
            &manifest,
            Entity::from_bits(5),
            1,
            [80, 80],
            None,
            Some(&craters),
        )
        .unwrap(); // 浅坑让原本全平的GPU窗口按需分配页。
        assert_eq!(identity.crater_revision, Some(craters.revision()));
        assert!(identity.pages.iter().any(|page| page.level == 0)); // 近景必须显示真实浅坑。
        assert!(identity.pages.iter().any(|page| page.level == 2)); // 父级远景也需叠加同一世界高度。
        let formal = BTreeMap::new(); // 无正式作者高度页，基准平地为Y=0。
        let live = BTreeMap::new(); // F12草稿完全独立。
        let near =
            sample_page_grid(&manifest.spec, &formal, &live, craters.tiles(), 0, 80, 80).unwrap();
        let far =
            sample_page_grid(&manifest.spec, &formal, &live, craters.tiles(), 2, 20, 20).unwrap();
        assert_eq!(near, -0.5);
        assert_eq!(far, near); // 同一世界格点在远近Mip读取相同会话修订。
    }

    #[test]
    fn camera_lod_adoption_preserves_authoritative_content_identity() {
        use crate::maps::dynamic_map::sparse_lod::{compile_view_plan, ViewLodKey};
        let manifest = empty_manifest();
        let identity = legacy_desired_identity(&manifest, Entity::from_bits(81), 1, [0, 0], None, None).unwrap();
        let plan = compile_view_plan(&manifest.spec, &identity.display_plan, ViewLodKey {
            minimum_multiplier: 8, coverage_cells: manifest.spec.cells,
        }).unwrap();
        let mut pages = SparseHeightPageRuntime::default();
        pages.resident_identity = Some(identity);
        let before = pages.ground_content_revision().unwrap();
        pages.resident_identity.as_mut().unwrap().display_plan = plan;
        assert_eq!(pages.ground_content_revision().unwrap(), before, "拉远不能重建树木或使作者高度页失效");
        pages.resident_identity.as_mut().unwrap().live_revision = Some(2);
        assert_ne!(pages.ground_content_revision().unwrap(), before, "真实雕刻仍须使承托内容换代");
    }

    #[test] // 快照必须拒绝迟到高度，同时允许正式空白页。
    fn blade_snapshot_only_marks_the_current_adopted_window_ready() {
        // 无GPU即可验证发布身份。
        let manifest = empty_manifest(); // 最大地图空白夹具不分配细格。
        let root = Entity::from_bits(81); // 唯一显示根身份。
        let identity = legacy_desired_identity(&manifest, root, 1, [0, 0], None, None).unwrap(); // 正式空白窗口。
        let plan = identity.display_plan.clone(); // 草根必须核对正式采用的完整显示计划。
        let mut pages = SparseHeightPageRuntime::default(); // 无在途后台任务。
        pages.map_id = Some(manifest.map_id.clone()); // 当前地图。
        pages.root = Some(root); // 当前根。
        pages.spec = Some(manifest.spec.clone()); // 固定大小世界规格。
        pages.extension = Some(
            ClipmapHeightExtension::new_flat(
                // 与地表相同的空白句柄。
                ClipmapBounds::new(
                    Vec2::from_array(manifest.spec.world_min),
                    Vec2::from_array(manifest.spec.world_max),
                )
                .unwrap(), // 正式范围。
                UVec2::new(manifest.spec.cells[0] as u32, manifest.spec.cells[1] as u32), // 无整图分配。
                &ClipmapFlatHeightPage {
                    heights: Handle::default(),
                    page_table: Handle::default(),
                }, // 只测试CPU发布身份。
            )
            .unwrap(),
        ); // 合法规格。
        pages.desired = Some(identity.clone()); // 已请求当前窗口。
        pages.resident_identity = Some(identity); // 空白窗口完整采用。
        let adopted = pages
            .blade_height(&manifest.map_id, &plan, Vec2::ZERO)
            .unwrap(); // 空白不再等待GLB。
        assert!(adopted.ready); // 已采用空白页可以显示草地。
        assert_eq!(adopted.logical_cells, UVec2::splat(65_536)); // 最大图不会缩成伪造整图纹理。
        pages.desired.as_mut().unwrap().live_revision = Some(2); // 模拟雕刻新页仍在后台生成。
        assert!(
            !pages
                .blade_height(&manifest.map_id, &plan, Vec2::ZERO)
                .unwrap()
                .ready
        ); // 旧图集不能冒充新平地。
        let wrong_map = MapId::parse("MAP-other-blade").unwrap(); // 模拟切图迟到读取。
        assert!(pages.blade_height(&wrong_map, &plan, Vec2::ZERO).is_none()); // 不发布跨图资源。
        pages.resident_identity = pages.desired.clone(); // 模拟一次完整原子采用。
        pages.extension_revision += 1; // 高度修订随采用变更。
        let moved = pages
            .blade_height(&manifest.map_id, &plan, Vec2::splat(1.0))
            .unwrap(); // 使用新的真实几何中心。
        assert!(moved.ready); // 完整发布恢复绘制。
        assert_ne!(adopted.revision[1], moved.revision[1]); // 高度换代可被下游检测。
        assert_ne!(adopted.revision[2], moved.revision[2]); // 视图换代独立于高度。
        pages.desired.as_mut().unwrap().pages = vec![ResidentPageKey {
            level: 3,
            key: SparseHeightTileKey { x: 2, z: 1 },
        }]; // 从已采用全平视图移向未驻留的非平父页。
        let entering_nonflat = pages
            .blade_height(&manifest.map_id, &plan, Vec2::splat(1.0))
            .unwrap(); // atlas仍为None的合法全平回退。
        assert!(entering_nonflat.ready); // 其余平地区域草不能全域消失。
        assert_eq!(
            entering_nonflat.max_mip,
            u32::from(manifest.spec.max_mip_level())
        ); // 父页缺失检查不能被max_mip=0跳过。
        assert_eq!(
            entering_nonflat.missing_pages[3],
            UVec4::new((2 | (1 << 16)) + 1, 0, 0, 0)
        ); // 只有真正非平缺页位置被拒绝。
        assert_eq!(entering_nonflat.missing_pages[0], UVec4::ZERO); // 未请求的零级仍是有效隐式平地。
    } // 不运行游戏也可验证避免平地假象的关键门。

    #[test]
    fn sculpt_blade_render_keeps_four_complete_windows_until_height_adoption_without_authoring_fallback() {
        use feathrise_environment::vegetation::blade::GpuBladeClipmapHeight;
        use crate::maps::dynamic_map::{sparse_runtime::SparseClipmapRuntime,
            vegetation::{DynamicVegetationState, sync_dynamic_blade_clipmap, DynamicBladeWindow}};
        let manifest = empty_manifest();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let root = app.world_mut().spawn_empty().id();
        let display = app.world_mut().spawn((SparseClipmapDisplay, Transform::default())).id();
        let identity = legacy_desired_identity(&manifest, root, 1, [0, 0], None, None).unwrap();
        let plan = identity.display_plan.clone();
        let mut pages = SparseHeightPageRuntime::default();
        pages.map_id = Some(manifest.map_id.clone()); pages.root = Some(root); pages.spec = Some(manifest.spec.clone());
        pages.extension = Some(ClipmapHeightExtension::new_flat(
            ClipmapBounds::new(Vec2::from_array(manifest.spec.world_min), Vec2::from_array(manifest.spec.world_max)).unwrap(),
            UVec2::new(manifest.spec.cells[0] as u32, manifest.spec.cells[1] as u32),
            &ClipmapFlatHeightPage { heights: Handle::default(), page_table: Handle::default() }).unwrap());
        pages.desired = Some(identity.clone()); pages.resident_identity = Some(identity);
        let baseline = pages.blade_render_height(&manifest.map_id, &plan, Vec2::ZERO).unwrap();
        let windows = (0..4).map(|_| app.world_mut().spawn((DynamicBladeWindow { snap_cells: 1 }, baseline.clone(),
            feathrise_environment::vegetation::blade::GpuBladePatch::square(Vec3::ZERO, 128.0, 65, 7)
                .with_height_field(baseline.world_min, baseline.world_max, baseline.height_min, baseline.height_max),
            bevy::camera::primitives::Aabb { center: bevy::math::Vec3A::ZERO, half_extents: bevy::math::Vec3A::new(64.0, 1.0, 64.0) },
            Transform::IDENTITY)).id()).collect::<Vec<_>>();
        app.insert_resource(ActiveDynamicMap { map_id: Some(manifest.map_id.clone()) })
            .insert_resource(pages)
            .insert_resource(SparseClipmapRuntime::adopted_blade_geometry_for_test(manifest.map_id.clone(), root, display, plan.clone()))
            .init_resource::<DynamicVegetationState>()
            .add_systems(Update, sync_dynamic_blade_clipmap);
        app.update();
        for revision in 1..=12 {
            {
                let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>();
                let desired = pages.desired.as_mut().unwrap();
                desired.live_revision = Some(revision);
                desired.height_bounds = [-100.0 * revision as f32, 100.0 * revision as f32];
                desired.pages = vec![ResidentPageKey { level: 0, key: SparseHeightTileKey { x: 0, z: 0 } }];
                assert!(!pages.blade_height(&manifest.map_id, &plan, Vec2::ZERO).unwrap().ready,
                    "最新作者点仍需等待同修订完整地形，不能用旧视觉版取得写权限");
            }
            app.update();
            for entity in &windows {
                let actual = app.world().get::<GpuBladeClipmapHeight>(*entity).unwrap();
                assert!(actual.ready, "第{revision}次雕刻在途不能提前熄灭整片草");
                assert_eq!(actual.atlas, baseline.atlas);
                assert_eq!(actual.page_table, baseline.page_table);
                assert_eq!(actual.revision, baseline.revision);
                assert_eq!([actual.height_min, actual.height_max], [baseline.height_min, baseline.height_max]);
                assert_eq!(app.world().get::<bevy::camera::primitives::Aabb>(*entity).unwrap().half_extents.y, 1.0);
                assert!(actual.missing_pages.iter().all(|value| *value == UVec4::ZERO), "未来非平页不能污染旧视觉版");
            }
        }
        {
            let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>();
            pages.resident_identity = pages.desired.clone();
            pages.extension_revision += 1; // 对应正式事务Mesh/图集/材质共同采用后的唯一换代边界。
        }
        app.update();
        let adopted = app.world().get::<GpuBladeClipmapHeight>(windows[0]).unwrap().clone();
        assert!(adopted.ready); assert_ne!(adopted.revision, baseline.revision);
        assert_eq!([adopted.height_min, adopted.height_max], [-1200.0, 1200.0]);
        for entity in &windows {
            let aabb = app.world().get::<bevy::camera::primitives::Aabb>(*entity).unwrap();
            assert_eq!(aabb.center.y, 0.0);
            assert_eq!(aabb.half_extents.y, 1201.0, "草窗CPU包围必须随同版高峰/深谷扩展");
        }
        for entity in &windows { assert_eq!(*app.world().get::<GpuBladeClipmapHeight>(*entity).unwrap(), adopted); }
        app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = Some(MapId::parse("MAP-other-render").unwrap());
        app.update();
        for entity in windows { assert!(!app.world().get::<GpuBladeClipmapHeight>(entity).unwrap().ready, "跨图仍立即撤销旧草显示"); }
    }

    #[test]
    fn sculpt_blade_render_retention_rejects_different_root_manifest_or_corrupt_dependencies() {
        let manifest = empty_manifest();
        let root = Entity::from_bits(82);
        let identity = legacy_desired_identity(&manifest, root, 1, [0, 0], None, None).unwrap();
        for change in 0..4 {
            let mut pages = SparseHeightPageRuntime::default();
            pages.map_id = Some(manifest.map_id.clone()); pages.root = Some(root); pages.spec = Some(manifest.spec.clone());
            pages.extension = Some(ClipmapHeightExtension::new_flat(
                ClipmapBounds::new(Vec2::ZERO, Vec2::splat(8192.0)).unwrap(), UVec2::splat(65536),
                &ClipmapFlatHeightPage { heights: Handle::default(), page_table: Handle::default() }).unwrap());
            pages.resident_identity = Some(identity.clone()); pages.desired = Some(identity.clone());
            let desired = pages.desired.as_mut().unwrap(); desired.live_revision = Some(4);
            match change {
                0 => desired.map_id = MapId::parse("MAP-other-render").unwrap(),
                1 => desired.root = Entity::from_bits(83),
                2 => desired.manifest_revision += 1,
                _ => {
                    let page = ResidentPageKey { level: 0, key: SparseHeightTileKey { x: 0, z: 0 } };
                    desired.dependencies = vec![PageDependency { page, sha256: "new".into() }];
                    pages.resident_identity.as_mut().unwrap().dependencies = vec![PageDependency { page, sha256: "old".into() }];
                }
            }
            assert!(!pages.blade_render_height(&manifest.map_id, &identity.display_plan, Vec2::ZERO).unwrap().ready, "case {change}");
        }
    }

    #[test]
    fn sculpt_blade_render_moving_window_preserves_old_nonflat_missing_pages() {
        let manifest = empty_manifest();
        let root = Entity::from_bits(86);
        let mut identity = legacy_desired_identity(&manifest, root, 1, [0, 0], None, None).unwrap();
        identity.live_revision = Some(1); // 即使实时覆盖资源已有空修订，也不能把Some误认为已经有非平页。
        let plan = identity.display_plan.clone();
        let mut pages = SparseHeightPageRuntime::default();
        pages.map_id = Some(manifest.map_id.clone()); pages.root = Some(root); pages.spec = Some(manifest.spec.clone());
        pages.extension = Some(ClipmapHeightExtension::new_flat(
            ClipmapBounds::new(Vec2::from_array(manifest.spec.world_min), Vec2::from_array(manifest.spec.world_max)).unwrap(),
            UVec2::new(manifest.spec.cells[0] as u32, manifest.spec.cells[1] as u32),
            &ClipmapFlatHeightPage { heights: Handle::default(), page_table: Handle::default() }).unwrap());
        pages.resident_identity = Some(identity.clone());
        let resident = pages.extension.as_ref().unwrap().blade_textures();
        identity.live_revision = Some(2); // 新雕刻尚未采用，同时相机窗口移到旧清单已有的非平页。
        identity.has_live_pages = true;
        identity.pages = [2, 4, 8].map(|x| ResidentPageKey { level: 0,
            key: SparseHeightTileKey { x, z: 0 } }).to_vec();
        identity.dependencies = vec![
            PageDependency { page: ResidentPageKey { level: 0, key: SparseHeightTileKey { x: 2, z: 0 } }, sha256: "old-disk-page".into() },
            PageDependency { page: ResidentPageKey { level: 0, key: SparseHeightTileKey { x: 5, z: 1 } }, sha256: "old-shared-corner".into() },
        ]; // 第4页的右上所有者同样来自旧清单；第8页则只属于未来live版。
        pages.desired = Some(identity);
        let strict = pages.blade_height(&manifest.map_id, &plan, Vec2::new(128.0, 0.0)).unwrap();
        assert!(!strict.ready, "严格作者高度仍等待当前编辑代次");
        let retained = pages.blade_render_height(&manifest.map_id, &plan, Vec2::new(128.0, 0.0)).unwrap();
        assert!(retained.ready);
        assert_eq!((retained.atlas, retained.page_table), resident);
        assert_eq!(retained.missing_pages[0], UVec4::new(3, 5, 0, 0),
            "旧内容真实缺页与共享边保留局部裁剪，只排除已证明原是平地的未来编辑页");

        pages.resident_identity.as_mut().unwrap().has_live_pages = true;
        let conservative = pages.blade_render_height(&manifest.map_id, &plan, Vec2::new(128.0, 0.0)).unwrap();
        assert!(conservative.ready);
        assert_eq!(conservative.missing_pages[0], UVec4::new(3, 5, 9, 0),
            "旧内容已有其他实时块但缺少全图旧占用证据时，不能把未驻留页猜成平地");
        pages.resident_identity = pages.desired.clone();
        pages.extension_revision += 1;
        let adopted = pages.blade_render_height(&manifest.map_id, &plan, Vec2::new(128.0, 0.0)).unwrap();
        assert!(adopted.ready);
        assert!(adopted.missing_pages.iter().all(|mask| *mask == UVec4::ZERO));
        assert_ne!(adopted.revision, retained.revision);
    }

    #[test] // 移出旧页不关草，新增非平页只局部等待，内容换代仍严格拒绝。
    fn blade_resident_coverage_masks_only_missing_pages_and_rejects_new_content() {
        // 请求与采用身份分离的回归。
        let manifest = empty_manifest(); // 最大规格的轻量清单。
        let mut identity =
            legacy_desired_identity(&manifest, Entity::from_bits(99), 3, [0, 0], None, None)
                .unwrap(); // 受检同图身份。
        let first = ResidentPageKey {
            level: 0,
            key: SparseHeightTileKey { x: 0, z: 0 },
        }; // 当前视图非平页。
        let second = ResidentPageKey {
            level: 0,
            key: SparseHeightTileKey { x: 1, z: 0 },
        }; // 稍早视图还需要的邻页。
        identity.pages = vec![first, second]; // BTree顺序与生产构造一致。
        identity.dependencies = vec![
            PageDependency {
                page: first,
                sha256: "first".into(),
            },
            PageDependency {
                page: second,
                sha256: "second".into(),
            },
        ]; // 内容地址作为共享边身份。
        let mut pages = SparseHeightPageRuntime::default(); // 无纹理分配的纯身份测试。
        pages.resident_identity = Some(identity.clone()); // 旧完整页组仍被地表使用。
        pages.desired = Some(identity.clone()); // 初始视图完全一致。
        assert!(pages.resident_content_is_current()); // 完整首载可以绘制。
        pages.desired.as_mut().unwrap().pages.pop(); // 移镜后仅需第一页。
        pages.desired.as_mut().unwrap().dependencies.pop(); // 目标内容依赖同步缩小。
        assert!(!pages.resident_window_is_current()); // 页流送仍可异步裁减驻留集合。
        assert!(pages.resident_content_is_current()); // 草继续使用已覆盖数据，不能全片闪黑。
        let subset = pages.desired.clone(); // 保存基准覆盖请求。
        pages.desired.as_mut().unwrap().pages.push(ResidentPageKey {
            level: 0,
            key: SparseHeightTileKey { x: 2, z: 0 },
        }); // 请求此前没有的非平页。
        assert!(pages.resident_content_is_current()); // 新页请求不能把已有区域的草全部关闭。
        assert_eq!(pages.blade_missing_pages()[0], UVec4::new(3, 0, 0, 0)); // 缺失(2,0)编码为3，其余驻留页不受影响。
        assert!(
            pages.blade_missing_pages()[1..]
                .iter()
                .all(|entry| *entry == UVec4::ZERO)
        ); // 不把基础页误映射到别的Mip。
        pages
            .resident_identity
            .as_mut()
            .unwrap()
            .pages
            .push(ResidentPageKey {
                level: 0,
                key: SparseHeightTileKey { x: 2, z: 0 },
            }); // 模拟新增页完整采用。
        assert!(
            pages
                .blade_missing_pages()
                .iter()
                .all(|entry| *entry == UVec4::ZERO)
        ); // 页就绪后局部草自然恢复。
        for field in 0..5 {
            // 内容身份的每一个独立分量均需严格核对。
            pages.desired = subset.clone(); // 每项从同一个合法子集开始。
            let desired = pages.desired.as_mut().unwrap(); // 只修改当前测试分量。
            match field {
                // 分别模拟切图、重建、磁盘热更、雕刻和技能破坏。
                0 => desired.map_id = MapId::parse("MAP-different-blade").unwrap(), // 不同地图。
                1 => desired.root = Entity::from_bits(100),                         // 同图不同根。
                2 => desired.manifest_revision += 1, // 磁盘版本变化。
                3 => desired.live_revision = Some(4), // 作者实时高度变化。
                _ => desired.crater_revision = Some(5), // 技能高度变化。
            } // 不允许在不同内容版本间复用旧页。
            assert!(!pages.resident_content_is_current()); // 所有内容换代均拒绝旧快照。
        } // 完成各身份分量校验。
        pages.desired = subset; // 回到合法子集。
        pages.desired.as_mut().unwrap().dependencies[0].sha256 = "replaced".into(); // 防御性检查同修订坏hash。
        assert!(!pages.resident_content_is_current()); // 共享边同坐标但不同内容也不可采用。
    } // 新非平页用显式局部掩码，内容代次从不放宽。

    fn insert_page(manifest: &mut SparseHeightManifest, page: ResidentPageKey) {
        let tile = SparseHeightTile::blank_at_level(&manifest.spec, page.level, page.key).unwrap();
        if page.level == 0 {
            manifest.tiles.push(SparseHeightTileRef {
                key: page.key,
                sha256: tile.sha256(),
            });
            manifest.tiles.sort_by_key(|entry| entry.key);
        } else {
            manifest.mip_tiles.push(SparseHeightMipTileRef {
                level: page.level,
                key: page.key,
                sha256: tile.sha256(),
            });
            manifest
                .mip_tiles
                .sort_by_key(|entry| (entry.level, entry.key));
        }
        assert_eq!(manifest.schema_version, SPARSE_HEIGHT_SCHEMA_VERSION);
    }

    fn formal_pages_for_dependencies(
        manifest: &SparseHeightManifest,
        identity: &PageSetIdentity,
    ) -> BTreeMap<ResidentPageKey, Arc<SparseHeightTile>> {
        identity
            .dependencies
            .iter()
            .map(|dependency| {
                (
                    dependency.page,
                    Arc::new(
                        SparseHeightTile::blank_at_level(
                            &manifest.spec,
                            dependency.page.level,
                            dependency.page.key,
                        )
                        .unwrap(),
                    ),
                )
            })
            .collect()
    }

    fn test_terrain_material() -> TerrainSurfaceMaterial {
        ExtendedMaterial {
            base: StandardMaterial::default(),
            extension: TerrainSurfaceExtension::new(TerrainSurfaceDescriptor {
                world_min: Vec2::ZERO,
                world_max: Vec2::ONE,
                weight_maps: [Handle::default(), Handle::default()],
                weight_map_count: 1,
                base_color_array: Handle::default(),
                arm_array: Handle::default(),
                normal_array: Handle::default(),
                index_map: Handle::default(),
                layer_table: Handle::default(),
                layers: [TerrainSurfaceLayer::default(); 8],
                layer_count: 1,
                residual_layer: 0,
                grass_layer: 0,
                grass_micro_normal_strength: 0.0,
                grass_variation_cell_meters: 1.0,
            }),
        }
    }

    #[test]
    fn clipmap_installs_height_fallback_before_surface_pbr_is_ready() {
        let mut app = App::new(); // 最小主世界即可覆盖材质所有权切换，无需启动RenderApp。
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<TerrainSurfaceMaterial>>()
            .init_resource::<Assets<ClipmapHeightMaterial>>()
            .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
            .init_resource::<SparseHeightPageRuntime>()
            .add_message::<AssetEvent<TerrainSurfaceMaterial>>()
            .add_systems(Update, sync_sparse_clipmap_surface);
        let standard = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.3, 0.4), // 兜底仍保留新地图原有白模外观。
                ..default()
            });
        let fallback_page = ClipmapFlatHeightPage {
            heights: Handle::default(),
            page_table: Handle::default(),
        };
        let extension = ClipmapHeightExtension::new_flat(
            ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(),
            UVec2::splat(512),
            &fallback_page,
        )
        .unwrap();
        {
            let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>();
            pages.extension = Some(extension); // 模拟高度清单已验证、地表纹理仍在加载。
            pages.extension_revision = 7;
        }
        let display = app
            .world_mut()
            .spawn((
                DynamicSparseClipmapPrimitive,
                MeshMaterial3d(standard.clone()),
            ))
            .id();

        app.update(); // 生产系统必须立即用带高度扩展的基础PBR替换普通平面材质。

        assert!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(display)
                .is_none()
        );
        let installed = app
            .world()
            .get::<MeshMaterial3d<ClipmapHeightMaterial>>(display)
            .expect("地表PBR未就绪时也必须安装高度位移材质");
        let material = app
            .world()
            .resource::<Assets<ClipmapHeightMaterial>>()
            .get(&installed.0)
            .expect("兜底材质资产必须由高度页会话持有");
        assert_eq!(material.base.base_color, Color::srgb(0.2, 0.3, 0.4));
        let pages = app.world().resource::<SparseHeightPageRuntime>();
        assert_eq!(pages.fallback_extension_revision, 7); // 同一页修订的稳定帧不再改写材质资产。
        assert_eq!(
            pages.fallback_material.as_ref().map(Handle::id),
            Some(installed.0.id())
        );
    }

    #[test]
    fn clipmap_keeps_a_height_fallback_when_surface_pbr_arrives_first() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<TerrainSurfaceMaterial>>()
            .init_resource::<Assets<ClipmapHeightMaterial>>()
            .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
            .init_resource::<SparseHeightPageRuntime>()
            .add_message::<AssetEvent<TerrainSurfaceMaterial>>()
            .add_systems(Update, sync_sparse_clipmap_surface);
        let surface = app
            .world_mut()
            .resource_mut::<Assets<TerrainSurfaceMaterial>>()
            .add(test_terrain_material());
        let extension = ClipmapHeightExtension::new_flat(
            ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(),
            UVec2::splat(512),
            &ClipmapFlatHeightPage {
                heights: Handle::default(),
                page_table: Handle::default(),
            },
        )
        .unwrap();
        {
            let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>();
            pages.extension = Some(extension);
            pages.extension_revision = 3;
        }
        let display = app
            .world_mut()
            .spawn((DynamicSparseClipmapPrimitive, MeshMaterial3d(surface)))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>(display)
                .is_some()
        ); // 完整地表正常升级为唯一组合材质。
        let retained_fallback = app
            .world()
            .resource::<SparseHeightPageRuntime>()
            .fallback_material
            .clone()
            .expect("完整地表路径也必须预留高度兜底资产");
        assert!(
            app.world()
                .resource::<Assets<ClipmapHeightMaterial>>()
                .get(&retained_fallback)
                .is_some()
        );

        app.world_mut()
            .entity_mut(display)
            .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
            .remove::<SparseClipmapSurfaceLease>(); // 模拟完整地表事务失败且普通白模组件已经不在实体上。
        app.update();

        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<ClipmapHeightMaterial>>(display)
                .expect("完整地表失效后必须无缝恢复高度兜底")
                .0
                .id(),
            retained_fallback.id()
        );
    }

    #[test]
    fn every_level_window_is_bounded_at_map_edges() {
        let spec = empty_manifest().spec;
        for focus in [[0, 0], spec.cells] {
            for level in 0..=spec.max_mip_level() {
                let pages = level_window_pages(
                    &spec,
                    focus,
                    level,
                    legacy_visible_half_cells(&spec, level),
                )
                .unwrap();
                assert!(!pages.is_empty());
                assert!(pages.len() <= 4); // 单层固定预算与全图页数无关。
                let counts = spec.mip_tile_counts(level).unwrap();
                assert!(
                    pages
                        .iter()
                        .all(|key| key.x < counts[0] && key.z < counts[1])
                );
            }
        }
    }

    #[test] // 扩大细层范围必须真实选择超过2×2页，并把显示计划纳入完整页身份。
    fn expanded_fine_band_selects_nine_pages_and_rejects_old_plan_identity() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],
            [520.0, 520.0], // 基础Mip恰有3×3页，便于证明分页器不再固定限制2×2。
            1.0,
            -32.0,
            64.0,
        )
        .unwrap();
        let authored = vec![
            feathrise_map_content::MapTerrainDisplayLod {
                outer_half_extent_meters: Some(193.0), // 向外量化为194格，加64格保护带后跨满三页。
                spacing_multiplier: 1,
            },
            feathrise_map_content::MapTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 8,
            },
        ];
        let plan = feathrise_map_content::compile_display_lod_plan(&spec, &authored).unwrap();
        let fine = plan
            .sample_ranges
            .iter()
            .find(|range| range.mip_level == 0)
            .unwrap();
        let focus = [260, 260];
        let selected = level_window_pages(&spec, focus, 0, fine.half_cells).unwrap();
        assert_eq!(selected.len(), 9); // 页窗口按真实半径扩展为3×3，而不是旧固定2×2。
        assert_eq!(selected.first(), Some(&SparseHeightTileKey { x: 0, z: 0 }));
        assert_eq!(selected.last(), Some(&SparseHeightTileKey { x: 2, z: 2 }));
        assert!(
            feathrise_map_content::worst_case_display_page_upload_bytes(&spec, &plan).unwrap()
                <= MAX_PAGE_UPLOAD_BYTES as u64
        ); // 扩大范围仍须先通过同一8MiB最坏预算。

        let mut manifest = SparseHeightManifest::empty(
            MapId::parse("MAP-expanded-display-pages").unwrap(),
            spec.clone(),
        )
        .unwrap();
        for key in &selected {
            insert_page(
                &mut manifest,
                ResidentPageKey {
                    level: 0,
                    key: *key,
                },
            );
        }
        manifest.validate().unwrap();
        let identity = desired_identity(
            &manifest,
            Entity::from_bits(91),
            7,
            focus,
            &plan,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            identity.pages.iter().filter(|page| page.level == 0).count(),
            9
        ); // 每个可见页及其共享边正式依赖都从真实窗口冻结。

        let alternate = vec![
            feathrise_map_content::MapTerrainDisplayLod {
                outer_half_extent_meters: Some(64.0),
                spacing_multiplier: 1,
            },
            feathrise_map_content::MapTerrainDisplayLod {
                outer_half_extent_meters: None,
                spacing_multiplier: 8,
            },
        ];
        let alternate_plan = feathrise_map_content::compile_display_lod_plan(&spec, &alternate).unwrap();
        let alternate_identity = desired_identity(
            &manifest,
            Entity::from_bits(91),
            7,
            focus,
            &alternate_plan,
            None,
            None,
        )
        .unwrap();
        assert_ne!(identity, alternate_identity); // 同地图、根、修订和焦点仍不能让旧页冒充新配置。
        let mut runtime = SparseHeightPageRuntime {
            desired: Some(alternate_identity),
            resident_identity: Some(identity),
            ..default()
        };
        assert!(!runtime.resident_window_is_current());
        runtime.resident_identity = runtime.desired.clone();
        assert!(runtime.resident_window_is_current());
    }

    #[test]
    fn page_build_uses_compact_hierarchy_table_and_shared_owner() {
        let mut manifest = empty_manifest();
        for page in [
            ResidentPageKey {
                level: 0,
                key: SparseHeightTileKey { x: 0, z: 0 },
            },
            ResidentPageKey {
                level: 0,
                key: SparseHeightTileKey { x: 1, z: 0 },
            },
            ResidentPageKey {
                level: 1,
                key: SparseHeightTileKey { x: 0, z: 0 },
            },
        ] {
            insert_page(&mut manifest, page);
        }
        manifest.validate().unwrap();
        let identity =
            legacy_desired_identity(&manifest, Entity::from_bits(1), 1, [0, 0], None, None)
                .unwrap();
        let formal = formal_pages_for_dependencies(&manifest, &identity);
        let built = build_page_set(
            identity,
            manifest.spec.clone(),
            formal,
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        assert!(built.height_bytes.len() + built.page_table_bytes.len() <= MAX_PAGE_UPLOAD_BYTES);
        assert_eq!(built.table_size, UVec2::new(256, 511)); // 256+128+...+1 行。
        assert_eq!(built.max_mip, 8);
    }

    #[test]
    fn rectangular_map_stacks_only_each_levels_real_z_rows() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],     // 世界原点保持简单，测试只关心两轴页数不对称。
            [8192.0, 64.0], // X 有二百五十六页，Z 只有两页。
            0.125,          // 触发合法最大 X 层级。
            -1024.0,        // 高度量程与正式空图一致。
            2048.0,         // 世界零高度位于量程内部。
        )
        .unwrap(); // 矩形地图属于正式新建规格。
        let (table_size, offsets) = page_table_layout(&spec).unwrap(); // 布局不能按 X 页数复制方形表。
        assert_eq!(spec.tile_counts(), [256, 2]); // 基础层两轴页数先证明测试前提。
        assert_eq!(spec.max_mip_level(), 8); // 最长轴仍需要八级父页。
        assert_eq!(table_size, UVec2::new(256, 10)); // Z 行为 2+1+1+1+1+1+1+1+1。
        assert_eq!(offsets, vec![0, 2, 3, 4, 5, 6, 7, 8, 9]); // Shader 的逐级行偏移必须完全一致。
    }

    #[test]
    fn finite_500m_map_pads_partial_top_mip_with_its_boundary_height() {
        let spec = SparseHeightSpec::new(
            [0.0, 0.0],
            [500.0, 500.0],
            0.125, // 用户测试图每轴4000格，最高父层为250格，不能填满固定256格页。
            -80.0,
            160.0, // 与 MAP-1790000205101 的实际高度编码范围一致。
        )
        .unwrap();
        let level = spec.max_mip_level();
        assert_eq!(level, 4);
        assert_eq!(spec.mip_cells(level), Some([250, 250]));
        let page = ResidentPageKey {
            level,
            key: SparseHeightTileKey { x: 0, z: 0 },
        };
        let mut tile = SparseHeightTile::blank_at_level(&spec, level, page.key).unwrap();
        tile.set([250, 250], spec.encode_height(12.0).unwrap())
            .unwrap(); // 最外侧真实点使用可辨认的最高值，便于验证固定页余量复制。
        let mut formal = BTreeMap::new();
        formal.insert(page, Arc::new(tile));
        let identity = PageSetIdentity {
            map_id: MapId::parse("MAP-clipmap-500m-edge").unwrap(),
            root: Entity::from_bits(500),
            manifest_revision: 1,
            live_revision: None,
            crater_revision: None, // 旧页测试沿用没有技能破坏的基线。
            has_live_pages: false,
            height_bounds: [0.0, 12.0],
            display_plan: legacy_display_plan(&spec), // 手工页候选仍需绑定同版显示计划。
            pages: vec![page],
            dependencies: Vec::new(),
        };

        let built = build_page_set(identity, spec, formal, BTreeMap::new(), BTreeMap::new())
            .expect("不整除的有限地图尾页必须以边缘值安全填满固定GPU页");

        let first_layer_final_texel = usize::try_from(PAGE_POINTS * PAGE_POINTS).unwrap() - 1; // 第二个合法数组层是空填充，不参与断言。
        let byte = first_layer_final_texel * 2;
        assert_eq!(
            u16::from_le_bytes([built.height_bytes[byte], built.height_bytes[byte + 1]]),
            u16::MAX
        ); // (256,256) 的填充样本复制真实 (250,250)，不会越界或变成平地。
    }

    #[test]
    fn maximum_all_level_window_upload_remains_below_eight_mib() {
        let mut manifest = empty_manifest();
        let focus = [manifest.spec.cells[0] / 2, manifest.spec.cells[1] / 2];
        for level in 0..=manifest.spec.max_mip_level() {
            let counts = manifest.spec.mip_tile_counts(level).unwrap();
            for key in level_window_pages(
                &manifest.spec,
                focus,
                level,
                legacy_visible_half_cells(&manifest.spec, level),
            )
            .unwrap()
            {
                for dz in 0..=1 {
                    for dx in 0..=1 {
                        let neighbor = SparseHeightTileKey {
                            x: key.x + dx,
                            z: key.z + dz,
                        };
                        if neighbor.x < counts[0]
                            && neighbor.z < counts[1]
                            && manifest_page_hash(
                                &manifest,
                                ResidentPageKey {
                                    level,
                                    key: neighbor,
                                },
                            )
                            .is_none()
                        {
                            insert_page(
                                &mut manifest,
                                ResidentPageKey {
                                    level,
                                    key: neighbor,
                                },
                            );
                        }
                    }
                }
            }
        }
        manifest.validate().unwrap();
        let identity =
            legacy_desired_identity(&manifest, Entity::from_bits(2), 1, focus, None, None).unwrap();
        assert!(identity.pages.len() <= 4 * 9);
        let formal = formal_pages_for_dependencies(&manifest, &identity);
        let built = build_page_set(
            identity,
            manifest.spec.clone(),
            formal,
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        assert!(built.height_bytes.len() + built.page_table_bytes.len() <= MAX_PAGE_UPLOAD_BYTES);
    }

    #[test]
    fn live_flat_tile_overrides_every_sampled_hierarchy_level_without_disk_asset() {
        let manifest = empty_manifest();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let tile = Arc::new(SparseHeightTile::blank(&manifest.spec, key).unwrap());
        let mut live = SparseHeightLiveOverride::default();
        live.retarget(Some(manifest.map_id.clone()), 4);
        live.publish_tiles(&manifest.map_id, 5, [Arc::clone(&tile)])
            .unwrap();
        let identity = legacy_desired_identity(
            &manifest,
            Entity::from_bits(3),
            1,
            [0, 0],
            Some(&live),
            None,
        )
        .unwrap();
        assert_eq!(identity.pages.len(), 9); // 基础页及八个祖先页都携带同一未保存修订。
        assert!(identity.dependencies.is_empty());
        assert_eq!(identity.live_revision, Some(5));
        let live_tiles = live_tiles_for_pages(&manifest.spec, &identity, &live.snapshot()).unwrap();
        let built = build_page_set(
            identity,
            manifest.spec.clone(),
            BTreeMap::new(),
            live_tiles,
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(built.layer_count, 9);
        assert_eq!(
            u32::from_le_bytes(built.page_table_bytes[0..4].try_into().unwrap()),
            1
        );
    }

    #[test]
    fn unsaved_height_is_visible_in_every_far_parent_page() {
        let manifest = empty_manifest();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let mut tile = SparseHeightTile::blank(&manifest.spec, key).unwrap();
        tile.set([0, 0], manifest.spec.encode_height(12.0).unwrap())
            .unwrap();
        let mut live = SparseHeightLiveOverride::default();
        live.retarget(Some(manifest.map_id.clone()), 7);
        live.publish_tiles(&manifest.map_id, 8, [Arc::new(tile)])
            .unwrap();
        let identity = legacy_desired_identity(
            &manifest,
            Entity::from_bits(4),
            1,
            [0, 0],
            Some(&live),
            None,
        )
        .unwrap();
        let live_tiles = live_tiles_for_pages(&manifest.spec, &identity, &live.snapshot()).unwrap();
        let built = build_page_set(
            identity,
            manifest.spec.clone(),
            BTreeMap::new(),
            live_tiles,
            BTreeMap::new(),
        )
        .unwrap();
        let layer_bytes = (PAGE_POINTS * PAGE_POINTS * 2) as usize;
        for layer in 0..=usize::from(manifest.spec.max_mip_level()) {
            let start = layer * layer_bytes;
            assert_eq!(
                u16::from_le_bytes(built.height_bytes[start..start + 2].try_into().unwrap()),
                u16::MAX
            ); // 全部父页的原点从同一未保存基础格点重建，远环不会等保存后才抬升。
        }
    }

    #[test]
    fn live_snapshot_page_worker_keeps_its_frozen_height_and_releases_old_tree_after_retarget() {
        let manifest = empty_manifest();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let mut original = SparseHeightTile::blank(&manifest.spec, key).unwrap();
        let expected = manifest.spec.encode_height(12.0).unwrap();
        original.set([0, 0], expected).unwrap();
        let original = Arc::new(original);
        let retired = Arc::downgrade(&original);
        let mut live = SparseHeightLiveOverride::default();
        live.retarget(Some(manifest.map_id.clone()), 4);
        live.publish_tiles(&manifest.map_id, 5, [original]).unwrap();
        let identity = legacy_desired_identity(&manifest, Entity::from_bits(104), 1, [0, 0], Some(&live), None).unwrap();
        let snapshot = live.snapshot();
        let pool = bevy::tasks::TaskPool::new();
        let spec = manifest.spec.clone();
        let mut pages = SparseHeightPageRuntime::default();
        let frozen_identity = identity.clone();
        pages.building = Some(identity.clone());
        pages.task = Some(pool.spawn(async move {
            build_page_set_with_live(identity, spec, BTreeMap::new(), snapshot, BTreeMap::new())
        })); // 与正式启动路径相同，仅主测试线程等待；运行时仍非阻塞check_ready。
        let mut replacement = SparseHeightTile::blank(&manifest.spec, key).unwrap();
        replacement.set([0, 0], manifest.spec.encode_height(24.0).unwrap()).unwrap();
        live.publish_tiles(&manifest.map_id, 6, [Arc::new(replacement)]).unwrap();
        let other = MapId::parse("MAP-live-worker-retarget").unwrap();
        live.retarget(Some(other.clone()), 7);
        pages.begin_target(other.clone(), Entity::from_bits(105), "other-height.json".into());
        assert!(pages.task.is_some(), "切图不能丢失真实worker或增开一个编码槽");
        let built = bevy::tasks::futures_lite::future::block_on(pages.task.take().unwrap()).unwrap();
        assert_eq!(built.identity, frozen_identity);
        let layer_bytes = (PAGE_POINTS * PAGE_POINTS * 2) as usize;
        let expected_height = manifest.spec.decode_height(expected);
        let (table_size, table_offsets) = page_table_layout(&manifest.spec).unwrap();
        assert_eq!(built.identity.pages.len(), usize::from(manifest.spec.max_mip_level()) + 1);
        for (layer, page) in built.identity.pages.iter().enumerate() {
            assert_eq!(page.level as usize, layer);
            assert_eq!(page.key, key);
            let start = layer * layer_bytes;
            let sample = u16::from_le_bytes(built.height_bytes[start..start + 2].try_into().unwrap());
            let table_y = table_offsets[usize::from(page.level)] + page.key.z;
            let table_index = (table_y as usize * table_size.x as usize + page.key.x as usize) * 16;
            let entry = std::array::from_fn::<_, 4, _>(|word| u32::from_le_bytes(
                built.page_table_bytes[table_index + word * 4..table_index + word * 4 + 4].try_into().unwrap()));
            assert_eq!(entry[0], layer as u32 + 1);
            // GPU每页按自身min/span重新量化，不能把UNORM码与作者全图高度码直接比较。
            // 连同页表解码回米制高度；12m与后来24m都编码为65535，但真实范围必须仍是旧版本。
            let sampled_height = f32::from_bits(entry[1]) + f32::from(sample) / 65535.0 * f32::from_bits(entry[2]);
            assert_eq!(sampled_height, expected_height,
                "旧快照的全部父页必须来自旧作者块，不能在worker回读新修订或另一地图");
        }
        assert_eq!(pages.map_id.as_ref(), Some(&other));
        assert!(pages.desired.is_none() && pages.cpu.is_none() && pages.prepared.is_none(),
            "带旧map/root/revision的完成值没有新地图采用资格");
        assert!(retired.upgrade().is_none(), "完整旧索引和所选作者块在worker结束前释放，不随完成值回主帧");
    }

    #[test]
    fn live_snapshot_page_worker_rejects_wrong_revision_map_or_tile_shape() {
        let manifest = empty_manifest();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let mut live = SparseHeightLiveOverride::default();
        live.retarget(Some(manifest.map_id.clone()), 1);
        live.publish_tiles(&manifest.map_id, 2, [Arc::new(SparseHeightTile::blank(&manifest.spec, key).unwrap())]).unwrap();
        let identity = legacy_desired_identity(&manifest, Entity::from_bits(106), 1, [0, 0], Some(&live), None).unwrap();
        let mut wrong_revision = identity.clone(); wrong_revision.live_revision = Some(3);
        assert!(build_page_set_with_live(wrong_revision, manifest.spec.clone(), BTreeMap::new(), live.snapshot(), BTreeMap::new()).is_err());
        let mut wrong_map = identity.clone(); wrong_map.map_id = MapId::parse("MAP-wrong-live-worker").unwrap();
        assert!(build_page_set_with_live(wrong_map, manifest.spec.clone(), BTreeMap::new(), live.snapshot(), BTreeMap::new()).is_err());
        let wrong_spec = SparseHeightSpec::new([0.0; 2], [64.0; 2], 1.0, -1024.0, 2048.0).unwrap();
        assert_ne!(wrong_spec.tile_shape(key), manifest.spec.tile_shape(key),
            "夹具使用真实合法规格，但65×65尾页不能冒充当前地图256×256内部页");
        live.publish_tiles(&manifest.map_id, 2, [Arc::new(SparseHeightTile::blank(&wrong_spec, key).unwrap())]).unwrap();
        assert!(build_page_set_with_live(identity, manifest.spec, BTreeMap::new(), live.snapshot(), BTreeMap::new()).is_err(),
            "持久索引只共享句柄，正式worker仍负责复验块形状，损坏页不能发布半图");
    }
}
