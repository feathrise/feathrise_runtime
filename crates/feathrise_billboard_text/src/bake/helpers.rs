// bake/helpers.rs — 固定相机池的纯状态辅助函数与回归测试。

use crate::types::BillboardBakeSlot; // 纯函数只读取槽位状态，不访问ECS World。

/// 返回第一个空闲固定相机槽位，确保所有任务都受池容量硬上限约束。
pub(super) fn first_idle_slot(slots: &[BillboardBakeSlot]) -> Option<usize> {
    slots.iter().position(|slot| slot.billboard.is_none()) // FIFO任务始终优先占用编号最小的空闲槽位。
}

/// 推进最终纹理保留帧；只有布局完成且本次归零时才允许释放槽位。
pub(super) fn advance_bake_retirement(
    needs_layout_sync: bool,   // true表示尚未得到可用于裁切的最终文字布局。
    frames_remaining: &mut u8, // 布局完成后仍需跨过的Update计数。
) -> bool {
    if needs_layout_sync || *frames_remaining == 0 {
        // 布局未完成或已经归零时保持状态不变。
        return false; // 当前帧不能释放相机槽位。
    }
    *frames_remaining -= 1; // 为渲染提取保留至少一个写入最终纹理的完整帧。
    *frames_remaining == 0 // 仅从1变为0的这一次请求完成清理。
}

#[cfg(test)] // 纯状态测试不启动渲染器，验证相机并发硬上限和烘焙帧时序。
mod tests {
    use bevy::prelude::*; // World用于生成有效且互不重复的Entity标识。

    use super::{advance_bake_retirement, first_idle_slot}; // 直接测试生产系统调用的纯函数。
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

    #[test] // 最终文字布局完成后应保留一个完整渲染帧再释放固定槽位。
    fn baked_billboard_slot_retires_after_one_final_render_frame() {
        let mut frames_remaining = 2; // 新任务从两个Update计数开始，为最终渲染留出一帧。
        assert!(!advance_bake_retirement(true, &mut frames_remaining)); // 布局未完成时不得推进。
        assert_eq!(frames_remaining, 2); // 等布局期间帧预算保持不变。
        assert!(!advance_bake_retirement(false, &mut frames_remaining)); // 第一次推进只准备最终渲染。
        assert_eq!(frames_remaining, 1); // 当前帧末尾仍由固定相机写入最终纹理。
        assert!(advance_bake_retirement(false, &mut frames_remaining)); // 下一次Update才允许归还槽位。
        assert_eq!(frames_remaining, 0); // 完成后计数稳定归零。
        assert!(!advance_bake_retirement(false, &mut frames_remaining)); // 已完成任务不得重复释放。
    }
}
