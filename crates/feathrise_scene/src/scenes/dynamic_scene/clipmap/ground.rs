//! Clipmap作者查询先与正式可见环带求交，再以局部基础作者页提供相机无关的最终高度。
#[cfg(test)] // 真实Mesh三角面作为独立查询参照。
#[path = "ground_tests.rs"] // 测试夹具不进入运行时。
mod tests; // 覆盖接缝、边界和分页等待。
#[cfg(test)]
#[path = "ground_surface_snap_tests.rs"]
mod surface_snap_tests; // 连续三维承托的网格吸附，不依赖高度包络或固定抬高射线。
#[path = "ground_frame.rs"]
mod frame_queries; // 倾斜对象保留世界竖直射线和连续承托层，不能把世界Y当作源Y。
use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use super::SparseHeightPageRuntime; // 正式高度采用边界。
use super::authority::{AuthoritativeHeightRuntime, AuthorityHeightSnapshot, AuthorityRequestSink}; // 作者精度页独立于当前可见环带窗口。
use crate::scenes::dynamic_scene::sparse_runtime::{SparseClipmapDisplay, SparseClipmapRuntime};
use bevy::prelude::*; // 只读资源和纯数学数据。
#[cfg(test)]
use feathrise_environment::vegetation::blade::GpuBladeClipmapBand; // 固定模板回归转换为正式环带描述。
use feathrise_environment::vegetation::blade::GpuBladeClipmapHeight; // 草根与笔刷共享同一几何参数。
use feathrise_scene_content::{ActiveDynamicScene, SceneId}; // 场景身份必须匹配。
use std::sync::Arc; // 查询共享最多一个局部页组，不复制整图。 // 唯一显示实体。

#[cfg(test)]
pub(crate) fn test_ground_snapshot() -> ClipmapGroundSnapshot {
    tests::snapshot()
}

#[cfg(test)]
pub(crate) fn test_authoring_ground_snapshot(
    scene_id: SceneId,
    root: Entity,
    live_revision: Option<u64>,
) -> ClipmapGroundSnapshot {
    // 仅本库测试可重新绑定真实坡面夹具；生产查询仍只能读取正式发布的不可变快照。
    let mut snapshot = tests::snapshot();
    snapshot.scene_id = scene_id.clone();
    snapshot.revision[0] = root.to_bits();
    snapshot.height.revision = snapshot.revision;
    snapshot.content_revision.root = root.to_bits();
    snapshot.content_revision.live = live_revision;
    if let Some(authority) = snapshot.authority.as_mut() {
        let authority = Arc::get_mut(authority).expect("新建夹具的作者页尚未共享");
        authority.identity.scene_id = scene_id;
        authority.identity.content = snapshot.content_revision;
    }
    snapshot
}

pub(super) struct GroundPageBytes {
    // 后台冻结的GPU上传字节；Arc共享且无可变接口。
    pub heights: Vec<u8>, // R16_UNORM逐层257²。
    pub table: Vec<u8>,   // RGBA32_UINT紧凑页表。
    pub width: u32,       // 页表宽度。
    pub height_bounds: [f32; 2], // 同一worker实际采样界；新候选绝不改变旧CPU/GPU版本的包围范围。
} // 内部类型不暴露可写字节。

#[derive(Resource, Default)] // 编辑器只读取当前正式快照。
pub struct DynamicClipmapGroundSurface {
    // 失效时清空，不能继承旧图落点。
    pub snapshot: Option<ClipmapGroundSnapshot>, // Clone只增加Arc引用。
    pub(super) authority_requests: AuthorityRequestSink, // 查询只合并至固定六十四页需求槽。
} // 查询任务由调用者统一single-flight。

/// 已采用地表的内容身份；相机换页和环带吸附不会改变它。
///
/// 【功能设计】Mesh 植被已经把世界 Y 冻结进实例变换，因此只能在作者高度、
/// 会话浅坑或场景根真实换代时重建当前常驻块。分页窗口和观察中心属于查询输入，
/// 不能冒充内容修改并让整片树木随视角移动反复重建。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipmapGroundContentRevision {
    pub(super) root: u64,
    pub(super) manifest: u64,
    pub(super) live: Option<u64>,
    pub(super) crater: Option<u64>,
    pub(super) topology: u64,
}

impl ClipmapGroundContentRevision {
    pub(crate) fn status_words(self) -> [u64; 5] {
        [
            self.root,
            self.manifest,
            self.live.unwrap_or(0),
            self.crater.unwrap_or(0),
            self.topology,
        ]
    }
}

/// 一份具体局部页和环带可用于查询的身份；缺页恢复后允许单块重试。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipmapGroundQueryRevision {
    revision: [u64; 3],
    missing_pages: [UVec4; 16],
    authority: u64, // 基础作者页到达或失败时只唤醒等待的局部消费者。
    ready: bool,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)] // 编辑器落笔必须排在完整地表发布之后。
pub struct ClipmapGroundPublishSet; // 避免在旧修订写入密度后才发现地表换代。

pub(crate) fn clear_clipmap_ground_surface(mut output: InstanceResMut<DynamicClipmapGroundSurface>) {
    output.snapshot = None;
    output.authority_requests.clear();
} // 离场即撤销旧场景快照，重开同名场景也不能误用。

#[derive(Clone)] // 小型身份与共享页数据。
pub struct ClipmapGroundSnapshot {
    author_stitches:Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,
    pub(super) holes: Arc<[feathrise_scene_content::SceneTerrainCutout]>,
    pub(crate) terrain_meshes: Arc<[crate::scenes::dynamic_scene::terrain_topology::TerrainMeshQuery]>,
    pub(super) hole_revision: u64,
    // 每次发布代表一个完整地表修订。
    pub scene_id: SceneId,                                          // 外部场景门。
    pub revision: [u64; 3],                                     // 根实体、已采用高度、吸附中心。
    pub missing_pages: [UVec4; 16], // 同内容窗口缺页也属于完整查询身份。
    pub ready: bool,                // 不同内容修订禁止回退平地。
    pub(super) content_revision: ClipmapGroundContentRevision, // 与视角无关的作者/玩法高度身份。
    height: GpuBladeClipmapHeight,  // 与草根完全相同的世界映射和环带。
    pages: Option<Arc<GroundPageBytes>>, // None仅表示正式采用的隐式平地。
    pub(super) authority: Option<Arc<AuthorityHeightSnapshot>>, // 局部Mip 0作者页，不随观察中心换代。
    pub(super) authority_requests: AuthorityRequestSink, // 后台快照也只能向当前身份提交有界需求。
} // 快照可安全发送至计算池。

