//! 通用地表生命周期与上一完整RenderApp帧诊断资源。

use std::sync::atomic::{AtomicU32, Ordering}; // 渲染阶段通过无锁计数器跨系统汇总同一帧证据。

use bevy::asset::RenderAssetUsages; // 内部零权重图只需随资产进入渲染世界。
use bevy::prelude::{Assets, FromWorld, Handle, Image, Resource, World}; // 回退资源在主世界创建唯一强图片句柄。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 构造一乘一线性RGBA零纹理。
use bevy::render::storage::ShaderBuffer; // 旧固定层管线需绑定一项中性动态层表。

use super::material::TerrainSurfaceIndexedLayer; // 回退存储表与正式动态场景使用同一 GPU 布局。

/// 所有单权重图场景共用的内部零纹理。
#[derive(Resource, Clone, Debug)] // 场景描述符只克隆强句柄，不额外加载磁盘资产。
pub struct TerrainSurfaceFallbackImages {
    pub zero_weight: Handle<Image>, // 第二RGBA槽未使用时绑定合法的一乘一全零图片。
    pub white_base_color_array: Handle<Image>, // 无纹理场景仍需绑定合法二维数组。
    pub neutral_arm_array: Handle<Image>, // AO一、粗糙度一、金属度零的二维数组。
    pub neutral_normal_array: Handle<Image>, // 切线空间 (0,0,1) 中性法线。
    pub zero_index: Handle<Image>,  // 一乘一 RGBA32_UINT 零索引回退。
    pub neutral_layer_table: Handle<ShaderBuffer>, // 单项白色高粗糙中性存储表。
} // 完成内部回退图片资源定义。

