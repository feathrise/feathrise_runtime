//! 可选渲染功能对玩法相机和标准阴影的临时运行时租约。
//!
//! 【功能设计】租约只覆盖实际渲染组件，不改写 `RenderSettings`。普通画质系统先根据当前玩家设置求出
//! 基线，本模块随后幂等应用额外要求；释放帧先恢复作者组件，再让最新玩家设置同帧重新接管。

use bevy::{
    camera::CameraMainTextureUsages, // 保存并扩展主颜色纹理用途，Solari/DLSS需要计算写入及清空用途。
    core_pipeline::prepass::{
        DeferredPrepass, DeferredPrepassDoubleBuffer, DepthPrepass, DepthPrepassDoubleBuffer,
        MotionVectorPrepass, NormalPrepass,
    }, // 可选渲染器共用的延迟、深度、运动矢量与双缓冲附件。
    pbr::deferred::PbrDeferredLightingDepthId, // Deferred视图自动派生的lighting pass id也必须精确归还。
    platform::collections::HashMap,            // 租约数量极小，按稳定ID保存公开要求。
    prelude::*,                                // ECS资源、组件、实体与相机类型。
    render::render_resource::TextureUsages,    // 主纹理存储绑定用途位。
};

/// 一份运行时图形租约的稳定标识；释放后不复用旧值，避免迟到调用误删新租约。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GraphicsRuntimeLeaseId(u64); // 字段私有，调用方只能使用注册表返回的合法票据。

/// 画质租约在 `PostUpdate` 中的公开排序锚点；可选渲染器只能在恢复阶段前维护声明，不能直接改相机。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphicsRuntimeLeaseSystems {
    RestoreReleased, // 先归还本帧已经释放的作者状态。
    ApplyActive,     // 普通画质收敛后再应用仍有效的合并要求。
}

/// 一份可组合的运行时图形要求。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphicsRuntimeRequirements {
    pub target_camera: Entity, // 只覆盖这一台明确相机，纹理相机和其他窗口相机不受影响。
    pub force_native_render_scale: bool, // 临时移除项目固定比例主通道，使用目标原生物理尺寸。
    pub force_single_sample: bool, // 强制Msaa::Off并暂停TAA历史，供Solari与时间重建共用。
    pub force_fxaa: bool,      // 没有时间重建器时以单帧FXAA提供基础边缘抗锯齿。
    pub suspend_fxaa: bool,    // DLSS RR接管最终抗锯齿时暂停FXAA，避免重建结果被二次模糊。
    pub suspend_dlss_super_resolution: bool, // RR交接期间要求独立SR先完整退出目标相机。
    pub require_hdr: bool,     // 保证HDR主目标和可用色调映射存在。
    pub require_deferred_prepass: bool, // 为延迟材质建立GBuffer预通道。
    pub require_depth_prepass: bool, // 为光追重投影和几何信息建立深度预通道。
    pub require_normal_prepass: bool, // 作者实体/线框显示读取单采样法线，不与普通画质争抢附件。
    pub require_motion_vector_prepass: bool, // 为时间域光照历史提供运动矢量。
    pub require_deferred_double_buffer: bool, // 保留上一帧GBuffer供Solari重投影。
    pub require_depth_double_buffer: bool, // 保留上一帧深度供Solari重投影。
    pub require_main_texture_storage_binding: bool, // 允许计算管线写回相机主纹理。
    pub require_main_texture_copy_source: bool, // 允许在UI绘制前复制最终颜色供帧生成HUD-less输入。
    pub suspend_ssao: bool,    // 租约期暂停玩法相机屏幕空间环境遮蔽。
    pub suspend_contact_shadows: bool, // 租约期暂停玩法相机屏幕空间接触阴影。
    pub suspend_standard_shadows: bool, // 租约期暂停全部灯光标准阴影图资格。
}

