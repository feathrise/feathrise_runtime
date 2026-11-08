//! 通用地表材质的轻量ECS组件。

use bevy::asset::AssetId; // 材质租约只保存带代次的精确资产身份，不延长会话资产寿命。
use bevy::prelude::{Component, Handle, Image}; // 组件只保存固定数量身份或当前模式的少量强Image句柄。
use bevy::render::extract_component::ExtractComponent; // 把主世界地表身份同步到RenderApp。
use bevy::render::sync_component::SyncComponent; // Bevy 0.19提取插件要求声明目标Bundle。

use super::material::TerrainSurfaceMaterial; // 主世界材质仲裁必须使用完整扩展材质类型身份。

/// 子地图的渲染证据不能混入父图；实体地址与地表世代共同拒绝迟到反馈。
#[derive(Component, Clone, Copy, Debug, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct TerrainSurfaceInstanceScope { pub instance: bevy::prelude::Entity, pub generation: u32 }

/// 只观察有界准备席位的候选；活动子图不重复聚合整套GPU诊断。
#[derive(Component, Clone, Copy, Debug, Default, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct TerrainSurfaceInstanceWarmup;

/// 标记已经绑定正式权重材质的地形实体或短寿命预热实体。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)] // 零尺寸标记可直接跨世界复制。
pub struct TerrainSurfaceCarrier; // RenderApp只观察显式承接实体，不扫描所有PBR网格。

/// 仅用于材质管线准备的离屏副本；有Mesh/AABB也不属于可取景、可编辑的地图内容。
#[derive(Component, Clone, Copy, Debug, Default, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct TerrainSurfaceWarmup;

/// 雕刻体积面复用地表PBR，但不加入唯一地表加载/预热统计，隐藏体积候选不能阻塞整图Ready。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainVolumeSurface;

impl ExtractComponent<bevy::render::RenderApp> for TerrainSurfaceCarrier {
    // 使用Bevy组件提取器同步零尺寸身份。
    type QueryData = &'static Self; // 主世界只读取标记本身。
    type QueryFilter = (); // 预热实体即使位于视锥外也必须提取。
    type Out = Self; // 渲染世界继续使用同一零尺寸组件。

    fn extract_component(marker: &Self) -> Option<Self::Out> {
        // 每个命中实体输出一个标记副本。
        Some(*marker) // 标记存在即同步，移除时由同步插件自动清理。
    } // 完成承接标记提取。
} // 完成承接标记提取实现。

impl SyncComponent<bevy::render::RenderApp> for TerrainSurfaceCarrier {
    // 声明渲染世界目标组件类型。
    type Target = Self; // 主世界与渲染世界保持一对一标记。
} // 完成同步组件实现。

/// 记录一个运行时系统对正式地表材质的精确临时替代。
///
/// 【功能设计】动态地图仍拥有 `source` 对应的作者地表；Solari 等运行时系统只能在同一实体上
/// 临时安装 `installed`。地图稳定核对仅在两个带代次 AssetId 同时匹配时保留替代，避免双方逐帧
/// 抢写 `MeshMaterial3d`；租约撤销、资产回收或实体换代后则立即恢复地图自己的正式材质。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainSurfaceMaterialOverride {
    source: AssetId<TerrainSurfaceMaterial>, // 地图适配器当前持有的正式作者材质。
    installed: AssetId<TerrainSurfaceMaterial>, // 租约所有者实际安装到实体的会话材质。
}

impl TerrainSurfaceMaterialOverride {
    /// 为同一地表实体冻结不可混用的正式与替代材质身份。
    pub fn new(
        source: &Handle<TerrainSurfaceMaterial>,
        installed: &Handle<TerrainSurfaceMaterial>,
    ) -> Self {
        Self {
            source: source.id(),
            installed: installed.id(),
        }
    }

    /// 只有正式源和当前已安装材质都仍属于本租约时返回 `true`。
    pub fn matches(
        &self,
        source: AssetId<TerrainSurfaceMaterial>,
        installed: AssetId<TerrainSurfaceMaterial>,
    ) -> bool {
        self.source == source && self.installed == installed
    }
}

/// 同步当前模式完整运行时纹理组的 GPU 上传状态；探针不参与绘制。
#[derive(Component, Clone, Debug)] // 旧固定层为三张，动态索引 PBR 为五张，不分拆版本证据。
pub struct TerrainSurfaceTextureProbe {
    pub(super) images: Vec<Handle<Image>>, // 顺序由地图适配器冻结，RenderApp 只核对全部句柄。
    pub(super) failed: bool,               // 任一根资产或依赖明确失败时整体置位。
} // 完成纹理探针数据定义。

impl TerrainSurfaceTextureProbe {
    // 地图层只通过具名构造器发布完整或失败状态。
    /// 为已经发起异步加载的完整纹理组创建等待探针。
    pub fn new(images: impl IntoIterator<Item = Handle<Image>>) -> Self {
        Self {
            // 原子构造完整探针值。
            images: images.into_iter().collect(), // 数量小且只在版本边界构造。
            failed: false,                        // 初始状态属于加载或GPU上传等待。
        } // 返回正常等待探针。
    } // 完成正常探针构造。

    /// 把同一组纹理标记为已经确认失败。
    pub fn failed(images: impl IntoIterator<Item = Handle<Image>>) -> Self {
        // 保留句柄便于后续同路径热修重新触发加载。
        Self {
            // 原子构造失败探针值。
            images: images.into_iter().collect(), // 失败期间继续记录准确资产身份。
            failed: true,                         // RenderApp下一完整帧发布明确错误而非无限等待。
        } // 返回失败探针。
    } // 完成失败探针构造。
} // 完成纹理探针公共接口。

impl ExtractComponent<bevy::render::RenderApp> for TerrainSurfaceTextureProbe {
    // 把当前完整纹理组和失败位同步到RenderApp。
    type QueryData = &'static Self; // 主世界只读完整探针。
    type QueryFilter = (); // 探针没有网格或可见性仍必须无条件提取。
    type Out = Self; // 渲染世界使用同一只读值类型。

    fn extract_component(probe: &Self) -> Option<Self::Out> {
        // 每帧只克隆少量强句柄。
        Some(probe.clone()) // 不复制纹理像素，只复制引用计数和布尔状态。
    } // 完成纹理探针提取。
} // 完成探针提取实现。

impl SyncComponent<bevy::render::RenderApp> for TerrainSurfaceTextureProbe {
    // 声明渲染世界目标组件类型。
    type Target = Self; // 探针在两个世界保持相同字段。
} // 完成探针同步实现。