impl FromWorld for TerrainSurfaceFallbackImages {
    // 插件初始化时只创建一次内部图片资产。
    fn from_world(world: &mut World) -> Self {
        // 从已经注册的Image资产表取得稳定强句柄。
        let image = Image::new_fill(
            // 创建不经过sRGB转换的四通道零权重纹理。
            Extent3d {
                // 声明最小合法二维图片范围。
                width: 1,                 // 单像素足以为未使用槽提供确定性零值。
                height: 1,                // 单像素足以为未使用槽提供确定性零值。
                depth_or_array_layers: 1, // 普通二维纹理而非数组。
            }, // 完成图片尺寸。
            TextureDimension::D2,            // 与权重绑定声明的texture_2d一致。
            &[0, 0, 0, 0],                   // 四个通道严格为零，残余层仍由Shader重建。
            TextureFormat::Rgba8Unorm,       // 权重是线性数值，禁止sRGB解码。
            RenderAssetUsages::RENDER_WORLD, // CPU无需读取这张固定回退图片。
        ); // 完成内部零图片构造。
        let zero_weight = world.resource_mut::<Assets<Image>>().add(image); // 加入全局图片资产表并强持有。
        // WebGPU二维数组绑定至少使用两个切片；两层相同像素不会改变纯参数层结果。
        let mut base_color = Image::new_fill(
            Extent3d {
                width: 1,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255, 255, 255, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        base_color
            .reinterpret_stacked_2d_as_array(2)
            .expect("内部基础色回退图必须可解释为两层二维数组");
        let white_base_color_array = world.resource_mut::<Assets<Image>>().add(base_color);
        let mut arm = Image::new_fill(
            Extent3d {
                width: 1,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255, 255, 0, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        arm.reinterpret_stacked_2d_as_array(2)
            .expect("内部ARM回退图必须可解释为两层二维数组");
        let neutral_arm_array = world.resource_mut::<Assets<Image>>().add(arm);
        let mut normal = Image::new_fill(
            Extent3d {
                width: 1,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[128, 128, 255, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        normal
            .reinterpret_stacked_2d_as_array(2)
            .expect("内部法线回退图必须可解释为两层二维数组");
        let neutral_normal_array = world.resource_mut::<Assets<Image>>().add(normal);
        let zero_index = world.resource_mut::<Assets<Image>>().add(Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0; 16],
            TextureFormat::Rgba32Uint,
            RenderAssetUsages::RENDER_WORLD,
        ));
        let neutral_layer_table =
            world
                .resource_mut::<Assets<ShaderBuffer>>()
                .add(ShaderBuffer::from(vec![TerrainSurfaceIndexedLayer {
                    base_color: bevy::math::Vec4::ONE,
                    pbr_and_texture: bevy::math::Vec4::new(1.0, 0.0, 1.0, 0.0),
                    normal_height_projection: bevy::math::Vec4::ZERO,
                }]));
        Self {
            zero_weight,
            white_base_color_array,
            neutral_arm_array,
            neutral_normal_array,
            zero_index,
            neutral_layer_table,
        } // 发布给所有场景适配器复用同一句柄。
    } // 完成回退资源初始化。
} // 完成FromWorld实现。

/// 当前场景地表从白模到正式材质的明确生命周期阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] // 值类型便于F3读取和状态机单元测试。
pub enum TerrainSurfacePhase {
    #[default]
    Inactive, // 当前场景没有启用通用权重地表。
    White,   // 白模已经出现但尚未发起正式纹理加载。
    Loading, // 当前模式的完整纹理组正在由AssetServer异步加载或上传。
    Warming, // 纹理已加载，离屏同网格实体正在等待PBR管线和Forward/Deferred实际排队证据。
    Active,  // 正式材质已经原子转交给真实地形实体。
    Failed,  // 首载或热更新已确认失败，真实地形恢复纯白材质。
} // 完成地表生命周期枚举。

/// 当前地表修订的具体失败项。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] // 固定枚举可无分配显示到F3并用于状态机测试。
pub enum TerrainSurfaceFailure {
    #[default]
    None, // 当前没有已经确认的失败。
    WeightMap,      // 一张正式RGBA权重图缺失、损坏或依赖失败。
    IndexMap,       // 动态场景 RGBA32_UINT 局部索引图失败。
    BaseColorArray, // 正式sRGB基础色二维数组缺失、损坏或依赖失败。
    NormalArray,    // 正式线性法线二维数组失败。
    ArmArray,       // 正式线性ARM二维数组缺失、损坏或依赖失败。
    ShaderPipeline, // WGSL编译或GPU管线创建已经明确失败。
    Definition,     // 场景地表JSON版本、层参数或资源路径无效。
} // 完成失败项枚举。

/// 主世界读取的上一完整RenderApp帧地表快照。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)] // 固定小值可无分配覆盖并供测试直接比较。
pub struct TerrainSurfaceRenderStats {
    pub phase: TerrainSurfacePhase, // 场景生命周期系统发布的当前白模、等待、活动或失败阶段。
    pub failure: TerrainSurfaceFailure, // Failed阶段对应的具体纹理或Shader项。
    pub generation: u32,            // 每次进入场景或重新加载正式纹理时递增，迟到证据不得跨代采用。
    pub texture_ready: u32,         // 完整纹理组中已经存在于RenderAssets<GpuImage>的数量。
    pub texture_expected: u32,      // 当前唯一探针声明的完整纹理数。
    pub texture_waiting: u32,       // 尚未上传但没有明确失败的纹理数量。
    pub texture_error: u32,         // 任一纹理或依赖明确失败时为一。
    pub carrier_instances: u32,     // RenderApp中仍绑定TerrainSurfaceMaterial的承接实体数量。
    pub pipeline_ready: u32,        // 全部已发现地表PBR专用管线就绪时为一。
    pub pipeline_waiting: u32,      // 材质尚未提取、专用化或管线仍在创建时为一。
    pub pipeline_error: u32,        // 任一WGSL编译或设备创建失败时为一。
    pub phase_queued: u32,          // 全部承接实体已进入自身当前视图的Forward或Deferred表面分箱时为一。
} // 完成F3与场景预热共享的地表快照。

/// 上一完整渲染帧的子场景候选反馈；数量由宿主准备席位约束，不按活动场景全量扫描。
#[derive(Resource, Default)]
pub struct TerrainSurfaceInstanceFeedback(pub Vec<(bevy::prelude::Entity, TerrainSurfaceRenderStats)>);

impl TerrainSurfaceRenderStats {
    // 场景生命周期只修改主世界字段，不触碰RenderApp证据。
    /// 发布新的场景代次和生命周期阶段。
    pub fn begin_generation(&mut self) {
        // 每次首次加载或显式热更新都开始新代次。
        self.generation = self.generation.wrapping_add(1).max(1); // 零保留未启用语义，回绕后重新从一开始。
        self.phase = TerrainSurfacePhase::Loading; // 新代次首先等待三张正式纹理。
        self.failure = TerrainSurfaceFailure::None; // 新会话尚未确认任何失败。
    } // 完成新代次发布。

    /// 只切换生命周期阶段，保留上一完整RenderApp证据供诊断解释。
    pub fn set_phase(&mut self, phase: TerrainSurfacePhase) {
        // 场景系统在主线程状态边界调用。
        self.phase = phase; // 固定枚举写入不会触发任何资产或渲染工作。
        if phase != TerrainSurfacePhase::Failed {
            // 离开失败态必须清除上一个修订的原因。
            self.failure = TerrainSurfaceFailure::None; // F3不得在活动或等待阶段显示陈旧失败项。
        } // 完成非失败阶段原因清理。
    } // 完成阶段更新。

    /// 发布当前修订已经确认的具体失败项。
    pub fn set_failure(&mut self, failure: TerrainSurfaceFailure) {
        // 场景层在恢复白模后调用。
        self.phase = TerrainSurfacePhase::Failed; // 失败原因与生命周期阶段原子一致。
        self.failure = failure; // 保存无需分配的具体失败身份。
    } // 完成失败状态发布。

    /// 清空离图状态和全部上一帧证据。
    pub fn clear(&mut self) {
        // HN-01离场时恢复全局未启用快照。
        *self = Self::default(); // 一次覆盖所有固定字段，避免漏清旧场景证据。
    } // 完成快照清理。
} // 完成主世界快照接口。

/// 同一事务取得完整纹理、管线和Forward/Deferred证据且至少观察两帧后才允许原子接管白模。
pub fn terrain_surface_warmup_can_finalize(
    stats: &TerrainSurfaceRenderStats,
    expected_carriers: u32,
    observed_frames: u8,
) -> bool {
    observed_frames >= 2
        && expected_carriers > 0
        && stats.texture_expected > 0
        && stats.texture_ready == stats.texture_expected
        && stats.texture_waiting == 0
        && stats.texture_error == 0
        && stats.carrier_instances == expected_carriers
        && stats.pipeline_ready == 1
        && stats.pipeline_waiting == 0
        && stats.pipeline_error == 0
        && stats.phase_queued == 1
}

/// RenderApp当前帧地表证据计数器。
#[derive(Resource, Default)] // 原子字段可安全零初始化并只存在于渲染世界。
pub(super) struct TerrainSurfaceRenderFrameCounters {
    texture_ready: AtomicU32,     // 当前帧已上传的正式纹理数。
    texture_expected: AtomicU32,  // 当前帧探针声明的正式纹理总数。
    texture_waiting: AtomicU32,   // 当前帧仍等待上传的正式纹理数。
    texture_error: AtomicU32,     // 当前帧纹理整体失败标志。
    carrier_instances: AtomicU32, // 当前帧正式材质承接实体数。
    pipeline_ready: AtomicU32,    // 当前帧地表专用管线完整就绪标志。
    pipeline_waiting: AtomicU32,  // 当前帧地表专用管线等待标志。
    pipeline_error: AtomicU32,    // 当前帧地表专用管线错误标志。
    phase_queued: AtomicU32,      // 当前帧全部地表承接实体进入Forward/Deferred表面分箱的标志。
} // 完成渲染帧内部计数器。

impl TerrainSurfaceRenderFrameCounters {
    // 所有RenderApp写点通过具名方法表达状态语义。
    /// 发布当前唯一探针的三张GPU纹理状态。
    pub(super) fn record_textures(
        // RenderApp观察系统每帧调用一次。
        &self,         // 只通过原子字段修改共享计数器。
        ready: u32,    // 已存在于GpuImage表的句柄数。
        expected: u32, // 探针声明的总句柄数。
        failed: bool,  // 主世界已经确认任一资产加载失败。
    ) {
        // 覆盖当前帧唯一地表纹理快照。
        self.texture_ready.store(ready, Ordering::Relaxed); // 发布已上传数量。
        self.texture_expected.store(expected, Ordering::Relaxed); // 发布总纹理数量。
        self.texture_waiting.store(
            // 失败时等待数量必须归零，避免同时显示两种互斥状态。
            if failed {
                0
            } else {
                expected.saturating_sub(ready)
            }, // 仅未失败的缺图属于异步等待。
            Ordering::Relaxed, // 帧边界保证不需要跨字段强顺序。
        ); // 完成等待数量发布。
        self.texture_error
            .store(u32::from(failed), Ordering::Relaxed); // 发布唯一错误标志。
    } // 完成纹理证据记录。

    /// 发布正式材质提取、管线与Opaque排队状态。
    pub(super) fn record_pipeline(
        // RenderApp观察系统每帧调用一次。
        &self,         // 只通过原子字段覆盖本帧唯一快照。
        carriers: u32, // 同步到渲染世界的承接实体数。
        ready: bool,   // 全部匹配管线是否已经可执行。
        waiting: bool, // 是否仍等待提取、专用化或创建。
        error: bool,   // 是否出现明确管线错误。
        queued: bool,  // 是否至少进入一个Opaque3d分箱。
    ) {
        // 覆盖当前帧管线证据。
        self.carrier_instances.store(carriers, Ordering::Relaxed); // 发布承接实体数。
        self.pipeline_ready
            .store(u32::from(ready), Ordering::Relaxed); // 发布完整就绪标志。
        self.pipeline_waiting
            .store(u32::from(waiting), Ordering::Relaxed); // 发布等待标志。
        self.pipeline_error
            .store(u32::from(error), Ordering::Relaxed); // 发布错误标志。
        self.phase_queued
            .store(u32::from(queued), Ordering::Relaxed); // 发布Opaque排队证据。
    } // 完成管线证据记录。

    /// 在下一帧ExtractSchedule开始时取走并清零上一完整渲染帧。
    pub(super) fn snapshot_and_reset(&self) -> TerrainSurfaceRenderStats {
        // 返回不含场景生命周期字段的渲染快照。
        let take = |counter: &AtomicU32| counter.swap(0, Ordering::Relaxed); // 每个字段在同一明确帧边界原子取走。
        TerrainSurfaceRenderStats {
            // 把内部计数映射为主世界公开字段。
            phase: TerrainSurfacePhase::Inactive, // 发布系统会保留主世界当前生命周期阶段。
            failure: TerrainSurfaceFailure::None, // 发布系统会保留主世界当前失败原因。
            generation: 0,                        // 发布系统会保留主世界当前场景代次。
            texture_ready: take(&self.texture_ready), // 发布已上传纹理数并归零。
            texture_expected: take(&self.texture_expected), // 发布期望纹理数并归零。
            texture_waiting: take(&self.texture_waiting), // 发布等待纹理数并归零。
            texture_error: take(&self.texture_error), // 发布纹理错误标志并归零。
            carrier_instances: take(&self.carrier_instances), // 发布承接实体数并归零。
            pipeline_ready: take(&self.pipeline_ready), // 发布管线就绪标志并归零。
            pipeline_waiting: take(&self.pipeline_waiting), // 发布管线等待标志并归零。
            pipeline_error: take(&self.pipeline_error), // 发布管线错误标志并归零。
            phase_queued: take(&self.phase_queued), // 发布Opaque排队标志并归零。
        } // 返回上一完整帧快照。
    } // 完成帧边界发布。
} // 完成渲染帧计数器接口。

#[cfg(test)]
mod readiness_tests {
    use super::*;

    #[test]
    fn complete_surface_receipt_never_bypasses_missing_or_failed_gpu_evidence() {
        let ready = TerrainSurfaceRenderStats {
            texture_ready: 5, texture_expected: 5, carrier_instances: 1,
            pipeline_ready: 1, phase_queued: 1, ..Default::default()
        };
        assert!(terrain_surface_warmup_can_finalize(&ready, 1, 2));
        assert!(!terrain_surface_warmup_can_finalize(&ready, 1, 1));
        assert!(!terrain_surface_warmup_can_finalize(&ready, 2, 2));
        // Forward/Deferred只增加可证明的正式路径；纹理、载体、管线失败或普通深度预通道均不能提前放行。
        for incomplete in [
            TerrainSurfaceRenderStats { texture_ready: 4, ..ready },
            TerrainSurfaceRenderStats { texture_waiting: 1, ..ready },
            TerrainSurfaceRenderStats { texture_error: 1, ..ready },
            TerrainSurfaceRenderStats { carrier_instances: 0, ..ready },
            TerrainSurfaceRenderStats { pipeline_ready: 0, pipeline_waiting: 1, ..ready },
            TerrainSurfaceRenderStats { pipeline_error: 1, ..ready },
            TerrainSurfaceRenderStats { phase_queued: 0, ..ready },
        ] {
            assert!(!terrain_surface_warmup_can_finalize(&incomplete, 1, 2), "{incomplete:?}");
        }
    }
}
