//! 模型稀疏锚点在原三角面内细分；生成量受上传预算约束，几何边界与叠层不靠额外射线猜测。
use bevy::prelude::*;
use feathrise_scene_content::ModelSurfaceAnchor;
use super::{MeshSurfaceSnapshot, SurfaceHit};

impl MeshSurfaceSnapshot {
    /// 每项是对象局部点、法线和代表表面积；小笔刷非网格锚点只保留自己的精确位置。
    pub fn vegetation_candidates(&self, anchor: ModelSurfaceAnchor, author_spacing: f32,
        render_spacing: f32, limit: usize) -> Result<Vec<(SurfaceHit,f32)>,String> {
        if !author_spacing.is_finite() || author_spacing <= 0. || !render_spacing.is_finite() || render_spacing <= 0. {
            return Err("模型植被候选格距无效".into());
        }
        let triangle = self.triangle(anchor).ok_or("模型植被锚点已失效，原数据已保留")?;
        let [a,b,c] = triangle.points;
        let edge = (b-a).length().max((c-a).length()).max((c-b).length());
        let n = (edge/author_spacing).ceil().max(1.);
        let area = (b-a).cross(c-a).length()/(n*(n+1.));
        let uv = Vec2::from_array(anchor.barycentric)*n-Vec2::splat(1./3.);
        let grid = uv.round();
        if (uv-grid).abs().max_element() > 0.002 || grid.min_element() < 0. {
            return Ok(vec![(self.resolve(anchor,Mat4::IDENTITY).ok_or("模型表面失效")?,area.min(render_spacing.powi(2)))]);
        }
        // 细分局部重心网格覆盖的同一个小区，候选始终保留原primitive和三角面身份。
        let refinement = (area.sqrt()/render_spacing).ceil().max(1.);
        if !refinement.is_finite() || refinement > (limit as f32).sqrt() {
            return Err("模型植被局部候选超过后台上传预算".into());
        }
        let refine = refinement as u32;
        let fine_n = n*refinement;
        let fine_area = (b-a).cross(c-a).length()/(fine_n*(fine_n+1.));
        let mut result = Vec::new();
        for v in 0..refine { for u in 0..refine {
            let barycentric = ((grid*refinement+Vec2::new(u as f32,v as f32)+Vec2::splat(1./3.))/fine_n).to_array();
            if barycentric[0]+barycentric[1] > 1. { continue; }
            if result.len() == limit { return Err("模型植被候选超过后台上传预算".into()); }
            let hit = self.resolve(ModelSurfaceAnchor {barycentric,..anchor},Mat4::IDENTITY).ok_or("模型表面失效")?;
            result.push((hit,fine_area));
        }}
        Ok(result)
    }
}