impl GraphicsRuntimeRequirements {
    /// 为明确目标相机构造不覆盖任何状态的通用租约要求。
    pub fn new(target_camera: Entity) -> Self {
        Self {
            target_camera,                               // 保存调用方明确选择的唯一目标相机。
            force_native_render_scale: false,            // 默认不改变固定渲染比例。
            force_single_sample: false,                  // 默认不改变玩家MSAA/TAA选择。
            force_fxaa: false,                           // 默认不强制添加FXAA。
            suspend_fxaa: false,                         // 默认不暂停玩家FXAA。
            suspend_dlss_super_resolution: false,        // 默认允许设置模块按玩家请求管理独立 SR。
            require_hdr: false,                          // 默认不取得HDR所有权。
            require_deferred_prepass: false,             // 默认不创建GBuffer。
            require_depth_prepass: false,                // 默认不创建深度附件。
            require_normal_prepass: false,               // 默认不取得法线预通道所有权。
            require_motion_vector_prepass: false,        // 默认不创建运动矢量附件。
            require_deferred_double_buffer: false,       // 默认不保留上一帧GBuffer。
            require_depth_double_buffer: false,          // 默认不保留上一帧深度。
            require_main_texture_storage_binding: false, // 默认不扩展相机主纹理用途。
            require_main_texture_copy_source: false,     // 默认不允许额外复制主颜色。
            suspend_ssao: false,                         // 默认保留玩家SSAO。
            suspend_contact_shadows: false,              // 默认保留玩家接触阴影。
            suspend_standard_shadows: false,             // 默认保留灯光标准阴影。
        }
    }

    /// 构造 Bevy Solari 实时光照所需的相机基线；标准阴影默认保留给不进TLAS的光栅内容。
    pub fn solari_warmup(target_camera: Entity) -> Self {
        Self {
            target_camera,                              // 严格绑定玩法主相机。
            force_native_render_scale: true,            // P1固定使用原生1080p诊断，不叠加空间放大。
            force_single_sample: true, // Solari要求单采样，并暂停与其历史链不兼容的项目TAA。
            force_fxaa: true,          // 没有RR时继续用单帧FXAA托底边缘。
            suspend_fxaa: false,       // 基础Solari路径没有时间重建器，保留FXAA。
            suspend_dlss_super_resolution: false, // 基础 Solari + SR 路径允许独立超分继续接管。
            require_hdr: true,         // Solari计算输出写入HDR主目标。
            require_deferred_prepass: true, // 可见兼容材质激活后写入延迟GBuffer。
            require_depth_prepass: true, // 直接光与间接光重建依赖当前深度。
            require_normal_prepass: false, // Solari本身不接管普通法线预通道。
            require_motion_vector_prepass: true, // 时间域历史需要运动矢量。
            require_deferred_double_buffer: true, // Solari读取上一帧GBuffer。
            require_depth_double_buffer: true, // Solari读取上一帧深度。
            require_main_texture_storage_binding: true, // 计算着色器需要写主纹理。
            require_main_texture_copy_source: false, // Solari自身不复制UI前颜色。
            suspend_ssao: true,        // 光追活动时不叠加屏幕空间环境遮蔽。
            suspend_contact_shadows: true, // 光追活动时不叠加屏幕空间接触阴影。
            suspend_standard_shadows: false, // P3角色与GPU叶片不进TLAS，活动态兼容层仍需该可见度来源。
        }
    }

    /// 构造共享 PBR 材质切到 Deferred 时，离屏辅助视图保持可见所需的最小要求。
    ///
    /// 【功能设计】该租约只补齐标准 Deferred/Depth 预通道及其单采样约束；不会把 HDR、
    /// 运动矢量、双缓冲、主纹理存储用途、FXAA、屏幕效果或标准阴影所有权传播给辅助相机。
    pub fn deferred_raster_compatibility(target_camera: Entity) -> Self {
        let mut requirements = Self::new(target_camera);
        requirements.force_single_sample = true; // Bevy 0.19 Deferred不支持MSAA，必须由租约显式拥有并恢复。
        requirements.require_deferred_prepass = true; // 让全局Deferred材质在该视图写入完整GBuffer。
        requirements.require_depth_prepass = true; // Bevy标准Deferred照明依赖同一视图的深度预通道。
        requirements
    }

