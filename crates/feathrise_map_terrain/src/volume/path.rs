//! 多点折线/曲线按同一弧长序号雕刻，截面沿切向旋转；重计算仅由宿主后台调用。
mod geometry;
mod types;
use super::{VolumeBrush, VolumeBrushBatch, VolumeBrushFrame, VolumeBrushKind, VolumeBrushShape};
use bevy::math::{Mat3, Mat4, Quat, Rect, Vec3, Vec3Swizzles};
use std::sync::Arc;
pub use types::{VolumePathBatches, VolumePathParameters, VolumePathPlan, VolumePathSection};

impl VolumePathPlan {
    pub fn new(
        nodes: &[[f32; 3]],
        parameters: VolumePathParameters,
        cell_size: f32,
        bounds: Rect,
    ) -> Result<Self, String> {
        Self::new_in_source_frame(nodes, parameters, cell_size, bounds, Mat4::IDENTITY)
    }

    /// 节点和预览保持世界坐标，边界与密度刷印进入选中网格的作者源；不使用地图容器范围。
    pub fn new_in_source_frame(
        nodes: &[[f32; 3]],
        parameters: VolumePathParameters,
        cell_size: f32,
        bounds: Rect,
        source_to_world: Mat4,
    ) -> Result<Self, String> {
        if !source_to_world.is_finite()
            || source_to_world.determinant().abs() <= 1e-8
            || !source_to_world.inverse().is_finite()
        {
            return Err("路径地形对象姿态无效".into());
        }
        if !cell_size.is_finite()
            || !(0.01..=0.5).contains(&cell_size)
            || !parameters.width.is_finite()
            || !parameters.height.is_finite()
            || !(0.5..=16.).contains(&parameters.width)
            || !(0.5..=16.).contains(&parameters.height)
            || !bounds.min.is_finite()
            || !bounds.max.is_finite()
        {
            return Err("路径截面或地形规格无效".into());
        }
        let world_step = VolumeBrush {
            source_to_world: Mat3::from_mat4(source_to_world),
            ..Default::default()
        }.world_step(cell_size);
        let points = geometry::expand(nodes, parameters.curved, world_step * 0.125)?;
        let mut lengths = Vec::new();
        lengths
            .try_reserve_exact(points.len())
            .map_err(|e| format!("路径弧长表分配失败：{e}"))?;
        lengths.push(0.);
        for pair in points.windows(2) {
            let length = *lengths.last().unwrap() + pair[0].as_dvec3().distance(pair[1].as_dvec3());
            if !length.is_finite() {
                return Err("路径长度无法表示".into());
            }
            lengths.push(length);
        }
        let mut node_lengths = Vec::new();
        node_lengths
            .try_reserve_exact(nodes.len())
            .map_err(|e| format!("路径节点索引分配失败：{e}"))?;
        let mut cursor = 0;
        for node in nodes {
            while cursor + 1 < points.len()
                && points[cursor].distance_squared(Vec3::from_array(*node)) > 0.0000001
            {
                cursor += 1;
            }
            node_lengths.push(lengths[cursor]);
        }
        let plan = Self {
            points: points.into(),
            lengths: lengths.into(),
            node_lengths: node_lengths.into(),
            parameters,
            world_step,
            source_to_world,
            source_bounds: bounds,
        };
        // 完整曲线和旋转截面都在后台检查；不能只检查控制点并把曲线越界部分静默裁掉。
        for distance in plan.lengths.iter().copied() {
            let brush = plan.brush_at(distance, VolumeBrushKind::Dig, 1., world_step)
                .in_source_frame(source_to_world);
            let center = Vec3::from_array(brush.from);
            let half = brush.sample_half_extents(cell_size) + Vec3::splat(cell_size);
            if (center.xz() - half.xz()).cmplt(bounds.min).any()
                || (center.xz() + half.xz()).cmpgt(bounds.max).any()
            {
                return Err("路径旋转截面越出地图边界".into());
            }
        }
        Ok(plan)
    }

    /// 固定大小的身份核对，来源移动或改尺寸后必须重新准备完整路径。
    pub fn matches_source(&self, bounds: Rect, source_to_world: Mat4) -> bool {
        self.source_bounds == bounds && self.source_to_world == source_to_world
    }

