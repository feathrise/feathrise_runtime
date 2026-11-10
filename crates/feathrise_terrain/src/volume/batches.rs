//! 长笔划使用全局等距刷印和有界局部窗口；总长度不再受单次计算预算限制。
use super::{VolumeBrush, VolumeBrushBatch, VolumeBrushBatches};
use bevy::math::Vec3;

impl VolumeBrushBatches {
    pub fn new(brush: VolumeBrush, cell_size: f32) -> Result<Self, String> {
        let from = Vec3::from_array(brush.from);
        let to = Vec3::from_array(brush.to);
        if !cell_size.is_finite()
            || !(0.01..=0.5).contains(&cell_size)
            || !from.is_finite()
            || !to.is_finite()
            || from.abs().max_element() > 100_000.
            || to.abs().max_element() > 100_000.
            || !brush.radius.is_finite()
            || !(brush.world_step(cell_size)..=8.).contains(&brush.radius)
            || !brush.shape_valid(cell_size, 8.)
            || !brush.strength.is_finite()
            || !(0.01..=1.).contains(&brush.strength)
        {
            return Err("三维笔划参数无效".into());
        }
        let steps = (from.distance(to) / brush.stamp_spacing(cell_size)).ceil() as u32;
        Ok(Self {
            brush,
            steps,
            next: 0,
            cell_size,
        })
    }

    /// 每批最多256个刷印、128个作用域块；八次二分只缩小内部窗口，不截短整条路径。
    pub fn next_batch(&mut self) -> Result<Option<VolumeBrushBatch>, String> {
        if self.finished() {
            return Ok(None);
        }
        let mut batch = VolumeBrushBatch {
            brush: self.brush,
            first: self.next,
            last: self.next,
            steps: self.steps,
            cell_size: self.cell_size,
            stamps: None,
            cached_bounds: None,
        };
        if batch.block_count() > 128 {
            return Err("单个笔刷邻域超过体积编辑窗口，请缩小笔刷尺寸".into());
        }
        let mut low = self.next;
        let mut high = self.next.saturating_add(255).min(self.steps);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            batch.last = middle;
            if batch.block_count() <= 128 {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        batch.last = low;
        self.next = low + 1;
        Ok(Some(batch))
    }

    pub fn finished(&self) -> bool {
        self.next > self.steps
    }
    pub fn completed_stamps(&self) -> u32 {
        self.next
    }
    pub fn total_stamps(&self) -> u32 {
        self.steps + 1
    }
}

impl VolumeBrushBatch {
    /// 普通鼠标段继续沿用原单段预算，确认式长笔划改用next_batch持续处理。
    pub fn single(brush: VolumeBrush, cell_size: f32) -> Result<Self, String> {
        let plan = VolumeBrushBatches::new(brush, cell_size)?;
        if plan.steps > 256 {
            return Err("三维笔刷本段过长，请连续移动而不是跳过大片地形".into());
        }
        Ok(Self {
            brush,
            first: 0,
            last: plan.steps,
            steps: plan.steps,
            cell_size,
            stamps: None,
            cached_bounds: None,
        })
    }

    fn center(&self, stamp: u32) -> Vec3 {
        Vec3::from_array(self.brush.from).lerp(
            Vec3::from_array(self.brush.to),
            stamp as f32 / self.steps.max(1) as f32,
        )
    }

    pub fn centers(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.brushes().map(|brush| Vec3::from_array(brush.from))
    }

    pub fn brushes(&self) -> impl Iterator<Item = VolumeBrush> + '_ {
        (self.first..=self.last).map(|stamp| {
            self.stamps.as_ref().map_or_else(
                || VolumeBrush {
                    from: self.center(stamp).to_array(),
                    to: self.center(stamp).to_array(),
                    ..self.brush
                },
                |stamps| stamps[stamp as usize],
            )
        })
    }