    /// 构造 DLSS-G 与 FSR-G 共用的独立帧生成输入要求，不接管超分比例或普通屏幕效果。
    pub fn frame_generation(target_camera: Entity) -> Self {
        let mut requirements = Self::new(target_camera); // 复用空租约，避免引入超分或Solari的附加所有权。
        requirements.force_single_sample = true; // 深度和运动输入统一为单采样，暂时暂停玩家MSAA/TAA。
        requirements.force_fxaa = true; // 原生或空间放大路径以FXAA托底；时间超分的suspend_fxaa仍具有更高优先级。
        requirements.require_hdr = true; // 共用HUDless捕获从HDR主颜色编码至窗口sRGB。
        requirements.require_main_texture_copy_source = true; // 两后端共用UI前颜色捕获所需的纹理用途。
        requirements.require_depth_prepass = true; // 无需任何超分提供者也能取得本帧深度。
        requirements.require_motion_vector_prepass = true; // 独立租约持续提供本帧运动矢量。
        requirements // 仅声明固定六项要求，释放时由既有所有权恢复机制统一归还。
    }

    /// 构造 DLSS Super Resolution 临时接管普通画质所需的窗口相机要求。
    pub fn dlss_super_resolution(target_camera: Entity) -> Self {
        let mut requirements = Self::new(target_camera); // 从不影响 Deferred 或标准阴影的空要求开始。
        requirements.force_native_render_scale = true; // 暂停项目自有固定比例，由 NGX 最佳内部尺寸接管。
        requirements.force_single_sample = true; // DLSS 时间重建与 MSAA/TAA 不能叠加。
        requirements.suspend_fxaa = true; // SR 已承担抗锯齿，禁止普通 FXAA 二次模糊。
        requirements.require_hdr = true; // 当前封装以 HDR 主颜色作为 DLSS 输入。
        requirements.require_depth_prepass = true; // DLSS 时间重建需要当前深度。
        requirements.require_motion_vector_prepass = true; // DLSS 时间重建需要运动矢量。
        requirements.require_main_texture_storage_binding = true; // NGX 输出需要主纹理存储用途。
        requirements.suspend_ssao = true; // DLSS 生效时按正式产品协议暂停普通 SSAO。
        requirements.suspend_contact_shadows = true; // DLSS 生效时暂停普通接触阴影。
        requirements // 返回只影响明确目标相机的可合并要求。
    }

    /// 构造 FSR 3时间域超分临时接管普通画质所需的窗口相机要求。
    pub fn fsr3_super_resolution(target_camera: Entity) -> Self {
        let mut requirements = Self::new(target_camera); // 从不改变Deferred和标准阴影的空要求开始。
        requirements.force_native_render_scale = true; // 暂停项目空间放大，由FSR固定内部尺寸接管。
        requirements.force_single_sample = true; // 时间重建不能与MSAA或普通TAA叠加。
        requirements.suspend_fxaa = true; // FSR已经承担时间域抗锯齿，禁止FXAA二次模糊。
        requirements.require_hdr = true; // 固定上游输入和输出均为Rgba16Float。
        requirements.require_depth_prepass = true; // FSR重投影需要当前Depth32Float。
        requirements.require_motion_vector_prepass = true; // FSR历史需要当前Rg16Float运动矢量。
        requirements.require_main_texture_storage_binding = true; // 计算管线写入备用HDR主纹理。
        requirements.require_main_texture_copy_source = true; // 把Bevy全尺寸主纹理有效区紧凑复制为FSR颜色输入。
        requirements.suspend_ssao = true; // 当前SSAO不支持低分辨率预通道寻址。
        requirements.suspend_contact_shadows = true; // 接触阴影会越过低分辨率深度有效区。
        requirements // 返回只覆盖明确目标相机的可恢复要求。
    }

