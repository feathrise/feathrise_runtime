// bake/helpers.rs — 固定相机池的纯状态辅助函数与回归测试。

use crate::types::BillboardBakeSlot; // 纯函数只读取槽位状态，不访问ECS World。

/// 返回第一个空闲固定相机槽位，确保所有任务都受池容量硬上限约束。
pub(super) fn first_idle_slot(slots: &[BillboardBakeSlot]) -> Option<usize> {
    slots.iter().position(|slot| slot.billboard.is_none()) // FIFO任务始终优先占用编号最小的空闲槽位。
}

/// 必须同时具有最终布局和当前非零代次的提交回执；旧回执与尚未绘制均保持隐藏。
pub(super) fn bake_draw_is_complete(needs_layout_sync: bool, generation: u64, rendered_generation: u64) -> bool {
    !needs_layout_sync && generation != 0 && rendered_generation == generation
}

#[cfg(test)] // 纯状态测试不启动渲染器，验证相机并发硬上限和渲染回执身份。
mod tests {
    use bevy::prelude::*; // World用于生成有效且互不重复的Entity标识。

    use super::{bake_draw_is_complete, first_idle_slot}; // 直接测试生产系统调用的纯函数。
    use crate::constants::DEFAULT_BAKE_POOL_SIZE; // 回归默认固定池容量。
    use crate::types::BillboardBakeSlot; // 构造与生产资源相同的槽位数据。

    #[test] // 任意数量任务都不能把活跃槽位数推高到固定容量之外。
    fn fixed_pool_never_claims_more_than_four_slots() {
        let mut world = World::new(); // 只用World分配稳定Entity，不加载任何Bevy插件。
        let camera_entities: Vec<Entity> = (0..DEFAULT_BAKE_POOL_SIZE) // 为每个固定槽位生成一台模拟相机。
            .map(|_| world.spawn_empty().id()) // 每个槽位必须长期绑定不同相机实体。
            .collect(); // 收集后用于验证复用期间相机标识不变化。
        let mut slots: Vec<BillboardBakeSlot> =
            camera_entities // 按正式池结构创建四个空闲槽位。
                .iter() // 遍历固定相机实体。
                .enumerate() // 编号同时作为模拟渲染层偏移。
                .map(|(index, camera_entity)| BillboardBakeSlot {
                    // 每个槽位初始均为空闲。
                    camera_entity: *camera_entity, // 固定相机实体整个测试期间不替换。
                    layer: index,                  // 测试只需保证不同槽位使用不同层。
                    billboard: None,               // 初始没有正在烘焙的任务。
                    generation: 0,
                    rendered_generation: default(),
                })
                .collect(); // 得到与默认容量完全一致的固定池。

        for _ in 0..DEFAULT_BAKE_POOL_SIZE {
            // 连续提交四个任务应依次占满全部槽位。
            let billboard = world.spawn_empty().id(); // 为本轮生成一个模拟Billboard任务。
            let slot_index = first_idle_slot(&slots).expect("固定池尚未占满时必须存在空闲槽位"); // 获取唯一空闲位置。
            slots[slot_index].billboard = Some(billboard); // 模拟生产系统成功分配任务。
        }

        assert!(first_idle_slot(&slots).is_none()); // 第五个任务必须排队，不能生成第五台相机。
        assert_eq!(
            slots.iter().filter(|slot| slot.billboard.is_some()).count(),
            4
        ); // 活跃并发严格等于固定容量。

        let reused_camera = slots[1].camera_entity; // 记录即将释放槽位原本绑定的固定相机。
        slots[1].billboard = None; // 模拟第二个任务烘焙完成并归还槽位。
        assert_eq!(first_idle_slot(&slots), Some(1)); // 下一任务应复用刚释放的同一槽位。
        assert_eq!(slots[1].camera_entity, reused_camera); // 复用不得创建或替换Camera2d实体。
    }

    #[test] // 冷启动、布局未就绪、旧任务或重复回执均不能被CPU帧数绕过。
    fn only_current_render_receipt_completes_a_laid_out_bake() {
        assert!(!bake_draw_is_complete(false, 0, 0));
        assert!(!bake_draw_is_complete(true, 1, 1));
        assert!(!bake_draw_is_complete(false, 1, 0));
        assert!(!bake_draw_is_complete(false, 2, 1));
        assert!(!bake_draw_is_complete(false, 2, 3));
        assert!(bake_draw_is_complete(false, 2, 2));
        assert!(bake_draw_is_complete(false, 2, 2)); // 纯判据幂等；清理系统由槽位身份确保只退休一次。
    }
}