#[derive(Debug, Clone, Copy, PartialEq)] // 结果不携带任何ECS修改。
pub enum ClipmapGroundHit {
    // 编辑器据此决定能否落笔。
    Hit(Vec3),        // 最近真实三角面交点。
    WaitingForHeight, // 涉及未采用高度，绝不替代成零平面。
    Miss,             // 射线不命中有限场景。
    Failed,           // 非法射线输入。
} // 身份核验由调用者在正式采用前完成。

pub(crate) fn sync_clipmap_ground_surface(
    frame: feathrise_scene_core::residency::InstanceFrame,
    objects: Query<&super::super::instances::TerrainObjectInstance>,
    native: Option<InstanceRes<super::super::voxel_objects::VoxelObjectRuntime>>,
    runtime: Option<InstanceRes<super::super::resources::DynamicSceneRuntimeState>>,
    // 页采用和显示中心更新后发布。
    active: InstanceRes<ActiveDynamicScene>,              // 当前场景。
    pages: InstanceRes<SparseHeightPageRuntime>,        // 已采用CPU/GPU共同页组。
    geometry: InstanceRes<SparseClipmapRuntime>,        // 当前环带。
    authority: InstanceRes<AuthoritativeHeightRuntime>, // 与当前正式内容同代的局部作者精度页。
    displays: InstanceQuery<&Transform, With<SparseClipmapDisplay>>, // 当前实际吸附中心。
    mut output: InstanceResMut<DynamicClipmapGroundSurface>, // 轻量原子替换。
    topology: Option<InstanceRes<super::super::terrain_topology::TerrainTopologyRuntime>>,
) {
    if frame.instance().and_then(|entity|objects.get(entity).ok()).is_some_and(|object|object.definition.voxel.is_some()) {
        if let Some(native)=native.as_deref() && let Some(root)=runtime.as_deref().and_then(|runtime|runtime.root) {
            let changed=output.snapshot.as_ref().is_none_or(|snapshot|snapshot.revision[2]!=native.generation || snapshot.revision[0]!=root.to_bits());
            if changed {output.snapshot=native.ground_snapshot(root);}
        }
        return; // 原生对象的真实BVH不能被没有高度页的Clipmap分支清空。
    }
    // 不启动独立任务，不扫描页像素。
    output.snapshot = (|| {
        // 任一身份缺失则撤销旧地表。
        let map = active.scene_id.as_ref()?; // 不跨场景保留。
        let (display, config) = geometry.blade_geometry(map)?; // 已采用几何。
        let transform = displays.get(display).ok()?; // 显示实体仍存在。
        let height = pages.blade_height(map, config, transform.translation.xz())?; // 同源GPU描述。
        let content_revision = pages.ground_content_revision()?; // 与当前已采用CPU页严格同代。
        Some(ClipmapGroundSnapshot {
            author_stitches:pages.author_stitches.clone(),
            holes: topology
                .as_deref()
                .map_or_else(Arc::default, |value| value.applied.clone()),
            terrain_meshes: topology
                .as_deref()
                .map_or_else(Arc::default, |value| value.surfaces.clone()),
            hole_revision: topology.as_deref().map_or(0, |value| value.revision),
            scene_id: map.clone(),
            revision: height.revision,
            missing_pages: height.missing_pages,
            ready: height.ready,
            content_revision,
            height,
            pages: pages.cpu.clone(),
            authority: authority.snapshot_for(content_revision),
            authority_requests: output.authority_requests.clone(),
        }) // 常数大小共享快照。
    })(); // 切换场景时自然清空。
} // 前台不等待射线计算。

