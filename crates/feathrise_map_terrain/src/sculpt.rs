//! 高度雕刻只操作拥有网格和共享拓扑；调用方负责世界/源坐标转换和历史采用。
use std::collections::BTreeMap;
use crate::{TerrainMeshInput, TerrainEditingIndex, TerrainHeightBrushKind, TerrainHeightBrush, TerrainVertexHeightChange};

/// 同XYZ属性拆点统一写Y，洞顶和洞底按真实连通层区分；全部目标算完才改写网格。
pub fn sculpt_height_vertices(mesh: &mut TerrainMeshInput, index: &TerrainEditingIndex, brush: TerrainHeightBrush) -> Result<Vec<TerrainVertexHeightChange>, String> {
    if !brush.center.is_finite() || !brush.radius.is_finite() || brush.radius <= 0.0
        || !brush.strength.is_finite() || !brush.delta_seconds.is_finite() || !brush.delta_height.is_finite()
        || !brush.minimum.is_finite() || !brush.maximum.is_finite() || brush.minimum >= brush.maximum {
        return Err("地形雕刻的范围、强度或高度量程无效".into());
    }
    let affected = index.connected_representatives_in_circle(&mesh.positions, &mesh.indices,
        [brush.center.x, brush.center.z], brush.radius, brush.center.to_array(), brush.hit_triangle)
        .into_iter().collect::<BTreeMap<_, _>>();
    if affected.len() > 262_144 { return Err("一次高度雕刻超过262144个作者点；请缩小半径".into()); }
    let mut source = BTreeMap::new();
    for &vertex in affected.keys() {
        source.insert(vertex, index.logical_height(&mesh.positions, vertex).ok_or("地形拓扑索引与作者顶点不一致")?);
        if matches!(brush.kind, TerrainHeightBrushKind::Smooth) {
            for neighbor in index.neighbors(vertex) { let neighbor = neighbor as usize;
                if !source.contains_key(&neighbor) {
                    source.insert(neighbor, index.logical_height(&mesh.positions, neighbor).ok_or("地形邻接索引失效")?);
                }
            }
        }
    }
    let mut targets = Vec::with_capacity(affected.len());
    for (&vertex, &distance) in &affected {
        let old = source[&vertex]; let falloff = (1.0 - distance / brush.radius).clamp(0.0, 1.0);
        let next = match brush.kind {
            TerrainHeightBrushKind::RaiseLower => old + brush.delta_height * falloff,
            TerrainHeightBrushKind::Flatten => {
                let target = brush.flatten_target.filter(|target| target.is_finite()).ok_or("整平缺少冻结的真实高度")?;
                move_towards_height(old, f64::from(target), brush.delta_height, falloff)
            }
            TerrainHeightBrushKind::Smooth => {
                let mut sum = f64::from(old); let mut count = 1;
                for neighbor in index.neighbors(vertex) { if let Some(height) = source.get(&(neighbor as usize)) { sum += f64::from(*height); count += 1; } }
                move_towards_height(old, sum / f64::from(count), brush.delta_height, falloff)
            }
        }.clamp(brush.minimum, brush.maximum);
        if !next.is_finite() { return Err("地形雕刻生成非有限高度".into()); }
        targets.push((vertex, next));
    }
    let mut changes = Vec::new();
    for (representative, after) in targets { for vertex in index.coincident_vertices(representative) {
        let index = vertex as usize; let before = mesh.positions[index][1];
        if before != after { changes.push(TerrainVertexHeightChange { index, before, after }); mesh.positions[index][1] = after; }
    } }
    Ok(changes)
}

/// 不再由固定收敛系数限制平滑/压平速度；有限极值也先用f64求差，整刷校验后才写入。
fn move_towards_height(old: f32, target: f64, delta_height: f32, falloff: f32) -> f32 {
    let step = f64::from(delta_height.abs()) * f64::from(falloff);
    (f64::from(old) + (target - f64::from(old)).clamp(-step, step)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;

    #[test]
    fn smooth_and_flatten_consume_the_meter_budget_without_overshooting() {
        for kind in [TerrainHeightBrushKind::Smooth, TerrainHeightBrushKind::Flatten] {
            for (budget, expected) in [(0., 10.), (0.2, 9.8), (2., 8.), (100.,
                if matches!(kind, TerrainHeightBrushKind::Smooth) { 10. / 3. } else { -5. })] {
                let mut mesh = TerrainMeshInput { positions: vec![[0., 10., 0.], [1., 0., 0.], [0., 0., 1.]], indices: vec![0, 2, 1] };
                let index = crate::build_terrain_editing_index(&mesh).unwrap();
                let changes = sculpt_height_vertices(&mut mesh, &index, TerrainHeightBrush { kind,
                    center: Vec3::new(0., 10., 0.), radius: 0.5, strength: 1., delta_seconds: 0.05,
                    delta_height: budget, flatten_target: Some(-5.), minimum: -100., maximum: 100., hit_triangle: Some(0) }).unwrap();
                assert!((mesh.positions[0][1] - expected).abs() < 0.00001);
                assert_eq!(changes.is_empty(), budget == 0.);
                assert_eq!(mesh.positions[1][1], 0., "圆外邻点只读，平滑不能顺序污染目标");
            }
        }
    }

    #[test]
    fn finite_extreme_smoothing_and_flattening_do_not_overflow_intermediate_math() {
        for kind in [TerrainHeightBrushKind::Smooth, TerrainHeightBrushKind::Flatten] {
            let mut mesh = TerrainMeshInput { positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]], indices: vec![0, 2, 1] };
            let index = crate::build_terrain_editing_index(&mesh).unwrap();
            // 索引只冻结拓扑，读取的Y允许改变；极值计算与源文件几何面积校验分开验证。
            mesh.positions[0][1] = f32::MAX;
            mesh.positions[1][1] = -f32::MAX;
            mesh.positions[2][1] = -f32::MAX;
            sculpt_height_vertices(&mut mesh, &index, TerrainHeightBrush { kind,
                center: Vec3::new(0., f32::MAX, 0.), radius: 0.5, strength: 1., delta_seconds: 0.05,
                delta_height: f32::MAX, flatten_target: Some(-f32::MAX), minimum: -f32::MAX,
                maximum: f32::MAX, hit_triangle: Some(0) }).unwrap();
            assert_eq!(mesh.positions[0][1], 0.);
        }
    }
}