    /// 切换是否暂停标准阴影，保留给其他确实由替代阴影完整接管的运行时租约。
    pub fn with_standard_shadows_suspended(mut self, suspended: bool) -> Self {
        self.suspend_standard_shadows = suspended; // 只改变灯光阴影要求，其余相机契约保持不变。
        self
    }

    /// 切换是否暂停FXAA；DLSS RR已承担重建与抗锯齿时不得再叠加第二层后处理。
    pub fn with_fxaa_suspended(mut self, suspended: bool) -> Self {
        self.suspend_fxaa = suspended; // 只覆盖最终FXAA资格，Solari仍保持严格单采样。
        self
    }

    /// 切换是否暂停目标相机的独立 DLSS SR，供 RR 执行原子交接。
    pub fn with_dlss_super_resolution_suspended(mut self, suspended: bool) -> Self {
        self.suspend_dlss_super_resolution = suspended; // 只声明 SR 资格，不直接改任一厂商组件。
        self
    }

    /// 把同一目标相机上的另一份要求按“任一租约需要即保留”规则合并。
    fn merge(mut self, other: Self) -> Self {
        debug_assert_eq!(self.target_camera, other.target_camera); // 不同相机必须分别求值，不能误合并。
        self.force_native_render_scale |= other.force_native_render_scale;
        self.force_single_sample |= other.force_single_sample;
        self.force_fxaa |= other.force_fxaa;
        self.suspend_fxaa |= other.suspend_fxaa;
        self.suspend_dlss_super_resolution |= other.suspend_dlss_super_resolution;
        self.require_hdr |= other.require_hdr;
        self.require_deferred_prepass |= other.require_deferred_prepass;
        self.require_depth_prepass |= other.require_depth_prepass;
        self.require_normal_prepass |= other.require_normal_prepass;
        self.require_motion_vector_prepass |= other.require_motion_vector_prepass;
        self.require_deferred_double_buffer |= other.require_deferred_double_buffer;
        self.require_depth_double_buffer |= other.require_depth_double_buffer;
        self.require_main_texture_storage_binding |= other.require_main_texture_storage_binding;
        self.require_main_texture_copy_source |= other.require_main_texture_copy_source;
        self.suspend_ssao |= other.suspend_ssao;
        self.suspend_contact_shadows |= other.suspend_contact_shadows;
        self.suspend_standard_shadows |= other.suspend_standard_shadows;
        self // 返回不丢失任一调用方硬要求的合并结果。
    }
}

/// 全局图形运行时租约注册表；调用方只提交声明，实际组件写入统一由画质插件完成。
#[derive(Resource, Default)]
pub struct GraphicsRuntimeLeaseRegistry {
    next_id: u64, // 单调票据源，零值不对外返回。
    leases: HashMap<GraphicsRuntimeLeaseId, GraphicsRuntimeRequirements>, // 当前有效租约。
}

impl GraphicsRuntimeLeaseRegistry {
    /// 登记一份新租约并返回只能用于更新或释放该租约的稳定ID。
    pub fn acquire(&mut self, requirements: GraphicsRuntimeRequirements) -> GraphicsRuntimeLeaseId {
        self.next_id = self.next_id.wrapping_add(1).max(1); // 极端回绕时仍跳过未发放的零值。
        let id = GraphicsRuntimeLeaseId(self.next_id); // 票据在当前进程生命周期内单调增长。
        self.leases.insert(id, requirements); // 新要求从本帧后续画质系统开始生效。
        id
    }

    /// 原位更新仍有效的租约；未知或已释放票据返回 `false` 且不产生副作用。
    pub fn update(
        &mut self,
        id: GraphicsRuntimeLeaseId,
        requirements: GraphicsRuntimeRequirements,
    ) -> bool {
        let Some(current) = self.leases.get_mut(&id) else {
            return false; // 迟到更新不得复活已经释放的旧票据。
        };
        if *current != requirements {
            *current = requirements; // 只有真实变化才标记资源变更并触发目标组件调整。
        }
        true
    }