impl ClipmapGroundSnapshot {
    pub(super) fn synchronize_stitches(&mut self,data:Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>,author:Arc<feathrise_environment::vegetation::blade::GpuBladeClipmapStitches>) {
        self.author_stitches=author;
        if *self.height.stitches==*data{return;}
        if let Some(range)=data.height_range(){self.height.height_min=self.height.height_min.min(range.x);self.height.height_max=self.height.height_max.max(range.y);}
        self.height.stitches=data;
        self.height.revision[2]=((u64::from(self.height.fine_center.x.to_bits())<<32)|u64::from(self.height.fine_center.y.to_bits()))^self.height.stitches.revision();
        self.revision=self.height.revision;
    }
    /// 原生体素仅复用三维CPU查询合同：零高度环带、无高度页和无权威平面。
    /// 环境窗口是实际驻留的作者源范围，不能把一个巨大盒体当作整张密集水深网格。
    pub(in crate::scenes::dynamic_scene) fn from_density_mesh(scene_id: SceneId, root: Entity,
        generation: u64, geometry: [u8; 32], shape: Option<avian3d::prelude::Collider>,
        min: Vec3, max: Vec3, step: f32, foci: &[Vec3], radius: f32) -> Result<Self, String> {
        use feathrise_environment::vegetation::blade::GpuBladeClipmapBand;
        let revision=[root.to_bits(),u64::from_le_bytes(geometry[..8].try_into().unwrap()),generation];
        let bounds=(min.xz(),max.xz());
        let holes=foci.iter().take(2).filter_map(|focus| {
            let a=(focus.xz()-Vec2::splat(radius)).max(bounds.0);
            let b=(focus.xz()+Vec2::splat(radius)).min(bounds.1);
            a.cmplt(b).all().then_some(feathrise_scene_content::SceneTerrainCutout {min:a.to_array(),max:b.to_array()})
        }).collect::<Vec<_>>().into();
        // 雕刻后的空网格同样是完整采用：查询明确Miss，水域/植被不能永久停在Loading。
        let terrain_meshes=shape.map(|shape|super::super::terrain_topology::TerrainMeshQuery::new(shape,Mat4::IDENTITY))
            .transpose()?.into_iter().collect::<Vec<_>>().into();
        Ok(Self {author_stitches:Default::default(),holes,terrain_meshes,
            hole_revision:0,scene_id,revision,missing_pages:[UVec4::ZERO;16],ready:true,
            content_revision:ClipmapGroundContentRevision {root:root.to_bits(),manifest:revision[1],live:None,crater:None,
                topology:u64::from_le_bytes(geometry[8..16].try_into().unwrap())},
            height:GpuBladeClipmapHeight {stitches: Default::default(),holes:[Vec4::ZERO;32],hole_count:0,atlas:Handle::default(),page_table:Handle::default(),
                world_min:bounds.0,world_max:bounds.1,cell_size:Vec2::splat(step.max(radius/64.0)),logical_cells:UVec2::ONE,max_mip:0,
                fine_center:foci.first().map_or(Vec2::ZERO,|p|p.xz()),bands:[GpuBladeClipmapBand::default();16],band_count:0,
                height_min:min.y,height_max:max.y,revision,missing_pages:[UVec4::ZERO;16],ready:true},
            pages:None,authority:None,authority_requests:Default::default()})
    }
    pub(in crate::scenes::dynamic_scene) fn is_density_surface(&self) -> bool {self.height.band_count==0 && self.authority.is_none()}
    pub(crate) fn base_cell_size(&self) -> Vec2 { self.height.cell_size }
    /// 已采用的三维覆盖域；派生水面只在这些域内按所选水位找洞底。
    pub fn volume_footprints(&self) -> &[feathrise_scene_content::SceneTerrainCutout] {
        &self.holes
    }
    /// 完整体积可以深入基础高度量程之外；后台对象索引需包含真实三维查询范围。
    pub fn three_dimensional_bounds(&self) -> Option<(Vec3, Vec3)> {
        self.terrain_meshes.iter().map(|mesh| mesh.bounds()).reduce(|(min,max),(a,b)| (min.min(a),max.max(b)))
    }
    /// 后台对象索引采用完整已发布地表界，包含超出旧v2量程的浮点高度与洞穴三角面。
    /// 这里只复制冻结标量；缺页仍由真实查询返回等待，包围盒不能授予作者写入权限。
    pub fn surface_bounds(&self) -> Option<(Vec3, Vec3)> {
        let height = (self.height.band_count > 0).then(|| (
            Vec3::new(self.height.world_min.x, self.height.height_min, self.height.world_min.y),
            Vec3::new(self.height.world_max.x, self.height.height_max, self.height.world_max.y),
        ));
        height.into_iter().chain(self.three_dimensional_bounds())
            .reduce(|(min, max), (a, b)| (min.min(a), max.max(b)))
    }
    /// 局部体积原型与基础作者格点对齐，不能用显示LOD决定密度接缝。
    pub fn authoritative_grid(&self) -> Option<(Vec2, Vec2)> {
        let authority = self.authority.as_ref()?;
        (authority.identity.content == self.content_revision).then(|| {
            (
                Vec2::from_array(authority.spec.world_min),
                Vec2::new(
                    authority.spec.actual_spacing_meters[0] as f32,
                    authority.spec.actual_spacing_meters[1] as f32,
                ),
            )
        })
    }
    pub(crate) fn content_revision(&self) -> ClipmapGroundContentRevision {
        let mut value = self.content_revision;
        value.topology ^= self.hole_revision.rotate_left(32);
        value
    }

    /// F12 作者工具只接受已经正式采用到 CPU/GPU 页组的同代实时高度。
    pub fn authoring_height_revision(&self) -> Option<u64> {
        self.content_revision.live
    }

    /// 局部基础页到达或淘汰时变化；辅助显示据此重试缺页，而非每帧重复后台采样。
    pub fn authoritative_page_revision(&self) -> Option<u64> {
        self.authority
            .as_ref()
            .filter(|authority| authority.identity.content == self.content_revision)
            .map(|authority| authority.revision)
    }

    /// 普通摆放分帧重贴地用固定大小内容身份识别根、清单、作者高度、浅坑和拓扑换代。
    pub fn content_revision_words(&self) -> [u64; 7] {
        let revision = self.content_revision;
        [
            revision.root,
            revision.manifest,
            revision.live.unwrap_or(0),
            u64::from(revision.live.is_some()),
            revision.crater.unwrap_or(0),
            u64::from(revision.crater.is_some()),
            revision.topology ^ self.hole_revision.rotate_left(32),
        ] // presence位避免把None与合法的第0修订混为同一内容身份。
    }

    /// 后台作者整平只编辑基础高度，局部仍有会话浅坑时应拒绝持久化整平。
    pub fn session_crater_intersects_rect(
        &self,
        minimum: Vec2,
        maximum: Vec2,
    ) -> Result<bool, String> {
        if !minimum.is_finite() || !maximum.is_finite() || minimum.cmpgt(maximum).any() {
            return Err("整平范围无效".into());
        }
        let authority = self
            .authority
            .as_ref()
            .filter(|authority| authority.identity.content == self.content_revision)
            .ok_or("当前精确基础高度身份尚未就绪")?;
        Ok(authority.craters.values().any(|stamps| {
            stamps
                .iter()
                .any(|stamp| stamp.intersects_rect(minimum, maximum))
        }))
    }

    pub(crate) fn query_revision(&self) -> ClipmapGroundQueryRevision {
        ClipmapGroundQueryRevision {
            revision: self.revision,
            missing_pages: self.missing_pages,
            authority: self.authority.as_ref().map_or(0, |value| value.revision),
            ready: self.ready,
        }
    }

    // 射线遍历只在后台执行，固定64槽连续性检查可用于前台落笔门。
    pub(crate) fn max_mip(&self) -> u32 {
        self.height.max_mip
    } // 缺页掩码按真实场景级数，不能受旧全平图集钳制。

