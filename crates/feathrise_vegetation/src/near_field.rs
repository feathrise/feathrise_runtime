//! 与远场布局共享候选的近场实体代理、空间索引和确定性差分发布。

mod collection; // 纯候选收集不接触ECS可变世界，允许按层安全并行。
mod spatial_index; // 显式稳定落点按世界X/Z建立只读均匀网格索引。
mod systems; // ECS系统只负责观察变化、调用纯计算并串行发布消息。

#[cfg(test)] // 近场行为、空间索引和执行策略共同使用真实插件回归。
mod tests; // 测试单独拆分，避免生产模块超过项目单文件边界。

use feathrise_scene_core::residency::SceneResidencyAppExt;

use std::collections::BTreeMap; // 活动代理按层和实例ID保持稳定顺序。

use bevy::prelude::{App, Component, Plugin, Resource, Transform, Update}; // 公开ECS类型与插件注册所需最小Bevy接口。

use crate::{
    // 近场代理沿用核心公开稳定身份，不引入内容类别。
    VegetationLayerId,          // 层ID与实例ID共同组成全局近场主键。
    VegetationNearFieldEntered, // 消费项目根据进入消息创建物理或玩法代理。
    VegetationNearFieldExited,  // 消费项目根据退出消息确定性回收代理。
    VegetationPrototypeId,      // 原型ID让同一机制表达树木、岩石等任意内容。
}; // 完成公共核心类型导入。

use self::spatial_index::ExplicitPlacementSpatialIndexes; // 运行资源持有与配置代次一致的只读索引集合。

/// 标记会驱动近场代理内圈的玩家或观察者。
#[derive(Clone, Copy, Debug, Default, Component)] // 标记无字段且允许多个观察者共同决定驻留集合。
pub struct VegetationNearFieldObserver; // 多观察者结果合并，不绑定具体相机或玩家组件。

#[derive(Clone, Debug, PartialEq)] // 差分比较会检测同稳定ID下的原型或变换变化。
struct ActiveNearFieldProxy {
    // 仅近场内部保存消费者应拥有的完整代理描述。
    prototype: VegetationPrototypeId, // 层配置改变原型时先退出旧代理再进入新代理。
    transform: Transform,             // 消费者接收的远近一致最终变换。
} // 完成单个活动近场代理定义。

/// 当前已经向消费者发布的稳定近场代理集合。
#[derive(Clone, Debug, Default, Resource)] // 资源跨帧保存滞回集合、变化身份和空间索引。
pub struct VegetationNearFieldRuntime {
    // 公开资源只暴露只读统计方法，内部状态由插件独占维护。
    active: BTreeMap<(VegetationLayerId, u64), ActiveNearFieldProxy>, // 不允许不同层相同ID互相覆盖。
    last_layout_revision: u64,  // 布局变化时即使观察者静止也重新差分。
    last_state_generation: u64, // 配置或密度源变化都必须重新执行最终启用判断。
    last_observers: Vec<bevy::prelude::Vec2>, // 观察者位置未变时不运行候选枚举。
    explicit_indexes: ExplicitPlacementSpatialIndexes, // 显式落点索引只随层配置代次重建。
} // 完成近场跨帧运行状态定义。

impl VegetationNearFieldRuntime {
    // 对外只提供不会破坏内部确定性的只读查询。
    /// 返回当前已发布但尚未退出的代理数量。
    pub fn active_count(&self) -> usize {
        // 统计不暴露私有代理值或可变集合。
        self.active.len() // 消费者可用于F3和滞回回归。
    } // 完成活动数量查询。

    /// 查询一层稳定实例是否位于当前近场集合。
    pub fn contains(&self, layer: VegetationLayerId, instance_id: u64) -> bool {
        // 联合主键避免不同层ID冲突。
        self.active.contains_key(&(layer, instance_id)) // 层和实例共同构成唯一身份。
    } // 完成单实例驻留查询。
} // 完成近场运行资源公开接口。

/// 安装近场候选差分消息；Avian与玩法交互由工程另行适配。
#[derive(Clone, Copy, Debug, Default)] // 插件无配置状态且可按Bevy惯例直接构造。
pub struct VegetationNearFieldPlugin; // 插件不依赖任何物理crate。

impl Plugin for VegetationNearFieldPlugin {
    // 插件只注册资源、消息和唯一近场更新系统。
    fn build(&self, app: &mut App) {
        // 所有ECS写回都留在主应用Update调度中。
        app.init_resident_resource::<VegetationNearFieldRuntime>() // 跨帧保存滞回集合与显式空间索引。
            .add_message::<VegetationNearFieldEntered>() // 消费者按需生成实体、碰撞或交互。
            .add_message::<VegetationNearFieldExited>() // 消费者确定性回收代理。
            .add_resident_systems(Update, systems::update_near_field_proxies); // 只在观察者或布局变化时执行候选差分。
    } // 完成近场插件注册。
} // 完成近场插件实现。