    /// 释放一份租约；最后一份目标租约消失后，画质插件在同帧恢复作者基线。
    pub fn release(&mut self, id: GraphicsRuntimeLeaseId) -> bool {
        self.leases.remove(&id).is_some() // 重复释放安全返回false，不影响其他调用方。
    }

    /// 返回同一目标相机全部有效租约的合并结果。
    pub fn requirements_for_camera(&self, camera: Entity) -> Option<GraphicsRuntimeRequirements> {
        self.leases
            .values()
            .copied()
            .filter(|requirements| requirements.target_camera == camera)
            .reduce(GraphicsRuntimeRequirements::merge) // 租约数极小且无稳定帧堆分配。
    }

    /// 判断任一有效租约是否要求暂停标准阴影图。
    pub fn standard_shadows_suspended(&self) -> bool {
        self.leases
            .values()
            .any(|requirements| requirements.suspend_standard_shadows)
    }

    /// 返回当前有效租约数量，供诊断和单元测试核对泄漏。
    pub fn active_lease_count(&self) -> usize {
        self.leases.len() // 只暴露数量，不允许外部改写内部映射。
    }
}

/// 保存租约首次接管前、不由普通画质资源独立管理的相机附件状态。
#[derive(Component, Clone, Copy)]
pub(crate) struct GraphicsRuntimeLeaseSnapshot {
    deferred_prepass: bool,       // 作者原本是否持有延迟GBuffer预通道。
    depth_prepass: bool,          // 作者原本是否持有深度预通道。
    motion_vector_prepass: bool,  // 作者原本是否持有运动矢量预通道。
    deferred_double_buffer: bool, // 作者原本是否持有GBuffer双缓冲。
    depth_double_buffer: bool,    // 作者原本是否持有深度双缓冲。
    main_texture_usages: Option<CameraMainTextureUsages>, // 作者原本的完整主纹理用途或缺省状态。
}

/// 只在至少一份租约真正强制单采样时捕获的 MSAA 作者值。
#[derive(Component, Clone, Copy)]
pub(crate) struct GraphicsRuntimeLeaseMsaaSnapshot(Msaa); // 与其他附件分离，非采样租约不取得MSAA所有权。

/// 只有实际声明法线要求的租约才取得所有权，其他租约不能回收普通SSAO的法线附件。
#[derive(Component, Clone, Copy)]
pub(crate) struct GraphicsRuntimeLeaseNormalSnapshot(bool);

/// 只在至少一份租约真正要求 Deferred 时捕获的 lighting pass id 作者值。
///
/// Bevy 0.19 会自动插入默认组件却不会在 `DeferredPrepass` 移除后回收，因此必须由租约显式拥有。
#[derive(Component, Clone, Copy)]
pub(crate) struct GraphicsRuntimeLeaseDeferredLightingSnapshot(Option<PbrDeferredLightingDepthId>);

type RuntimeLeaseCameraData<'a> = (
    Entity,                                                   // 当前三维相机实体。
    &'a Msaa,                            // Camera3d required组件，纹理相机也有独立作者值。
    Has<DeferredPrepass>,                // 当前GBuffer预通道状态。
    Has<DepthPrepass>,                   // 当前深度预通道状态。
    Has<NormalPrepass>,                  // 作者四球依赖的实际法线预通道状态。
    Has<MotionVectorPrepass>,            // 当前运动矢量预通道状态。
    Has<DeferredPrepassDoubleBuffer>,    // 当前GBuffer双缓冲状态。
    Has<DepthPrepassDoubleBuffer>,       // 当前深度双缓冲状态。
    Option<&'a CameraMainTextureUsages>, // 当前相机主纹理用途。
    Option<&'a GraphicsRuntimeLeaseSnapshot>, // 已捕获作者基线。
    Option<&'a GraphicsRuntimeLeaseMsaaSnapshot>, // 只有强制单采样要求曾生效时存在。
    Option<&'a GraphicsRuntimeLeaseNormalSnapshot>, // 只有法线要求曾生效时存在。
    Option<&'a PbrDeferredLightingDepthId>, // 当前Deferred lighting pass id。
    Option<&'a GraphicsRuntimeLeaseDeferredLightingSnapshot>, // Deferred要求首次生效前的作者值。
);

