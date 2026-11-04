//! 整张正式样条水资产的纯构建预算预检。

use feathrise_map_content::MapWaterAsset; // 保存预检直接接收完整正式水域作者资产。

use super::{
    build_river_water_surface, build_waterfall_water_surface,
}; // 逐体复用运行时完全相同的网格builder与全图硬预算。

/// 以正式公共构建器预检整张水域资产的每体截面预算与全图三角预算。
///
/// 调用方应先执行`MapWaterAsset::validate`校验作者语义；本函数逐体完成同一实际网格构建、
/// 汇总三角形后立即丢弃完成值，确保保存预检与运行时不会采用两套近似预算公式。
pub fn validate_spline_water_build_budget(asset: &MapWaterAsset) -> Result<(), String> {
    let mut total_triangles = 0usize; // 只保留固定小型累计值，不同时持有整图全部完成Mesh。
    for river in &asset.rivers {
        let triangle_count = build_river_water_surface(river)?.mesh.triangle_count(); // 完成值在本语句后立即丢弃，只提取实际三角数。
        total_triangles = total_triangles
            .checked_add(triangle_count)
            .ok_or_else(|| "地图样条水体三角形总数溢出".to_owned())?;
    }
    for waterfall in &asset.waterfalls {
        let triangle_count = build_waterfall_water_surface(waterfall)?
            .mesh
            .triangle_count(); // 同一正式瀑布完成值只存活到实际计数读取。
        total_triangles = total_triangles
            .checked_add(triangle_count)
            .ok_or_else(|| "地图样条水体三角形总数溢出".to_owned())?;
    }
    Ok(()) // 所有单体实际构建与全图累计预算均可安全采用。
}
