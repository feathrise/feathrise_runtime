//! 统一环境运行时入口：聚合通用种植核心、可插拔后端、共享Mesh风与树木/景观领域入口。
//! 【功能设计】种植分布、空间竞争和足迹净空统一由vegetation核心负责；专业渲染后端不声明内容类别。

use bevy_app::{App, Plugin}; // 使用最小Bevy应用接口实现一个可选功能驱动的统一插件。

#[cfg(feature = "landscape")] // 只有消费者启用landscape时才编译并暴露固定景观公共API。
pub mod landscape; // 灌木和岩石继续使用独立稳定实例、GroundSnap与距离LOD实现。
#[cfg(feature = "mesh-wind")] // 共享PBR Mesh风材质可脱离具体树木与景观内容域独立编译。
pub mod mesh_wind; // 树木和灌木通过同一材质、噪声纹理与保守AABB接口驱动顶点风。
#[cfg(feature = "terrain-surface")] // 地表与水面共享显式轻量遮雨体，不依赖物理后端。
pub mod rain_shelter; // 提供作者组件、CPU受雨率和固定GPU缓冲。
#[cfg(feature = "terrain-surface")] // 局部水面高度图只属于水域/地表渲染组合。
pub mod rain_water; // 天气消费者与场景生产者共用图像和范围契约。
#[cfg(feature = "terrain-surface")] // 地形PBR权重混合不依赖任何植被后端。
pub mod terrain_surface;
#[cfg(feature = "tree")] // 只有消费者启用tree时才编译并暴露树木公共API。
pub mod tree; // 树木继续使用独立固定树位、GPU风材质、渲染统计和碰撞身份实现。 // 通用八层地表材质、RenderApp就绪证据与白模接管公共入口。
pub use feathrise_vegetation as vegetation; // 所有feature组合都公开无限层、竞争组、足迹与近场代理核心。

/// 常用统一入口重导出，工程可只依赖本 crate并从专业子命名空间取得类型。
pub mod prelude {
    // 保持入口轻量，不把三类crate的同名类型全部拍平到根命名空间。
    #[cfg(feature = "landscape")] // landscape功能开启时提供对应命名空间。
    pub use crate::landscape; // 使用`landscape::LandscapePrototype`等完整原始API。
    #[cfg(feature = "mesh-wind")] // mesh-wind功能开启时提供共享顶点风命名空间。
    pub use crate::mesh_wind; // 使用`mesh_wind::TreeWindMaterial`等具体Mesh风API。
    #[cfg(feature = "terrain-surface")] // 地表功能开启时提供独立命名空间。
    pub use crate::terrain_surface; // 使用`terrain_surface::TerrainSurfaceMaterial`等通用API。
    #[cfg(feature = "tree")] // tree功能开启时提供对应命名空间。
    pub use crate::tree; // 使用`tree::TreePrototype`等完整原始API。
    pub use crate::{EnvironmentPlugin, vegetation}; // 所有feature组合都公开统一插件和通用种植核心。
} // 完成统一prelude定义。

/// 按Cargo feature安装环境渲染能力的统一顶层插件。
#[derive(Debug, Default, Clone, Copy)] // 插件没有运行时配置，允许应用按值直接添加并便于测试构造。
pub struct EnvironmentPlugin; // 具体地图资产、稳定落点、密度图和生命周期仍由消费者负责。

impl Plugin for EnvironmentPlugin {
    // 聚合层保持原有插件安装顺序并避免跨实现隐式耦合。
    fn build(&self, app: &mut App) {
        app.add_plugins((
            vegetation::VegetationPlugin, // 所有环境组合共享事件触发的无限层布局核心。
            vegetation::VegetationNearFieldPlugin, // 只发布稳定代理差分，不绑定Avian或玩法。
        ));
        #[cfg(feature = "blade")] // 正式消费者通过正交blade feature安装叶片后端。
        app.add_plugins(vegetation::VegetationBladePlugin); // 保持288字节八档窗口、16字节实例与四外观槽批次ABI。
        #[cfg(feature = "mesh")] // 通用多部件PBR后端不声明树木、灌木或岩石类别。
        app.add_plugins((
            vegetation::VegetationMeshPlugin, // 安装变化驱动的通用Mesh生命周期与公告板诊断入口。
            vegetation::VegetationMeshBillboardPlugin, // 安装动态公告板逐视图GPU材质与内置Shader。
        )); // 固定公告板仍保留原材质，只有显式扩展材质进入自定义顶点管线。
        #[cfg(all(feature = "mesh-wind", not(feature = "tree")))]
        // tree开启时由TreePlugin在原有位置代为安装，避免重复插件。
        app.add_plugins(mesh_wind::MeshWindPlugin); // 独立共享风组合也能取得噪声资源和完整扩展材质管线。
        #[cfg(feature = "tree")] // 树木运行时保持独立于海量草地Compute链路。
        app.add_plugins(tree::TreePlugin); // 安装树木诊断、GPU风噪声和顶点风材质管线。
        #[cfg(feature = "landscape")] // 固定景观运行时只注册自身轻量诊断资源。
        app.add_plugins(landscape::LandscapePlugin); // 安装灌木与岩石稳定实例公共底座。
        #[cfg(feature = "terrain-surface")] // 地表插件独立安装，不再由GPU叶片插件间接拥有。
        app.add_plugins(terrain_surface::TerrainSurfacePlugin); // 注册权重材质、纹理探针和RenderApp管线观察器。
    } // 完成全部已启用环境插件的确定性注册。
} // 完成统一插件实现。