/// 最后一份目标租约释放后，先恢复不属于普通画质资源的作者附件。
pub(crate) fn restore_released_graphics_runtime_leases(
    mut commands: Commands, // 只为已经释放的目标提交精确恢复命令。
    registry: Res<GraphicsRuntimeLeaseRegistry>, // 判断每台已接管相机是否仍有有效租约。
    cameras: Query<RuntimeLeaseCameraData<'_>, With<Camera3d>>, // 三维相机集合通常只有玩法与少量预览相机。
) {
    for (
        entity,
        msaa,
        deferred_prepass,
        depth_prepass,
        normal_prepass,
        motion_vector_prepass,
        deferred_double_buffer,
        depth_double_buffer,
        main_texture_usages,
        snapshot,
        msaa_snapshot,
        normal_snapshot,
        deferred_lighting_id,
        deferred_lighting_snapshot,
    ) in &cameras
    {
        let requirements = registry.requirements_for_camera(entity);
        if let Some(original) = normal_snapshot.copied()
            && requirements.is_none_or(|requirements| !requirements.require_normal_prepass)
        {
            restore_marker::<NormalPrepass>(&mut commands, entity, normal_prepass, original.0);
            commands.entity(entity).try_remove::<GraphicsRuntimeLeaseNormalSnapshot>();
        }
        if let Some(msaa_snapshot) = msaa_snapshot.copied()
            && requirements.is_none_or(|requirements| !requirements.force_single_sample)
        {
            restore_msaa(&mut commands, entity, *msaa, msaa_snapshot.0); // 先归还采样数，窗口画质随后可按最新设置覆盖。
            commands
                .entity(entity)
                .try_remove::<GraphicsRuntimeLeaseMsaaSnapshot>(); // 恢复一次后停止稳定帧重复写作者旧值。
        }
        if let Some(deferred_lighting_snapshot) = deferred_lighting_snapshot.copied()
            && requirements.is_none_or(|requirements| !requirements.require_deferred_prepass)
        {
            restore_deferred_lighting_id(
                &mut commands,
                entity,
                deferred_lighting_id.copied(),
                deferred_lighting_snapshot.0,
            );
            commands
                .entity(entity)
                .try_remove::<GraphicsRuntimeLeaseDeferredLightingSnapshot>(); // 一轮Deferred所有权结束后允许重新捕获作者新值。
        }
        let Some(snapshot) = snapshot.copied() else {
            continue; // 从未被租约接管的相机保持零写入。
        };
        if requirements.is_some() {
            continue; // 仍有任一租约时由应用系统维护合并目标。
        }
        restore_marker::<DeferredPrepass>(
            &mut commands,
            entity,
            deferred_prepass,
            snapshot.deferred_prepass,
        );
        restore_marker::<DepthPrepass>(
            &mut commands,
            entity,
            depth_prepass,
            snapshot.depth_prepass,
        );
        restore_marker::<MotionVectorPrepass>(
            &mut commands,
            entity,
            motion_vector_prepass,
            snapshot.motion_vector_prepass,
        );
        restore_marker::<DeferredPrepassDoubleBuffer>(
            &mut commands,
            entity,
            deferred_double_buffer,
            snapshot.deferred_double_buffer,
        );
        restore_marker::<DepthPrepassDoubleBuffer>(
            &mut commands,
            entity,
            depth_double_buffer,
            snapshot.depth_double_buffer,
        );
        restore_main_texture_usages(
            &mut commands,
            entity,
            main_texture_usages.copied(),
            snapshot.main_texture_usages,
        );
        commands
            .entity(entity)
            .remove::<GraphicsRuntimeLeaseSnapshot>(); // 作者状态恢复后立即归还控制权。
    }
}