    pub fn segment_is_known(&self, from: Vec3, to: Vec3) -> bool {
        // 固定64槽保守检查，允许落笔前在主线程拒绝跨缺页补画。
        if !self.ready || !from.is_finite() || !to.is_finite() {
            return false;
        } // 未采用内容或非法端点没有连续性资格。
        let h = &self.height; // 使用权威场景格距和真实最高Mip。
        for level in 0..=h.max_mip.min(15) {
            // 最多16级，不遍历全图页表。
            let step = h.cell_size * (1_u32 << level) as f32; // 缺页所属Mip的世界格距。
            for code in self.missing_pages[level as usize].to_array() {
                // 单级最多四个非平缺页。
                if code == 0 {
                    continue;
                } // 空槽不是未知页。
                let key = code - 1; // 恢复世界固定页键。
                let tile = Vec2::new((key & 0xffff) as f32, (key >> 16) as f32); // 两轴分别解码。
                let min = h.world_min + tile * (256.0 * step) - step; // 扩一格覆盖三角面顶点支持及页边插值。
                let max = h.world_min + (tile + Vec2::ONE) * (256.0 * step) + step; // 保守范围允许少量提前断笔。
                let origin = Vec3::new(from.x, 0.0, from.z); // 作者笔画半径和插值采用世界XZ语义。
                let direction = Vec3::new(to.x - from.x, 0.0, to.z - from.z); // 参数t在[0,1]代表有限补画段。
                if ray_box(
                    origin,
                    direction,
                    Vec3::new(min.x, -1.0, min.y),
                    Vec3::new(max.x, 1.0, max.y),
                )
                .is_some_and(|(entry, _)| entry <= 1.0)
                {
                    return false;
                } // 任一可能未知的中间区域都切断连接。
            } // 无额外IO或像素读取。
        } // 全部局部缺页检查完成。
        self.authoritative_segment_is_known(from.xz(), to.xz()) // 最终落点还必须由同一批基础作者页连续覆盖。
    } // 只决定端点之间能否连接，不否定已精确命中的端点。

    /// 在相机无关的基础作者网格上取得最终世界 Y；缺页会自动合并局部预取需求。
    pub fn sample_authoritative_surface(&self, world_xz: Vec2) -> ClipmapGroundHit {
        if !world_xz.is_finite(){return ClipmapGroundHit::Failed;}
        if self.is_density_surface() {
            if !self.ready {return ClipmapGroundHit::WaitingForHeight;}
            return self.three_dimensional_hit(Vec3::new(world_xz.x,self.height.height_max+1.0,world_xz.y),Vec3::NEG_Y,
                self.height.height_max-self.height.height_min+2.0).map_or(ClipmapGroundHit::Miss,|(point,normal)| {
                    if normal.y>0.05 {ClipmapGroundHit::Hit(point)} else {ClipmapGroundHit::Miss}
                });
        }
        if self.holes.iter().any(|hole| hole.contains(world_xz)) {
            return ClipmapGroundHit::Miss;
        }
        if !self.ready {
            return ClipmapGroundHit::WaitingForHeight;
        }
        if world_xz.cmplt(self.height.world_min).any()||world_xz.cmpgt(self.height.world_max).any(){return ClipmapGroundHit::Miss;}
        if let Some(height)=self.author_stitches.boundary_height(world_xz,self.height.world_min,self.height.world_max) {
            return ClipmapGroundHit::Hit(Vec3::new(world_xz.x,height,world_xz.y)); // 已冻结完整边无需再等待原作者页。
        }
        let Some(authority) = self.authority.as_ref() else {
            return ClipmapGroundHit::WaitingForHeight;
        };
        if authority.identity.content != self.content_revision {
            return ClipmapGroundHit::WaitingForHeight;
        }
        let Ok(ready) = self.request_authoritative_rect(world_xz, world_xz) else {
            return ClipmapGroundHit::Failed;
        };
        if !ready {
            return ClipmapGroundHit::WaitingForHeight;
        }
        match authority.sample_surface(world_xz) {
            ClipmapGroundHit::Hit(mut point)=>{point.y=self.author_stitches.height(point.xz(),point.y,self.height.world_min,self.height.world_max);ClipmapGroundHit::Hit(point)},
            other=>other,
        }
    }

    /// 洞穴同一 X/Z 可有地板与顶板，真实 Mesh 命中保持完整 Y，不再吸回室外包络。
    pub fn sample_authoritative_hit(&self, point: Vec3) -> ClipmapGroundHit {
        if self.is_three_dimensional_surface(point) {
            ClipmapGroundHit::Hit(point)
        } else {
            self.sample_authoritative_surface(point.xz())
        }
    }

