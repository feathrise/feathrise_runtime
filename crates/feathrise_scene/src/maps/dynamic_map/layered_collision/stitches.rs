//! 物理只加密实际碰到外边的三角形；共同曲线节点与显示LOD独立，不把粗碰撞边跨过曲线拐点。
use super::types::ChunkGeometry;
use bevy::prelude::*;
use feathrise_environment::vegetation::blade::GpuBladeClipmapStitches;
use feathrise_map_content::SparseHeightSpec;
use std::collections::BTreeMap;

pub(super) fn refine(
    spec: &SparseHeightSpec,
    mesh: &mut ChunkGeometry,
    stitches: &GpuBladeClipmapStitches,
) -> Result<(), String> {
    if !stitches.active() {
        return Ok(());
    }
    let min = Vec2::from_array(spec.world_min);
    let max = Vec2::from_array(spec.world_max);
    let mut known: BTreeMap<[u32; 2], u32> = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, p)| ([p.x.to_bits(), p.z.to_bits()], i as u32))
        .collect();
    let old = std::mem::take(&mut mesh.triangles);
    let mut triangles = Vec::with_capacity(old.len());
    let vertex = |p: Vec2, mesh: &mut ChunkGeometry, known: &mut BTreeMap<[u32; 2], u32>| {
        let key = [p.x.to_bits(), p.y.to_bits()];
        if let Some(i) = known.get(&key) {
            return *i;
        }
        let i = mesh.vertices.len() as u32;
        mesh.vertices.push(Vec3::new(p.x, 0.0, p.y));
        mesh.base_grid_points.push([
            (f64::from(p.x) - f64::from(spec.world_min[0])) / spec.actual_spacing_meters[0],
            (f64::from(p.y) - f64::from(spec.world_min[1])) / spec.actual_spacing_meters[1],
        ]);
        known.insert(key, i);
        i
    };
    for triangle in old {
        let points = triangle.map(|i| mesh.vertices[i as usize].xz());
        let mut polygon = Vec::with_capacity(8);
        for side in 0..3 {
            polygon.push(triangle[side]);
            let a = points[side];
            let b = points[(side + 1) % 3];
            for edge in 0..4 {
                let info = stitches.edges[edge];
                if info.x < 2.0 {
                    continue;
                }
                let axis = if edge < 2 { 1 } else { 0 };
                let normal = 1 - axis;
                let bound = if edge % 2 == 0 {
                    min[normal]
                } else {
                    max[normal]
                };
                if (a[normal] - bound).abs() > 0.00001
                    || (b[normal] - bound).abs() > 0.00001
                    || (a[axis] - b[axis]).abs() < 0.00001
                {
                    continue;
                }
                let mut samples = Vec::new();
                for i in 1..info.x as usize - 1 {
                    let v = min[axis] + i as f32 * info.y;
                    let t = (v - a[axis]) / (b[axis] - a[axis]);
                    if t > 0.000001 && t < 0.999999 {
                        let mut p = a;
                        p[axis] = v;
                        p[normal] = bound;
                        samples.push((t, p));
                    }
                }
                samples.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (_, p) in samples {
                    polygon.push(vertex(p, mesh, &mut known));
                }
            }
        }
        if polygon.len() == 3 {
            triangles.push(triangle);
            continue;
        }
        let center = vertex((points[0] + points[1] + points[2]) / 3.0, mesh, &mut known);
        for side in 0..polygon.len() {
            triangles.push([center, polygon[side], polygon[(side + 1) % polygon.len()]]);
        }
        if mesh.vertices.len() > 65_536 || triangles.len() > 131_072 {
            return Err("拼接边物理细分超过既有单块预算".into());
        }
    }
    mesh.triangles = triangles;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rectangle(step: usize) -> ChunkGeometry {
        let count = 64 / step;
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for z in 0..=count {
            for x in 0..=count {
                vertices.push(Vec3::new((x * step) as f32, 0., (z * step) as f32));
            }
        }
        for z in 0..count {
            for x in 0..count {
                let a = (z * (count + 1) + x) as u32;
                let b = a + 1;
                let c = a + (count + 1) as u32;
                let d = c + 1;
                triangles.extend([[a, c, b], [b, c, d]]);
            }
        }
        let base_grid_points = vertices.iter().map(|p| [p.x as f64, p.z as f64]).collect();
        ChunkGeometry {
            vertices,
            triangles,
            base_grid_points,
        }
    }
    #[test]
    fn stitch_collision_retains_common_curve_knots_across_different_physics_lods() {
        let spec = SparseHeightSpec::new([0.; 2], [64.; 2], 1., 0., 32.).unwrap();
        let mut stitches = GpuBladeClipmapStitches::default();
        stitches.edges[1] = Vec4::new(33., 2., 8., 1.);
        for i in 0..=32 {
            stitches.heights[1][i / 4][i % 4] = (i as f32 * 0.27).sin() * 3. + 8.;
        }
        let mut expected = None;
        for step in [2, 4, 16, 64] {
            let mut mesh = rectangle(step);
            refine(&spec, &mut mesh, &stitches).unwrap();
            let mut boundary: Vec<_> = mesh
                .vertices
                .iter()
                .filter(|p| p.x == 64.)
                .map(|p| p.z as u32)
                .collect();
            boundary.sort();
            boundary.dedup();
            assert_eq!(boundary, (0..=32).map(|i| i * 2).collect::<Vec<_>>());
            // 真实细分后的三角边必须保留每个拐点，不能由一个粗边跨越多个节点。
            let mut segments = std::collections::BTreeSet::new();
            for triangle in &mesh.triangles {
                let p = triangle.map(|i| mesh.vertices[i as usize]);
                assert!((p[1] - p[0]).cross(p[2] - p[0]).y > 0.0, "绕序必须朝上");
                for i in 0..3 {
                    let (a, b) = (p[i], p[(i + 1) % 3]);
                    if a.x == 64. && b.x == 64. {
                        segments.insert(((a.z.min(b.z) / 2.) as u32, (a.z.max(b.z) / 2.) as u32));
                    }
                }
            }
            assert_eq!(segments, (0..32).map(|i| (i, i + 1)).collect());
            if let Some(ref previous) = expected {
                assert_eq!(&segments, previous);
            } else {
                expected = Some(segments);
            }
            assert_eq!(mesh.base_grid_points.len(), mesh.vertices.len());
        }
    }
}
