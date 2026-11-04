//! 场景 crate 的公共 ECS 组件，包含地图 GLB 热重建登记信息与内部生命周期标记。

use avian3d::prelude::ColliderConstructorHierarchy; // 保存地图重建时需要重新插入的 Avian 碰撞层级配置。
use bevy::camera::visibility::VisibilityRange; // 保存树木、灌木与岩石的作者 LOD 范围。
use bevy::prelude::*; // 引入 Component、Handle、WorldAsset 等 Bevy ECS 与资产类型。

/// 标记统一动态地图中唯一承担正式画面和地形碰撞的 GLB primitive。
///
/// F12 作者工具通过这个稳定身份把内存草稿原子应用到正式实体，不依赖中文节点名，
/// 也不会误改地表预热副本、河水或其他场景网格。
#[derive(Component, Clone, Copy, Debug, Default)] // 零大小身份可跨 crate 查询且不保存运行时状态。
pub struct DynamicMapTerrainPrimitive; // 正式动态地形 primitive 的唯一公共标记。

/// 稀疏 Clipmap 顶点由 GPU 高度页位移，不能被旧静态 Mesh 的 Solari BLAS 当作真实地形。
#[derive(Component)]
pub struct DynamicSparseClipmapPrimitive;

/// F12“显示网格”只接管显式登记的真实可见地形，不通过材质、碰撞或名称做宽泛推断。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 发布构建不携带作者调试目标身份。
#[derive(Component, Clone, Copy, Debug, Default)] // 零大小标记允许正式场景与作者预览跨 crate 共用。
pub struct TerrainGridTarget; // 建筑、水体、植被、角色、碰撞与离屏预热实体均不得获得此标记。

/// 标记稀疏高度派生的世界固定 Collider，供作者工具区分碰撞LOD与屋顶、桥面。
/// 地形作者Y必须查询基础页；其他真实承托面仍按物理命中处理，不按节点名猜测。
#[derive(Component)]
pub struct DynamicSparseTerrainCollider;

/// 登记一个需要在 GLB 资产热加载后完整重建的地图场景根。
#[derive(Component, Clone)] // 组件需要克隆场景句柄和碰撞配置，以便跨帧恢复地图根。
pub struct HotReloadableMapScene {
    pub scene: Handle<WorldAsset>, // 保存地图 Scene0 句柄，用于匹配全局 WorldAsset 修改事件。
    pub collision_hierarchy: ColliderConstructorHierarchy, // 保存地图专属碰撞规则，重建时重新触发 Avian 构造流程。
} // 结束可热重建地图组件定义。

impl HotReloadableMapScene {
    /// 创建地图热重建登记组件，调用方继续负责把相同句柄与碰撞配置插入场景根。
    pub fn new(
        scene: Handle<WorldAsset>, // 接收当前地图实际加载的 Scene0 句柄。
        collision_hierarchy: ColliderConstructorHierarchy, // 接收当前地图的 Avian 碰撞层级配置。
    ) -> Self {
        Self {
            scene,               // 保留稳定资产句柄，热加载时用 AssetId 精确匹配当前地图。
            collision_hierarchy, // 保留可克隆配置，避免通用系统了解各正式地图的名称白名单。
        } // 完成地图热重建登记组件构造。
    } // 结束地图热重建登记组件构造函数。
} // 结束 HotReloadableMapScene 实现。

/// 标记已移除旧碰撞、等待下一帧重新插入碰撞层级构造器的地图根。
#[derive(Component, Clone, Copy, Debug)] // 保存真实渲染帧身份，不能依赖会被F12暂停的固定时钟。
pub(crate) struct MapReloadPending {
    pub(crate) detected_frame: u32, // 必须跨过Modified所在Update帧后才能读取Bevy更新完成的场景层级。
}

/// 标记已经重新生成 WorldAssetRoot、正在等待 Avian 碰撞完成的地图根。
#[derive(Component)] // 纯生命周期标记投影到公共世界暂停原因，不直接拥有时间资源。
pub(crate) struct MapReloadAwaitingCollision; // 最后一个Pending/Awaiting消失后统一暂停系统才恢复玩法时间。

/// 保存场景细节系统首次接管前的作者 LOD 范围及其最远档身份。
#[derive(Component, Clone, PartialEq)] // 热重载与切档都从不可变作者值重新派生，避免累计缩放漂移。
pub(crate) struct SceneDetailAuthoredVisibilityRange {
    pub(crate) authored: VisibilityRange, // 高档恢复时精确写回地图或原型声明的原始范围。
    pub(crate) farthest_lod: bool,        // 只有最远档的退出边界使用独立最大显示距离倍率。
} // 完成固定景观视觉范围所有权备份。
