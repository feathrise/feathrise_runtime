//! F3 通用 ECS 与资产采样。
//!
//! 查询声明和计数聚合集中在这里，面板刷新系统只负责调度，文本格式化也无需直接
//! 了解 Bevy 查询细节。

use avian3d::prelude::{Collider, RigidBody};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use feathrise_billboard_text::{BillboardCamera, BillboardTextLink};
// 树木页只读核对实体LOD实际使用的扩展材质渲染方式。

/// F3 一次刷新所需的通用 ECS 查询集合。
#[derive(SystemParam)]
pub(super) struct ResourceMonitorQueries<'w, 's> {
    all_entities: Query<'w, 's, Entity>, // 统计世界中仍存活的实体总数。
    named_entities: Query<'w, 's, (), With<Name>>, // 统计带名称实体，辅助发现场景节点泄漏。
    billboard_links: Query<'w, 's, &'static BillboardTextLink>, // 拆分临时与常驻 Billboard，便于定位泄漏。
    billboard_cameras: Query<'w, 's, (), With<BillboardCamera>>, // 统计用于 Billboard 朝向的相机标记。
    mesh_instances: Query<'w, 's, (), With<Mesh3d>>,             // 统计主世界网格实例。
    scene_roots: Query<'w, 's, (), With<WorldAssetRoot>>,        // 统计世界资产根节点实例。
    cameras: Query<'w, 's, (), With<Camera>>,                    // 统计全部相机组件。
    point_lights: Query<'w, 's, (), With<PointLight>>,           // 统计点光源。
    directional_lights: Query<'w, 's, (), With<DirectionalLight>>, // 统计方向光。
    spot_lights: Query<'w, 's, (), With<SpotLight>>,             // 统计聚光灯。
    ui_nodes: Query<'w, 's, (), With<Node>>,                     // 统计 UI 布局节点。
    ui_texts: Query<'w, 's, (), With<Text>>,                     // 统计 UI 文本实体。
    rigid_bodies: Query<'w, 's, (), With<RigidBody>>,            // 统计物理刚体。
    colliders: Query<'w, 's, (), With<Collider>>,                // 统计物理碰撞体。
}

/// F3 一次刷新所需的资产集合。
#[derive(SystemParam)]
pub(super) struct ResourceMonitorAssets<'w> {
    meshes: Res<'w, Assets<Mesh>>,                // 读取已驻留网格资产数。
    materials: Res<'w, Assets<StandardMaterial>>, // 读取标准材质资产数。
    images: Res<'w, Assets<Image>>,                    // 读取图像资产数。
    animations: Res<'w, Assets<AnimationClip>>,        // 读取动画片段资产数。
}

impl ResourceMonitorAssets<'_> {
    /// 向树木分页提供同一份只读Mesh资产集合，避免系统参数重复借用资源。
    pub(super) fn meshes(&self) -> &Assets<Mesh> {
        &self.meshes // 树木半秒采样只读取当前可见LOD几何，不修改或克隆资产。
    } // 完成Mesh资产只读访问。

} // 完成资源集合的功能域内受控访问。

/// 与具体 ECS 查询解耦的一次通用资源快照。
#[derive(Default)] // 允许纯格式化测试只覆盖关心字段，不改变运行时采样或快照内存布局。
pub(super) struct ResourceMonitorSnapshot {
    pub(super) entities: usize,              // 存活实体总数。
    pub(super) named: usize,                 // 带 Name 组件的实体数。
    pub(super) billboards: usize,            // Billboard 链接总数。
    pub(super) billboard_temporary: usize,   // 带寿命的临时 Billboard 数。
    pub(super) billboard_persistent: usize,  // 不带寿命的常驻 Billboard 数。
    pub(super) billboard_cameras: usize,     // Billboard 朝向相机数。
    pub(super) mesh_instances: usize,        // Mesh3d 实例数。
    pub(super) scene_roots: usize,           // SceneRoot 实例数。
    pub(super) cameras: usize,               // 相机总数。
    pub(super) light_count: usize,           // 三类光源总数。
    pub(super) ui_nodes: usize,              // UI Node 总数。
    pub(super) ui_texts: usize,              // UI Text 总数。
    pub(super) rigid_bodies: usize,          // 物理刚体总数。
    pub(super) colliders: usize,             // 物理碰撞体总数。
    pub(super) asset_meshes: usize,          // 驻留 Mesh 资产数。
    pub(super) asset_materials: usize,       // 驻留 StandardMaterial 资产数。
    pub(super) asset_images: usize,          // 驻留 Image 资产数。
    pub(super) asset_animations: usize,      // 驻留 AnimationClip 资产数。
}

impl ResourceMonitorQueries<'_, '_> {
    /// 只在 F3 刷新周期到达时遍历查询并生成紧凑快照。
    pub(super) fn snapshot(&self, assets: &ResourceMonitorAssets<'_>) -> ResourceMonitorSnapshot {
        let light_count = self.point_lights.iter().count()
            + self.directional_lights.iter().count()
            + self.spot_lights.iter().count(); // 三类 Bevy 灯光合并为面板总数。
        let billboards = self.billboard_links.iter().count(); // Billboard 链接总数。
        let billboard_temporary = self
            .billboard_links
            .iter()
            .filter(|link| link.ttl.is_some())
            .count(); // 带 TTL 的链接视为临时 Billboard。
        let billboard_persistent = billboards.saturating_sub(billboard_temporary); // 饱和减法防止异常数据下溢。

        ResourceMonitorSnapshot {
            entities: self.all_entities.iter().count(), // 捕获实体总数。
            named: self.named_entities.iter().count(),  // 捕获命名实体数。
            billboards,                                 // 写入 Billboard 总数。
            billboard_temporary,                        // 写入临时 Billboard 数。
            billboard_persistent,                       // 写入常驻 Billboard 数。
            billboard_cameras: self.billboard_cameras.iter().count(), // 捕获 Billboard 相机数。
            mesh_instances: self.mesh_instances.iter().count(), // 捕获 Mesh3d 数。
            scene_roots: self.scene_roots.iter().count(), // 捕获 SceneRoot 数。
            cameras: self.cameras.iter().count(),       // 捕获相机数。
            light_count,                                // 写入三类光源总数。
            ui_nodes: self.ui_nodes.iter().count(),     // 捕获 UI 节点数。
            ui_texts: self.ui_texts.iter().count(),     // 捕获 UI 文本数。
            rigid_bodies: self.rigid_bodies.iter().count(), // 捕获刚体数。
            colliders: self.colliders.iter().count(),   // 捕获碰撞体数。
            asset_meshes: assets.meshes.iter().count(), // 捕获网格资产数。
            asset_materials: assets.materials.iter().count(), // 捕获材质资产数。
            asset_images: assets.images.iter().count(), // 捕获图像资产数。
            asset_animations: assets.animations.iter().count(), // 捕获动画片段资产数。
        }
    }
}