/// 在普通画质设置收敛后，幂等应用每台目标相机的合并租约要求。
pub(crate) fn apply_graphics_runtime_leases(
    mut commands: Commands, // 只在附件状态真实不一致时提交命令。
    registry: Res<GraphicsRuntimeLeaseRegistry>, // 读取调用方声明的不可变合并要求。
    cameras: Query<RuntimeLeaseCameraData<'_>, With<Camera3d>>, // 只枚举有限三维相机，不扫描场景网格。
) {
    for (
        entity,
        msaa,
        deferred_prepass,
        depth_prepass,
        normal_prepass,
        motion_vector_prepass,
        deferred_double_buffer,
        depth_double_buffer,
        main_texture_usages,
        snapshot,
        msaa_snapshot,
        normal_snapshot,
        deferred_lighting_id,
        deferred_lighting_snapshot,
    ) in &cameras
    {
        let Some(requirements) = registry.requirements_for_camera(entity) else {
            continue; // 非目标窗口或纹理相机完全隔离。
        };
        let baseline = snapshot.copied().unwrap_or(GraphicsRuntimeLeaseSnapshot {
            deferred_prepass,       // 首帧保存普通画质收敛后的真实作者GBuffer状态。
            depth_prepass,          // 首帧保存普通画质收敛后的真实作者深度状态。
            motion_vector_prepass,  // 首帧保存普通画质收敛后的真实作者运动矢量状态。
            deferred_double_buffer, // 首帧保存作者GBuffer历史状态。
            depth_double_buffer,    // 首帧保存作者深度历史状态。
            main_texture_usages: main_texture_usages.copied(), // 完整保存用途位，不能只记STORAGE位。
        });
        if snapshot.is_none() {
            commands.entity(entity).insert(baseline); // 每轮接管只捕获一次，更新租约不得覆盖作者基线。
        }
        if requirements.force_single_sample {
            if msaa_snapshot.is_none() {
                commands
                    .entity(entity)
                    .try_insert(GraphicsRuntimeLeaseMsaaSnapshot(*msaa)); // 在第一次强制前捕获当时权威作者值。
            }
            restore_msaa(&mut commands, entity, *msaa, Msaa::Off); // Deferred和Solari均要求单采样。
        }
        if requirements.require_normal_prepass {
            if normal_snapshot.is_none() {
                commands.entity(entity).try_insert(GraphicsRuntimeLeaseNormalSnapshot(normal_prepass));
            }
            restore_marker::<NormalPrepass>(&mut commands, entity, normal_prepass, true);
        }
        if requirements.require_deferred_prepass {
            if deferred_lighting_snapshot.is_none() {
                commands
                    .entity(entity)
                    .try_insert(GraphicsRuntimeLeaseDeferredLightingSnapshot(
                        deferred_lighting_id.copied(),
                    )); // 在Bevy自动补默认值之前捕获“存在/缺失+具体ID”的作者状态。
            }
            if deferred_lighting_id.is_none() {
                commands
                    .entity(entity)
                    .try_insert(PbrDeferredLightingDepthId::default()); // 同帧补齐，不让新建RTT首帧缺失Deferred lighting uniform。
            }
        }
        restore_marker::<DeferredPrepass>(
            &mut commands,
            entity,
            deferred_prepass,
            baseline.deferred_prepass || requirements.require_deferred_prepass,
        );
        restore_marker::<DepthPrepass>(
            &mut commands,
            entity,
            depth_prepass,
            baseline.depth_prepass || requirements.require_depth_prepass,
        );
        restore_marker::<MotionVectorPrepass>(
            &mut commands,
            entity,
            motion_vector_prepass,
            baseline.motion_vector_prepass || requirements.require_motion_vector_prepass,
        );
        restore_marker::<DeferredPrepassDoubleBuffer>(
            &mut commands,
            entity,
            deferred_double_buffer,
            baseline.deferred_double_buffer || requirements.require_deferred_double_buffer,
        );
        restore_marker::<DepthPrepassDoubleBuffer>(
            &mut commands,
            entity,
            depth_double_buffer,
            baseline.depth_double_buffer || requirements.require_depth_double_buffer,
        );
        let mut desired_usages = baseline.main_texture_usages; // 从完整作者用途开始按合并租约追加位。
        if requirements.require_main_texture_storage_binding {
            desired_usages = Some(
                desired_usages
                    .unwrap_or_default()
                    .with(TextureUsages::STORAGE_BINDING)
                    .with(TextureUsages::COPY_DST), // NGX会先清空输出；同时保留作者COPY或采样位。
            );
        }
        if requirements.require_main_texture_copy_source {
            desired_usages = Some(
                desired_usages
                    .unwrap_or_default()
                    .with(TextureUsages::COPY_SRC), // FG只读复制UI前最终颜色，不取得存储写所有权。
            );
        }
        restore_main_texture_usages(
            &mut commands,
            entity,
            main_texture_usages.copied(),
            desired_usages,
        );
    }
}

