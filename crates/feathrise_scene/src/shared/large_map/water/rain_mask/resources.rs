//! 捕获缓存只保存轻量句柄和实体身份；不保存或克隆Mesh顶点。
use super::super::material::DynamicWaterMaterial;
use super::material::RainHeightMaterial; // 无光照捕获材质。
use crate::maps::{ClipmapRainHeightMaterial, ClipmapWaterMaterial};
use bevy::prelude::*; // 实体与资产身份。
use std::collections::{HashMap, HashSet, VecDeque}; // 变化去重与固定安装预算队列。 // 原材质身份用于共享固定波参数。

pub(super) struct ProxyRecord {
    // 每个正式水primitive至多一个捕获代理。
    pub(super) entity: Entity, // 挂在原水实体下的子代理。
    pub(super) source_material: AssetId<DynamicWaterMaterial>, // 换材质时准确释放旧引用。
} // 完成代理记录。

pub(super) struct MaterialRecord {
    // 多个源primitive共享同一个捕获材质。
    pub(super) handle: Handle<RainHeightMaterial>, // 独占捕获资产句柄。
    pub(super) users: usize,                       // 最后一个代理消失才释放捕获材质。
} // 完成材质引用记录。

pub(super) struct ClipmapProxyRecord {
    // Clipmap每层拥有独立R8与水位，捕获材质不能跨层共享。
    pub(super) entity: Entity,
    pub(super) source_material: AssetId<ClipmapWaterMaterial>,
    pub(super) material: Handle<ClipmapRainHeightMaterial>,
}

#[derive(Resource, Default)] // 稳定帧不重建缓存、不复制几何。
pub(in super::super) struct RainMaskCache {
    // 单个高度图功能域的全部轻量状态。
    pub(super) proxies: HashMap<Entity, ProxyRecord>, // 原实体直接定位捕获代理。
    pub(super) materials: HashMap<AssetId<DynamicWaterMaterial>, MaterialRecord>, // 原材质直接定位固定uniform目标。
    pub(super) pending: VecDeque<Entity>, // 待安装变化有界每帧消费，不在一次Update安装整个地图。
    pub(super) queued: HashSet<Entity>,   // 每个源实体最多一个在途请求。
    pub(super) clipmap_proxies: HashMap<Entity, ClipmapProxyRecord>, // 最多八层，复用正式环带Mesh。
    pub(super) clipmap_pending: VecDeque<Entity>, // Clipmap代理也按固定预算渐进安装。
    pub(super) clipmap_queued: HashSet<Entity>, // 同一层多次变更只保留最新请求。
} // 完成缓存资源。