    pub fn is_three_dimensional_surface(&self, point: Vec3) -> bool {
        point.is_finite()
            && self
                .terrain_meshes
                .iter()
                .any(|mesh| mesh.contains_surface(point))
    }
    /// 网格吸附沿原命中层的连续共享边找目标Y；上坡和下坡都不得改选洞顶或下层。
    /// 仅在后台调用，复用当前完整地形快照和BVH；断边没有承托时返回Miss，不回退室外高度。
    pub fn sample_support_along_surface(&self, point: Vec3, target_xz: Vec2) -> ClipmapGroundHit {
        if !point.is_finite() || !target_xz.is_finite() {
            return ClipmapGroundHit::Failed;
        }
        if !self.ready {
            return ClipmapGroundHit::WaitingForHeight;
        }
        if !self.is_three_dimensional_surface(point) {
            // 基础高度同样保持原承托层；密度地形绝不能借最高垂射线猜测另一层。
            if self.is_density_surface() {
                return ClipmapGroundHit::Miss;
            }
            let base = match self.sample_authoritative_surface(point.xz()) {
                ClipmapGroundHit::Hit(base) => base,
                status => return status,
            };
            if base.distance(point) > 0.001 {
                return ClipmapGroundHit::Miss; // 原落点必须属于同代基础作者面。
            }
            // 已知洞口切断承托，不能从基础高度一侧直接吸到另一侧。
            if self.holes.iter().any(|hole| {
                ray_box(Vec3::new(point.x, 0.0, point.z),
                    Vec3::new(target_xz.x - point.x, 0.0, target_xz.y - point.z),
                    Vec3::new(hole.min[0], -1.0, hole.min[1]),
                    Vec3::new(hole.max[0], 1.0, hole.max[1]))
                    .is_some_and(|(entry, _)| entry <= 1.0)
            }) {
                return ClipmapGroundHit::Miss;
            }
            let target = match self.sample_authoritative_surface(target_xz) {
                ClipmapGroundHit::Hit(target) => target,
                status => return status,
            };
            let Ok(ready) = self.request_authoritative_rect(point.xz().min(target_xz), point.xz().max(target_xz)) else {
                return ClipmapGroundHit::Failed;
            };
            if !ready || !self.segment_is_known(point, target) {
                return ClipmapGroundHit::WaitingForHeight; // 中间缺页也提交有界需求，恢复后按身份重试。
            }
            return ClipmapGroundHit::Hit(target);
        }
        let mut triangles = Vec::new();
        for mesh in self.terrain_meshes.iter() {
            mesh.append_surface_triangles(point.xz(), target_xz, &mut triangles);
        }
        feathrise_terrain::follow_terrain_surface(point, target_xz, triangles)
            .map_or(ClipmapGroundHit::Miss, ClipmapGroundHit::Hit)
    }
    /// 已采用三维表面的局部查询；供洞内种植、水深和建筑选择指定层，禁止投回最高高度。
    /// 此方法只访问最多32份已冻结BVH，批量调用必须放到后台任务。
    pub fn three_dimensional_hit(
        &self,
        origin: Vec3,
        direction: Vec3,
        maximum: f32,
    ) -> Option<(Vec3, Vec3)> {
        if !self.ready
            || !origin.is_finite()
            || !direction.is_finite()
            || direction.length_squared() < 1e-12
            || !maximum.is_finite()
            || maximum <= 0.0
        {
            return None;
        }
        let direction = direction.normalize(); // 最大距离按世界米解释；对象逆变换后的射线保留这个参数尺度。
        self.terrain_meshes
            .iter()
            .filter_map(|mesh| mesh.ray_normal(origin, direction, maximum))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(distance, normal)| (origin + direction * distance, normal))
    }

    /// 同一XZ可以有多个洞层；以参考Y寻找下方承托面，适合洞内水面与固定高度摆放。
    pub fn sample_surface_below(&self, point: Vec3, maximum: f32) -> ClipmapGroundHit {
        if !point.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
            return ClipmapGroundHit::Failed;
        }
        if let Some((hit, _)) = self.three_dimensional_hit(point, Vec3::NEG_Y, maximum) {
            return ClipmapGroundHit::Hit(hit);
        }
        self.exterior_below(point, maximum)
    }
    /// 水面与地基必须由朝上的实体面承托；不能穿过洞顶背面再寻找更低的地板。
    pub fn sample_support_below(&self, point: Vec3, maximum: f32) -> ClipmapGroundHit {
        if !point.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
            return ClipmapGroundHit::Failed;
        }
        if let Some((hit, normal)) = self.three_dimensional_hit(point, Vec3::NEG_Y, maximum) {
            return if normal.y > 0.05 {
                ClipmapGroundHit::Hit(hit)
            } else {
                ClipmapGroundHit::Miss
            };
        }
        self.exterior_below(point, maximum)
    }
    fn exterior_below(&self, point: Vec3, maximum: f32) -> ClipmapGroundHit {
        if self.holes.iter().any(|hole| hole.contains(point.xz())) {
            return ClipmapGroundHit::Miss;
        }
        match self.sample_authoritative_surface(point.xz()) {
            ClipmapGroundHit::Hit(hit) if hit.y <= point.y && point.y - hit.y <= maximum => {
                ClipmapGroundHit::Hit(hit)
            }
            ClipmapGroundHit::Hit(_) => ClipmapGroundHit::Miss,
            other => other,
        }
    }
    fn center(&self, level: u32) -> Vec2 {
        // 与height.wgsl stable_world_xz一致。
        let h = &self.height; // 参数唯一来源。
        let step = h.cell_size * (1_u32 << level) as f32; // 几何层不钳制到高度Mip。
        // 与GPU使用同一个未钳制中心，拾取不能退回每个对象自己的近场。
        h.world_min + ((h.fine_center - h.world_min) / step + Vec2::splat(0.5)).floor() * step
    } // 内缘使用前一级中心。

    fn sample(&self, point: Vec2, level: u32) -> Option<f32> {
        if let Some(height)=self.height.stitches.boundary_height(point,self.height.world_min,self.height.world_max){return Some(height);}
        self.sample_base(point,level).map(|base|self.height.stitches.height(point,base,self.height.world_min,self.height.world_max))
    }
    fn sample_base(&self, point: Vec2, level: u32) -> Option<f32> {
        // 解码当前GPU页，None专指缺页。
        let h = &self.height; // 不读取未来作者高度。
        if point.cmplt(h.world_min).any() || point.cmpgt(h.world_max).any() {
            return Some(0.0);
        } // 顶点着色器场景外高度为零，片元再裁剪。
        let level = level.min(h.max_mip); // 与请求高度Mip一致。
        let scale = (1_u32 << level) as f32; // 当前页格距。
        let logical = ((point - h.world_min) / h.cell_size)
            .clamp(Vec2::ZERO, h.logical_cells.as_vec2())
            / scale; // 受限坐标。
        let counts = (h.logical_cells.as_vec2() / (256.0 * scale))
            .ceil()
            .as_uvec2(); // 每级真实页数。
        let tile = (logical / 256.0)
            .floor()
            .as_uvec2()
            .min(counts - UVec2::ONE); // 最远边归最后页。
        let code = (tile.x | (tile.y << 16)) + 1; // 与Blade缺页协议一致。
        if h.missing_pages[level as usize].to_array().contains(&code) {
            return None;
        } // 未加载非平页不能当平地。
        let Some(pages) = &self.pages else {
            return Some(0.0);
        }; // 已采用的空白地形。
        let row: u32 = (0..level)
            .map(|l| (h.logical_cells.y as f32 / (256.0 * (1_u32 << l) as f32)).ceil() as u32)
            .sum(); // 紧凑纵向布局。
        let offset = ((row + tile.y) * pages.width + tile.x) as usize * 16; // RGBA32入口。
        let Some(entry) = pages.table.get(offset..offset + 16) else {
            return Some(0.0);
        }; // Shader同样对旧小页表回退零。
        let words: [u32; 4] = std::array::from_fn(|i| {
            u32::from_le_bytes(entry[i * 4..i * 4 + 4].try_into().unwrap())
        }); // 无损整数读取。
        if words[0] == 0 {
            return Some(0.0);
        } // 隐式未编辑页。
        let local = (logical - tile.as_vec2() * 256.0).clamp(Vec2::ZERO, Vec2::splat(256.0)); // 页内线性过滤坐标。
        let lo = local.floor().as_uvec2(); // 左下texel。
        let hi = (lo + UVec2::ONE).min(UVec2::splat(256)); // 右上边界钳制。
        let raw = |x: u32, z: u32| {
            let at = (((words[0] - 1) * 257 * 257 + z * 257 + x) * 2) as usize;
            u16::from_le_bytes([pages.heights[at], pages.heights[at + 1]]) as f32 / 65535.0
        }; // 与R16_UNORM纹理转换一致。
        let f = local - lo.as_vec2(); // 半格顶点必须双线性采样。
        let a = raw(lo.x, lo.y).lerp(raw(hi.x, lo.y), f.x); // 下边。
        let b = raw(lo.x, hi.y).lerp(raw(hi.x, hi.y), f.x); // 上边。
        Some(f32::from_bits(words[1]) + a.lerp(b, f.y) * f32::from_bits(words[2])) // 每页min/span恢复高度。
    } // 所有输入与真正上传字节相同。

    /// 以固定环带层数和单格三角形数量查询世界点所在的正式可见曲面。
    ///
    /// 【功能设计】互动、点选和雨水代理都读取与渲染完全相同的普通格/五三角
    /// 接缝，不能用作者高度图的双线性点值近似实际三角面。查询最多访问十六个环带；
    /// 拼接边每层至多十六组逆映射、每组九个邻格，取最高有效正面，成本不随场景总格数增长。
    pub fn sample_surface(&self, world_xz: Vec2) -> ClipmapGroundHit {
        if self.holes.iter().any(|hole| hole.contains(world_xz)) {
            return ClipmapGroundHit::Miss;
        }
        if !world_xz.is_finite() {
            return ClipmapGroundHit::Failed;
        }
        if !self.ready {
            return ClipmapGroundHit::WaitingForHeight;
        }
        let h = &self.height;
        if world_xz.cmplt(h.world_min).any() || world_xz.cmpgt(h.world_max).any() {
            return ClipmapGroundHit::Miss;
        }

        let mut nearest:Option<Vec3>=None;let mut missing=false;
        let mut record=|hit:ClipmapGroundHit| {match hit {
            ClipmapGroundHit::Hit(p)=>{if nearest.is_none_or(|old|p.y>old.y){nearest=Some(p);}},
            ClipmapGroundHit::WaitingForHeight=>missing=true,_=>{},
        }}; // 与深度缓冲一致，收敛边角取最高有效正面，不能按候选遍历顺序选表面。
        for band in h.bands.iter().take(h.band_count as usize) {
            let level = band.geometry_mip;
            let step = h.cell_size * (1_u32 << level) as f32;
            let center = self.center(level);
            for candidate_index in 0..if h.stitches.active(){16}else{1} {
            let candidate=h.stitches.candidate(world_xz,h.world_min,h.world_max,step,center,candidate_index);
            if candidate_index%4>0&&(candidate.x-world_xz.x).abs()<0.000001||candidate_index/4>0&&(candidate.y-world_xz.y).abs()<0.000001 {continue;}
            for offset in [IVec2::ZERO,IVec2::new(-1,-1),IVec2::new(0,-1),IVec2::new(1,-1),IVec2::new(-1,0),IVec2::new(1,0),IVec2::new(-1,1),IVec2::new(0,1),IVec2::new(1,1)] {
            let cell=((candidate-center)/step).floor().as_ivec2()+offset;
            let x=cell.x;let z=cell.y;
            let inner = band.inner_half_cells.as_ivec2();
            let outer = band.outer_half_cells.as_ivec2();
            if x < -outer.x || x >= outer.x || z < -outer.y || z >= outer.y {
                continue;
            }
            let in_x = (-inner.x..inner.x).contains(&x);
            let in_z = (-inner.y..inner.y).contains(&z);
            if inner.cmpgt(IVec2::ZERO).all() && in_x && in_z {
                continue;
            }

            let corner = |px: f32, pz: f32, fine: bool| {
                let lod = if fine { level.saturating_sub(1) } else { level };
                (h.stitches.position(Vec2::new(px, pz) * step + self.center(lod),h.world_min,h.world_max), lod)
            };
            let edge = |px: i32, pz: i32| {
                inner.cmpgt(IVec2::ZERO).all()
                    && ((px.abs() == inner.x && pz.abs() <= inner.y)
                        || (pz.abs() == inner.y && px.abs() <= inner.x))
            };
            let corners = [
                corner(x as f32, z as f32, edge(x, z)),
                corner(x as f32, (z + 1) as f32, edge(x, z + 1)),
                corner((x + 1) as f32, (z + 1) as f32, edge(x + 1, z + 1)),
                corner((x + 1) as f32, z as f32, edge(x + 1, z)),
            ];
            let side = if !inner.cmpgt(IVec2::ZERO).all() {
                None
            } else if x == inner.x && in_z {
                Some(0)
            } else if z + 1 == -inner.y && in_x {
                Some(1)
            } else if x + 1 == -inner.x && in_z {
                Some(2)
            } else if z == inner.y && in_x {
                Some(3)
            } else {
                None
            };

            let visit = |points: [(Vec2, u32); 3]| -> Option<ClipmapGroundHit> {
                if h.stitches.active()&&(points[1].0-points[0].0).perp_dot(points[2].0-points[0].0)>=0.0{return None;}
                let weights = point_triangle_weights(world_xz, points.map(|point| point.0))?;
                let heights = points.map(|(point, lod)| self.sample(point, lod));
                let [Some(a), Some(b), Some(c)] = heights else {
                    return Some(ClipmapGroundHit::WaitingForHeight);
                };
                let y = a * weights.x + b * weights.y + c * weights.z;
                Some(ClipmapGroundHit::Hit(Vec3::new(world_xz.x, y, world_xz.y)))
            };

            if let Some(side) = side {
                let mid = match side {
                    0 => corner(x as f32, z as f32 + 0.5, true),
                    1 => corner(x as f32 + 0.5, z as f32 + 1.0, true),
                    2 => corner(x as f32 + 1.0, z as f32 + 0.5, true),
                    _ => corner(x as f32 + 0.5, z as f32, true),
                };
                let cell_center = corner(x as f32 + 0.5, z as f32 + 0.5, false);
                for edge_index in 0..4 {
                    if edge_index == side {
                        if let Some(hit) = visit([cell_center, corners[edge_index], mid]) {
                            if !h.stitches.active(){return hit;}record(hit);
                        }
                        if let Some(hit) = visit([cell_center, mid, corners[(edge_index + 1) % 4]])
                        {
                            if !h.stitches.active(){return hit;}record(hit);
                        }
                    } else if let Some(hit) = visit([
                        cell_center,
                        corners[edge_index],
                        corners[(edge_index + 1) % 4],
                    ]) {
                        if !h.stitches.active(){return hit;}record(hit);
                    }
                }
            } else {
                if let Some(hit) = visit([corners[0], corners[1], corners[3]]) {
                    if !h.stitches.active(){return hit;}record(hit);
                }
                if let Some(hit) = visit([corners[3], corners[1], corners[2]]) {
                    if !h.stitches.active(){return hit;}record(hit);
                }
            }
        }
            }}
        nearest.map_or(if missing {ClipmapGroundHit::WaitingForHeight}else{ClipmapGroundHit::Miss},ClipmapGroundHit::Hit)
    }

    pub fn ray_intersection(&self, origin: Vec3, direction: Vec3) -> ClipmapGroundHit {
        let height = self.height_ray_intersection(origin, direction);
        if !self.ready
            || !origin.is_finite()
            || !direction.is_finite()
            || direction.length_squared() < 1e-12
        {
            return height;
        }
        let maximum = match height {
            ClipmapGroundHit::Hit(point) => point.distance(origin) / direction.length(),
            ClipmapGroundHit::WaitingForHeight | ClipmapGroundHit::Failed => return height,
            _ => 1_000_000.0,
        };
        let distance = self
            .terrain_meshes
            .iter()
            .filter_map(|mesh| mesh.ray(origin, direction, maximum))
            .min_by(f32::total_cmp);
        distance.map_or(height, |distance| {
            ClipmapGroundHit::Hit(origin + direction * distance)
        })
    }

    pub fn height_ray_intersection(&self, origin: Vec3, direction: Vec3) -> ClipmapGroundHit {
        // 只能由single-flight后台调用。
        if !origin.is_finite() || !direction.is_finite() || direction.length_squared() < 1e-12 {
            return ClipmapGroundHit::Failed;
        } // 拒绝无效数学输入。
        if !self.ready {
            return ClipmapGroundHit::WaitingForHeight;
        } // 不同修订不能投射。
        let h = &self.height; // 固定上限的可变环带描述。
        let mut padding=Vec2::ZERO;
        for (edge,info) in h.stitches.edges.iter().enumerate(){let axis=if edge<2 {1}else{0};padding[axis]=padding[axis].max(info.y*info.w*0.5);}
        let mut nearest = f32::INFINITY; // 最近正向交点。
        let mut unknown = f32::INFINITY; // 可能挡住当前命中的缺页最早位置。
        for band in h.bands.iter().take(h.band_count as usize) {
            // 级数最多16，不随全图细格数增长。
            let level = band.geometry_mip; // UV_1直接使用的几何Mip。
            let step = h.cell_size * (1_u32 << level) as f32; // 当前几何格距。
            let center = self.center(level); // 当前环带世界中心。
            let inner = band.inner_half_cells.as_ivec2(); // 两轴独立中央洞口半径。
            let outer = band.outer_half_cells.as_ivec2(); // 两轴独立外边界。
            for z in -outer.y..outer.y {
                // 矩形环带行，先做保守行包围筛选。
                let row_min = Vec3::new(
                    center.x - (outer.x as f32 + 1.0) * step.x-padding.x,
                    h.height_min.min(0.0),
                    center.y + (z as f32 - 1.0) * step.y-padding.y,
                ); // 接缝最多移动半个粗格。
                let row_max = Vec3::new(
                    center.x + (outer.x as f32 + 1.0) * step.x+padding.x,
                    h.height_max.max(0.0),
                    center.y + (z as f32 + 2.0) * step.y+padding.y,
                ); // 包含地形量程。
                let Some((start, end)) = ray_box(origin, direction, row_min, row_max) else {
                    continue;
                }; // 排除整行。
                let from = origin.x + direction.x * start; // 射线穿行的X范围。
                let to = origin.x + direction.x * end; // 支持斜射线。
                let x0 = (((from.min(to) - center.x-padding.x) / step.x).floor() as i32 - 2).max(-outer.x);
                let x1 = (((from.max(to) - center.x+padding.x) / step.x).ceil() as i32 + 2).min(outer.x - 1);
                for x in x0..=x1 {
                    // 仅查询候选格。
                    let in_x = (-inner.x..inner.x).contains(&x);
                    let in_z = (-inner.y..inner.y).contains(&z);
                    if inner.cmpgt(IVec2::ZERO).all() && in_x && in_z {
                        continue;
                    } // 环带洞口不产生面。
                    let corner = |px: f32, pz: f32, fine: bool| {
                        let lod = if fine { level.saturating_sub(1) } else { level };
                        (h.stitches.position(Vec2::new(px, pz) * step + self.center(lod),h.world_min,h.world_max), lod)
                    }; // 内缘顶点沿细层中心。
                    let edge = |px: i32, pz: i32| {
                        inner.cmpgt(IVec2::ZERO).all()
                            && ((px.abs() == inner.x && pz.abs() <= inner.y)
                                || (pz.abs() == inner.y && px.abs() <= inner.x))
                    }; // 矩形内缘标记。
                    let corners = [
                        corner(x as f32, z as f32, edge(x, z)),
                        corner(x as f32, (z + 1) as f32, edge(x, z + 1)),
                        corner((x + 1) as f32, (z + 1) as f32, edge(x + 1, z + 1)),
                        corner((x + 1) as f32, z as f32, edge(x + 1, z)),
                    ]; // 顺时针四角。
                    let side = if !inner.cmpgt(IVec2::ZERO).all() {
                        None
                    } else if x == inner.x && in_z {
                        Some(0)
                    } else if z + 1 == -inner.y && in_x {
                        Some(1)
                    } else if x + 1 == -inner.x && in_z {
                        Some(2)
                    } else if z == inner.y && in_x {
                        Some(3)
                    } else {
                        None
                    }; // 与正式五角接缝同拓扑。
                    let mut visit = |points: [(Vec2, u32); 3]| {
                        // 一次精确三角面求交。
                        if h.stitches.active()&&(points[1].0-points[0].0).perp_dot(points[2].0-points[0].0)>=0.0{return;}
                        if (points[1].0 - points[0].0)
                            .perp_dot(points[2].0 - points[0].0)
                            .abs()
                            < 1e-9
                        {
                            return;
                        } // 吸附接缝零投影面不能落草，也不能造成虚假缺页等待。
                        let min = points
                            .iter()
                            .fold(Vec2::splat(f32::INFINITY), |v, p| v.min(p.0)); // 投影包围。
                        let max = points
                            .iter()
                            .fold(Vec2::splat(f32::NEG_INFINITY), |v, p| v.max(p.0)); // 投影包围。
                        let Some((entry, _)) = ray_box(
                            origin,
                            direction,
                            Vec3::new(min.x, h.height_min.min(0.0), min.y),
                            Vec3::new(max.x, h.height_max.max(0.0), max.y),
                        ) else {
                            return;
                        }; // 在采样前排除非候选面。
                        let heights = points.map(|(p, l)| self.sample(p, l)); // 顶点取高而非直接双线性取交点。
                        let [Some(a), Some(b), Some(c)] = heights else {
                            unknown = unknown.min(entry);
                            return;
                        }; // 缺页可能更近则禁止落笔。
                        let vertices = [
                            Vec3::new(points[0].0.x, a, points[0].0.y),
                            Vec3::new(points[1].0.x, b, points[1].0.y),
                            Vec3::new(points[2].0.x, c, points[2].0.y),
                        ]; // 与可见面相同三个顶点。
                        if let Some(t) = ray_triangle(origin, direction, vertices) {
                            let p = (origin + direction * t).xz();
                            if p.cmpge(h.world_min).all()
                                && p.cmple(h.world_max).all()
                                && !self.holes.iter().any(|hole| hole.contains(p))
                            {
                                nearest = nearest.min(t);
                            }
                        } // 场景外片元与拾取同时裁剪。
                    }; // 不分配三角面数组。
                    if let Some(side) = side {
                        // 一格五角扇形过渡。
                        let mid = match side {
                            0 => corner(x as f32, z as f32 + 0.5, true),
                            1 => corner(x as f32 + 0.5, z as f32 + 1.0, true),
                            2 => corner(x as f32 + 1.0, z as f32 + 0.5, true),
                            _ => corner(x as f32 + 0.5, z as f32, true),
                        }; // 内缘半格仍采细Mip。
                        let center = corner(x as f32 + 0.5, z as f32 + 0.5, false); // 扇心使用粗Mip。
                        for edge in 0..4 {
                            if edge == side {
                                visit([center, corners[edge], mid]);
                                visit([center, mid, corners[(edge + 1) % 4]]);
                            } else {
                                visit([center, corners[edge], corners[(edge + 1) % 4]]);
                            }
                        } // 五个实际三角形。
                    } else {
                        visit([corners[0], corners[1], corners[3]]);
                        visit([corners[3], corners[1], corners[2]]);
                    } // 常规对角线与模板一致。
                } // 下一候选格。
            } // 下一环带行。
        } // 所有可见环带完成。
        if unknown.is_finite() && unknown <= nearest {
            ClipmapGroundHit::WaitingForHeight
        } else if nearest.is_finite() {
            ClipmapGroundHit::Hit(origin + direction * nearest)
        } else {
            ClipmapGroundHit::Miss
        } // 最近地面或明确等待。
    } // 无IO、无全图分配、无逐株工作。
} // 只读地表接口。

