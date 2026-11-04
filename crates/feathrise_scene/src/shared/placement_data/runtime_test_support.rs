//! 下游CPU回归的真实Pending摆放身份；不模拟正式加载、GPU就绪或容器物理采用。

use super::components::{MapPlacementInstance, PlacementInstanceGate};
use feathrise_map_content::{MapId, MapPlacementRecord};

/// 仅测试feature导出的共享门句柄；状态变化不重施MPI，确保回归消费真实生命周期约束。
pub struct PendingPlacementActivation(PlacementInstanceGate);

impl PendingPlacementActivation {
    pub fn activate(&self) {
        self.0.activate();
    }

    pub fn retire(&self) {
        self.0.retire();
    }
}

/// 返回正常spawn即可触发Add观察的Pending组件及它的精确门句柄。
pub fn pending_placement_instance(
    map: &MapId,
    record: &MapPlacementRecord,
) -> (MapPlacementInstance, PendingPlacementActivation) {
    let gate = PlacementInstanceGate::new(false);
    (MapPlacementInstance::new_pending(map, record, gate.clone()), PendingPlacementActivation(gate))
}