    /// IO、块预算及高度预取共用真实旋转包围，曲线中间不能只由首末端点推断。
    pub fn bounds(&self) -> (Vec3, Vec3) {
        if let Some((min, max)) = self.cached_bounds {
            return (Vec3::from_array(min), Vec3::from_array(max));
        }
        if self.stamps.is_none() {
            let from = self.center(self.first);
            let to = self.center(self.last);
            let half = self.brush.sample_half_extents(self.cell_size);
            return (from.min(to) - half, from.max(to) + half);
        }
        self.brushes().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), brush| {
                let center = Vec3::from_array(brush.from);
                let half = brush.sample_half_extents(self.cell_size);
                (min.min(center - half), max.max(center + half))
            },
        )
    }

    pub fn last_center(&self) -> Vec3 {
        self.stamps.as_ref().map_or_else(
            || self.center(self.last),
            |stamps| Vec3::from_array(stamps[self.last as usize].from),
        )
    }

    /// 读取与高度预取只使用本批首末刷印，雕刻仍按完整笔划的全局采样点执行。
    pub fn scope(&self) -> VolumeBrush {
        VolumeBrush {
            from: self.center(self.first).to_array(),
            to: self.center(self.last).to_array(),
            ..self.brush
        }
    }

    pub(crate) fn block_count(&self) -> i64 {
        let (min, max) = self.bounds();
        let extent = 32. * self.cell_size;
        let size = ((max + Vec3::splat(self.cell_size)) / extent)
            .floor()
            .as_ivec3()
            - ((min - Vec3::splat(self.cell_size)) / extent)
                .floor()
                .as_ivec3()
            + bevy::math::IVec3::ONE;
        i64::from(size.x) * i64::from(size.y) * i64::from(size.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::{VolumeBrushKind, VolumeBrushShape, sparse::*};
    use bevy::math::{Rect, Vec2};
    #[test]
    fn long_line_batches_cover_each_global_stamp_once_and_bound_diagonal_windows() {
        for to in [[800., -200., 700.], [-800., 200., -700.], [0.; 3]] {
            let brush = VolumeBrush {
                to,
                radius: 0.5,
                ..Default::default()
            };
            let mut plan = VolumeBrushBatches::new(brush, 0.25).unwrap();
            let steps = plan.steps;
            let mut count = 0;
            while let Some(batch) = plan.next_batch().unwrap() {
                assert!(batch.last - batch.first < 256);
                assert!(batch.block_count() <= 128);
                for center in batch.centers() {
                    assert_eq!(
                        center,
                        Vec3::ZERO.lerp(Vec3::from_array(to), count as f32 / steps.max(1) as f32)
                    );
                    count += 1;
                }
            }
            assert_eq!(count, steps + 1);
            assert_eq!(plan.completed_stamps(), plan.total_stamps());
        }
    }

    #[test]
    fn batch_boundaries_preserve_low_strength_flatten_and_smooth_density() {
        let bounds = Rect::from_corners(Vec2::splat(-64.), Vec2::splat(64.));
        for kind in [VolumeBrushKind::Flatten, VolumeBrushKind::Smooth] {
            let source = new_sparse_volume(0.25, std::sync::Arc::default()).unwrap();
            let seed = VolumeBrush {
                kind: VolumeBrushKind::Fill,
                from: [4., 0., 4.],
                to: [4., 0., 4.],
                radius: 1.,
                ..Default::default()
            };
            let source = apply_sparse_brush(&source, seed, bounds, |_| Ok(0.07)).unwrap();
            let brush = VolumeBrush {
                kind,
                from: [3., 0.5, 4.],
                to: [5., 0.5, 4.],
                radius: 1.,
                shape: VolumeBrushShape::Box,
                half_height: 1.,
                plane_height: 0.07,
                frame: None,
                strength: 0.3,
                ..Default::default()
            };
            let expected = apply_sparse_brush(&source, brush, bounds, |_| Ok(0.07)).unwrap();
            assert_ne!(
                VolumeSource::Sparse(source.clone()),
                VolumeSource::Sparse(expected.clone())
            );
            let plan = VolumeBrushBatches::new(brush, 0.25).unwrap();
            let mut actual = source;
            let mut first = 0;
            while first <= plan.steps {
                let last = (first + 2).min(plan.steps);
                actual = apply_sparse_brush_batch(
                    &actual,
                    VolumeBrushBatch {
                        brush,
                        first,
                        last,
                        steps: plan.steps,
                        cell_size: 0.25,
                        stamps: None,
                        cached_bounds: None,
                    },
                    bounds,
                    |_| Ok(0.07),
                )
                .unwrap();
                first = last + 1;
            }
            assert_eq!(
                VolumeSource::Sparse(actual),
                VolumeSource::Sparse(expected),
                "非幂等笔刷的批次交界不能重复盖印或重新取整步距"
            );
        }
    }
}