fn point_triangle_weights(point: Vec2, triangle: [Vec2; 3]) -> Option<Vec3> {
    let area = (triangle[1] - triangle[0]).perp_dot(triangle[2] - triangle[0]);
    if area.abs() < 1e-9 {
        return None; // 吸附后退化的接缝面没有可查询面积。
    }
    let a = (triangle[1] - point).perp_dot(triangle[2] - point) / area;
    let b = (triangle[2] - point).perp_dot(triangle[0] - point) / area;
    let c = 1.0 - a - b;
    (a >= -1e-5 && b >= -1e-5 && c >= -1e-5).then_some(Vec3::new(a, b, c))
} // 与射线三角形相同的共享边容差，返回实际面内重心权重。

fn ray_box(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<(f32, f32)> {
    // 支持垂直与近水平射线。
    let mut start = 0.0_f32;
    let mut end = f32::INFINITY; // 只接受正向射线。
    for axis in 0..3 {
        if direction[axis].abs() < 1e-12 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - origin[axis]) / direction[axis];
            let b = (max[axis] - origin[axis]) / direction[axis];
            start = start.max(a.min(b));
            end = end.min(a.max(b));
            if start > end {
                return None;
            }
        }
    } // 三轴slab裁剪。
    Some((start, end)) // 候选区间。
} // 无堆分配。

fn ray_triangle(origin: Vec3, direction: Vec3, p: [Vec3; 3]) -> Option<f32> {
    // 双面最近三角面求交。
    let ab = p[1] - p[0];
    let ac = p[2] - p[0];
    let cross = direction.cross(ac);
    let det = ab.dot(cross); // Moller-Trumbore。
    if det.abs() < 1e-9 {
        return None;
    } // 零面积接缝面没有地表面积。
    let delta = origin - p[0];
    let u = delta.dot(cross) / det;
    let q = delta.cross(ab);
    let v = direction.dot(q) / det;
    let t = ac.dot(q) / det; // 重心与距离。
    (u >= -1e-5 && v >= -1e-5 && u + v <= 1.00001 && t >= 0.0).then_some(t) // 包含共享边的浮点容差。
} // 不使用作者双线性高度替代三角面。