    pub fn brush_at(
        &self,
        distance: f64,
        kind: VolumeBrushKind,
        strength: f32,
        step: f32,
    ) -> VolumeBrush {
        let distance = distance.clamp(0., self.length());
        let index = self
            .lengths
            .partition_point(|length| *length <= distance)
            .saturating_sub(1)
            .min(self.points.len() - 2);
        let length = self.lengths[index + 1] - self.lengths[index];
        let floor = self.points[index].lerp(
            self.points[index + 1],
            ((distance - self.lengths[index]) / length) as f32,
        );
        let forward = (self.points[index + 1] - self.points[index]).normalize();
        let right = if forward.y.abs() < 0.999 {
            Vec3::Y.cross(forward).normalize()
        } else {
            Vec3::Z.cross(forward).normalize()
        };
        let up = forward.cross(right).normalize();
        let (shape, rotation, half) = match self.parameters.section {
            VolumePathSection::Rectangle => (
                VolumeBrushShape::Box,
                Mat3::from_cols(right, up, forward),
                Vec3::new(
                    self.parameters.width * 0.5,
                    self.parameters.height * 0.5,
                    step,
                ),
            ),
            // 圆柱的局部Y沿路径，椭圆截面使用独立宽高，局部Z向下仍保持右手坐标系。
            VolumePathSection::Round => (
                VolumeBrushShape::Cylinder,
                Mat3::from_cols(right, forward, -up),
                Vec3::new(
                    self.parameters.width * 0.5,
                    step,
                    self.parameters.height * 0.5,
                ),
            ),
        };
        let center = floor + up * (self.parameters.height * 0.5);
        VolumeBrush {
            kind,
            from: center.to_array(),
            to: center.to_array(),
            radius: (self.parameters.width.max(self.parameters.height) * 0.5).max(step),
            strength,
            shape,
            half_height: step,
            plane_height: floor.y,
            frame: Some(VolumeBrushFrame {
                rotation: Quat::from_mat3(&rotation).to_array(),
                half_extents: half.to_array(),
                plane_normal: up.to_array(),
                plane_distance: up.dot(floor),
            }),
            ..Default::default() // 世界预览保持单位姿态，实际刷印统一在批次中转换。
        }
    }

    /// 预览和雕刻读取同一姿态算法；全程覆盖首尾，预览采样量不随作者节点数量增长。
    pub fn preview(&self, count: usize) -> Vec<VolumeBrush> {
        (0..count)
            .map(|i| {
                self.brush_at(
                    self.length() * i as f64 / count.saturating_sub(1).max(1) as f64,
                    VolumeBrushKind::Dig,
                    1.,
                    self.world_step,
                )
            })
            .collect()
    }

    /// 节点索引在后台冻结；前台所选截面不能借用整条路径终点的朝向。
    pub fn node_brush(&self, index: usize) -> Option<VolumeBrush> {
        self.node_lengths
            .get(index)
            .map(|distance| self.brush_at(*distance, VolumeBrushKind::Dig, 1., self.world_step))
    }

    pub fn batches(
        self: &Arc<Self>,
        kind: VolumeBrushKind,
        strength: f32,
        cell_size: f32,
        offset: Vec3,
    ) -> Result<VolumePathBatches, String> {
        if !strength.is_finite()
            || !(0.01..=1.).contains(&strength)
            || !offset.is_finite()
            || !cell_size.is_finite()
            || !(0.01..=0.5).contains(&cell_size)
        {
            return Err("路径雕刻参数无效".into());
        }
        // 弧长是世界单位，作者源缩小时必须同步缩短刷印间距，保证相邻截面仍连续。
        let world_step = VolumeBrush {
            source_to_world: Mat3::from_mat4(self.source_to_world),
            ..Default::default()
        }.world_step(cell_size);
        let spacing =
            (world_step * 0.5).min(self.parameters.width.min(self.parameters.height) * 0.125);
        let steps = (self.length() / f64::from(spacing)).ceil();
        if !steps.is_finite() || steps >= u64::MAX as f64 {
            return Err("路径刷印序号无法由当前平台表示".into());
        }
        Ok(VolumePathBatches {
            plan: self.clone(),
            kind,
            strength,
            offset,
            cell_size,
            world_step,
            steps: (steps as u64).max(1),
            next: 0,
        })
    }
}

impl VolumePathBatches {
    /// 此方法由后台调用；一批只冻结至多256个真实姿态，转弯和斜向包围超过窗口时自动提前结束。
    pub fn next_batch(&mut self) -> Result<Option<VolumeBrushBatch>, String> {
        if self.finished() {
            return Ok(None);
        }
        let mut stamps = Vec::new();
        stamps
            .try_reserve_exact(256)
            .map_err(|e| format!("路径局部刷印分配失败：{e}"))?;
        let mut bounds = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        while self.next <= self.steps && stamps.len() < 256 {
            let brush = self.stamp(self.next);
            let center = Vec3::from_array(brush.from);
            let half = brush.sample_half_extents(self.cell_size) + Vec3::splat(self.cell_size);
            let next = (bounds.0.min(center - half), bounds.1.max(center + half));
            let blocks = (next.1 / (self.cell_size * 32.)).floor().as_ivec3()
                - (next.0 / (self.cell_size * 32.)).floor().as_ivec3()
                + bevy::math::IVec3::ONE;
            if i64::from(blocks.x) * i64::from(blocks.y) * i64::from(blocks.z) > 128 {
                if stamps.is_empty() {
                    return Err("单个路径截面超过当前局部编辑窗口".into());
                }
                break;
            }
            bounds = next;
            stamps.push(brush);
            self.next += 1;
        }
        Ok(Some(VolumeBrushBatch {
            brush: stamps[0],
            first: 0,
            last: stamps.len() as u32 - 1,
            steps: stamps.len().saturating_sub(1) as u32,
            cell_size: self.cell_size,
            stamps: Some(stamps.into()),
            cached_bounds: Some((
                (bounds.0 + Vec3::splat(self.cell_size)).to_array(),
                (bounds.1 - Vec3::splat(self.cell_size)).to_array(),
            )),
        }))
    }
}

#[cfg(test)]
mod tests;
