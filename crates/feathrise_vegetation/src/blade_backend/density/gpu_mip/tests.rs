//! GPU Mip调度计划与uniform ABI的纯CPU回归。

use super::plan::GpuMipReadbackPlan;
use super::*; // 直接锁定内部调度描述，不需要创建渲染设备。 // 回读对齐计划与运行时节点使用同一实现。

#[test]
fn rectangular_extent_reaches_one_by_one_through_adjacent_levels() {
    let dispatches = build_gpu_mip_dispatches(128, 64).unwrap(); // 两个水平64块对应正式图集常见范围。
    let extents = dispatches
        .iter()
        .map(|dispatch| {
            (
                dispatch.source_level,
                dispatch.target_level,
                dispatch.source_width,
                dispatch.source_height,
                dispatch.target_width,
                dispatch.target_height,
            )
        })
        .collect::<Vec<_>>(); // 收集完整链便于一次锁定级号和尺寸。
    assert_eq!(
        extents,
        vec![
            (0, 1, 128, 64, 64, 32),
            (1, 2, 64, 32, 32, 16),
            (2, 3, 32, 16, 16, 8),
            (3, 4, 16, 8, 8, 4),
            (4, 5, 8, 4, 4, 2),
            (5, 6, 4, 2, 2, 1),
            (6, 7, 2, 1, 1, 1),
        ]
    ); // 非正方形轴收敛后继续重复边缘直到另一轴也到一。
}

#[test]
fn dispatch_uniform_and_workgroups_match_shader_abi() {
    let dispatch = GpuMipDispatch {
        source_level: 3,  // 级号不进入uniform，只用于纹理视图选择。
        target_level: 4,  // 级号不进入uniform，只用于纹理视图选择。
        source_width: 17, // 使用非八倍数暴露向上取工作组规则。
        source_height: 9, // 使用非八倍数暴露向上取工作组规则。
        target_width: 8,  // 下一层遵循向下整除二。
        target_height: 4, // 下一层遵循向下整除二。
    };
    assert_eq!(
        dispatch.uniform_bytes(),
        [17, 0, 0, 0, 9, 0, 0, 0, 8, 0, 0, 0, 4, 0, 0, 0]
    ); // 四个小端u32与WGSL两个vec2字段完全一致。
    assert_eq!(dispatch.workgroups(), (1, 1)); // 8×4目标只需一个8×8工作组。
}

#[test]
fn zero_extent_is_rejected_before_gpu_resource_creation() {
    assert!(build_gpu_mip_dispatches(0, 64).is_err()); // 零宽不得进入WebGPU描述符。
    assert!(build_gpu_mip_dispatches(64, 0).is_err()); // 零高不得进入WebGPU描述符。
    assert!(build_gpu_mip_dispatches(1, 1).unwrap().is_empty()); // 合法1×1不需要任何降采样。
}

#[test]
fn readback_plan_decodes_all_padded_rows_into_tight_mip_order() {
    let plan = GpuMipReadbackPlan::new(2, 2, 2).unwrap(); // 2×2基础级加1×1子级。
    let mut mapped = vec![0_u8; plan.buffer_size() as usize]; // 模拟256字节对齐staging映射。
    let base = plan.levels()[0]; // 基础级有两行且每行八个有效RGBA字节。
    mapped[base.offset as usize..base.offset as usize + 8]
        .copy_from_slice(&[1, 1, 1, 255, 2, 2, 2, 255]); // 写基础级第一有效行。
    let second_row = base.offset as usize + base.padded_row_bytes as usize; // 跳过第一行填充。
    mapped[second_row..second_row + 8].copy_from_slice(&[3, 3, 3, 255, 4, 4, 4, 255]); // 写基础级第二有效行。
    let child = plan.levels()[1]; // 子级起点独立对齐。
    mapped[child.offset as usize..child.offset as usize + 4].copy_from_slice(&[3, 3, 3, 255]); // 写1×1子级。
    assert_eq!(
        plan.decode(&mapped).unwrap(),
        vec![
            1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255, 3, 3, 3, 255,
        ]
    ); // 解码顺序严格为基础级紧凑行后接子级。
}

#[test]
fn readback_plan_requires_the_complete_chain_level_count() {
    assert!(GpuMipReadbackPlan::new(4, 2, 2).is_err()); // 4×2需要4×2、2×1、1×1三级。
    assert!(GpuMipReadbackPlan::new(4, 2, 3).is_ok()); // 完整级数通过。
}
