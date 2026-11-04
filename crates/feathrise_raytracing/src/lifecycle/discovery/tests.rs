use super::*; // 锁定动态目标的身份、预算与停止线。
use crate::constants::{
    DYNAMIC_MAX_TERRAIN_DERIVED_BYTES,
}; // 正式路径按候选实际派生字节执行同一停止线。

#[test]
fn dynamic_budget_accepts_small_map_and_rejects_oversized_terrain() {
    let small = RaytracingBudgetSnapshot {
        instances: DYNAMIC_EXPECTED_INSTANCE_COUNT,
        vertices: 4_225,
        triangles: 8_192,
        unique_materials: DYNAMIC_EXPECTED_UNIQUE_MATERIAL_COUNT,
        derived_bytes: 4_225 * 48 + 8_192 * 3 * 4,
        cpu_working_set_bytes: 0,
    };
    assert!(validate_dynamic_budget(small).is_ok()); // 100×100米测试图无需伪装成513²固定资产。
    assert!(
        validate_dynamic_budget(RaytracingBudgetSnapshot {
            vertices: DYNAMIC_MAX_TERRAIN_VERTEX_COUNT + 1,
            ..small
        })
        .is_err()
    ); // 超过已批准单地形规模时保持光栅，不能悄悄扩大TLAS预算。
}

#[test]
fn single_terrain_payload_fits_the_per_frame_adoption_gate() {
    assert!(DYNAMIC_MAX_TERRAIN_DERIVED_BYTES <= MAX_UPLOAD_BYTES_PER_FRAME); // 最大受支持地形必须能作为不可拆分Mesh原子采用。
    assert!(MAX_UPLOAD_BYTES_PER_FRAME < MAX_DERIVED_MESH_BYTES); // 单项门仍严格小于完整事务的总派生上限。
}