/// 恢复相机采样数；稳定帧不重复标记视图专用化状态。
fn restore_msaa(commands: &mut Commands, entity: Entity, current: Msaa, desired: Msaa) {
    if current != desired {
        commands.entity(entity).try_insert(desired); // 相机若同帧销毁则静默丢弃，不制造延迟命令错误。
    }
}

/// 精确恢复作者的 Deferred lighting pass id；缺失也是需要保留的有效作者状态。
fn restore_deferred_lighting_id(
    commands: &mut Commands,
    entity: Entity,
    current: Option<PbrDeferredLightingDepthId>,
    desired: Option<PbrDeferredLightingDepthId>,
) {
    match (current, desired) {
        (Some(current), Some(desired)) if current.get() != desired.get() => {
            commands.entity(entity).try_insert(desired);
        }
        (None, Some(desired)) => {
            commands.entity(entity).try_insert(desired);
        }
        (Some(_), None) => {
            commands
                .entity(entity)
                .try_remove::<PbrDeferredLightingDepthId>();
        }
        _ => {} // 存在性与数值一致时不触发视图重新专用化。
    }
}

/// 把一个零尺寸标记组件恢复到目标存在状态；稳定帧不重复写入。
fn restore_marker<T: Component + Default>(
    commands: &mut Commands, // 目标相机组件写入入口。
    entity: Entity,          // 当前目标相机。
    current: bool,           // 当前组件存在状态。
    desired: bool,           // 作者基线与合并租约共同要求的状态。
) {
    match (desired, current) {
        (true, false) => {
            commands.entity(entity).insert(T::default()); // 仅缺失时补齐要求组件。
        }
        (false, true) => {
            commands.entity(entity).remove::<T>(); // 仅无所有者时移除租约组件。
        }
        _ => {} // 状态一致时不触发Bevy变更检测或管线重建。
    }
}

/// 恢复或扩展相机主纹理用途，保留作者全部位标志。
fn restore_main_texture_usages(
    commands: &mut Commands,                  // 目标相机组件写入入口。
    entity: Entity,                           // 当前目标相机。
    current: Option<CameraMainTextureUsages>, // 当前完整用途。
    desired: Option<CameraMainTextureUsages>, // 作者基线或追加运行时用途后的目标用途。
) {
    match (desired, current) {
        (Some(desired), Some(current)) if desired.0 != current.0 => {
            commands.entity(entity).insert(desired); // 用途位真实变化时更新主纹理描述。
        }
        (Some(desired), None) => {
            commands.entity(entity).insert(desired); // 作者无显式组件时按默认用途加租约位。
        }
        (None, Some(_)) => {
            commands.entity(entity).remove::<CameraMainTextureUsages>(); // 作者原本缺失时精确恢复缺失状态。
        }
        _ => {} // 完整用途已一致时保持稳定。
    }
}

#[cfg(test)]
mod tests; // 租约所有权和相机隔离夹具独立放入同域子模块，保持运行时文件低于500行。
