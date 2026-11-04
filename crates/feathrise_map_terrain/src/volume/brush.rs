//! 后台密度布尔编辑与共享样本平滑；两格外缘不编辑，避免破坏高度接缝。
use super::types::*;
use bevy::math::{Mat3, Mat4, Quat, Vec3, Vec3Swizzles};

const PROTECTED_CELLS: u32 = 2;

impl VolumeBrush {
    /// 点与整平面进入作者空间，尺寸留在世界空间；平移区域原点时不再重复转换尺寸。
    pub fn in_source_frame(mut self, source_to_world: Mat4) -> Self {
        let inverse = source_to_world.inverse();
        self.from = inverse.transform_point3(Vec3::from_array(self.from)).to_array();
        self.to = inverse.transform_point3(Vec3::from_array(self.to)).to_array();
        self.plane_height = inverse.transform_point3(Vec3::new(0., self.plane_height, 0.)).y;
        if let Some(frame) = self.frame.as_mut() {
            // 路径底平面保留世界法线，源原点平移只改变该法线上的距离。
            frame.plane_distance -= Vec3::from_array(frame.plane_normal).dot(source_to_world.w_axis.truncate());
        }
        self.source_to_world = Mat3::from_mat4(source_to_world);
        self
    }

    fn maximum_scale(self) -> f32 {
        self.source_to_world.x_axis.length().max(self.source_to_world.y_axis.length())
            .max(self.source_to_world.z_axis.length())
    }

    pub fn world_step(self, step: f32) -> f32 {
        step * self.source_to_world.x_axis.length().min(self.source_to_world.y_axis.length())
            .min(self.source_to_world.z_axis.length())
    }

    /// IO预取、块预算与密度盖印共用形状包围范围，薄圆柱不能仍按球体漏读竖直块。
    pub fn sample_half_extents(self, step: f32) -> Vec3 {
        let y = if self.shape == VolumeBrushShape::Sphere {
            self.radius
        } else {
            self.half_height
        };
        let inverse = self.source_to_world.inverse();
        let inverse_abs = Mat3::from_cols(inverse.x_axis.abs(), inverse.y_axis.abs(), inverse.z_axis.abs());
        let half = if let Some(frame) = self.frame {
            let rotation = Quat::from_array(frame.rotation);
            let h = Vec3::from_array(frame.half_extents);
            (rotation * Vec3::X).abs() * h.x
                + (rotation * Vec3::Y).abs() * h.y
                + (rotation * Vec3::Z).abs() * h.z
        } else {
            Vec3::new(self.radius, y, self.radius)
        };
        inverse_abs * half + Vec3::splat(if self.kind == VolumeBrushKind::Smooth {
            0.0
        } else {
            3.0_f32.sqrt() * step
        })
    }

    /// 作者落点转换为区域局部坐标时，整平基准必须同时平移，不能悄悄回到世界零高度。
    pub fn translated(mut self, offset: Vec3) -> Self {
        self.from = (Vec3::from_array(self.from) + offset).to_array();
        self.to = (Vec3::from_array(self.to) + offset).to_array();
        self.plane_height += offset.y;
        if let Some(frame) = self.frame.as_mut() {
            frame.plane_distance += Vec3::from_array(frame.plane_normal).dot(self.source_to_world * offset);
        }
        self
    }

    pub(crate) fn shape_valid(self, step: f32, maximum: f32) -> bool {
        if !self.source_to_world.is_finite() || self.source_to_world.determinant().abs() <= 1e-8 {
            return false;
        }
        if let Some(frame) = self.frame {
            let rotation = Quat::from_array(frame.rotation);
            let half = Vec3::from_array(frame.half_extents);
            let normal = Vec3::from_array(frame.plane_normal);
            return rotation.is_finite()
                && (rotation.length_squared() - 1.).abs() < 0.001
                && half.is_finite()
                && half.min_element() >= self.world_step(step) * 0.25
                && half.max_element() <= maximum
                && normal.is_finite()
                && (normal.length_squared() - 1.).abs() < 0.001
                && frame.plane_distance.is_finite();
        }
        (self.shape == VolumeBrushShape::Sphere
            || self.half_height.is_finite() && (self.world_step(step)..=maximum).contains(&self.half_height))
            && (self.kind != VolumeBrushKind::Flatten || self.plane_height.is_finite())
    }

    pub(crate) fn stamp_spacing(self, step: f32) -> f32 {
        let size = if self.shape == VolumeBrushShape::Sphere {
            self.radius
        } else {
            self.radius.min(self.half_height)
        };
        step.min(size / (4. * self.maximum_scale()))
    }

    /// 真正的形状SDF保持方盒的平直面与圆柱的平底，不能仅改变光标或球形筛选范围。
    pub(crate) fn signed_distance(self, relative: Vec3) -> f32 {
        let (relative, half) = self.local_geometry(relative);
        let world_distance = match self.shape {
            VolumeBrushShape::Sphere => relative.length() - self.radius,
            VolumeBrushShape::Box => {
                let q = relative.abs() - half;
                q.max(Vec3::ZERO).length() + q.max_element().min(0.)
            }
            VolumeBrushShape::Cylinder => {
                let radial = if self.frame.is_some() {
                    ((relative.xz() / half.xz()).length() - 1.) * half.x.min(half.z)
                } else {
                    relative.xz().length() - self.radius
                };
                let q = bevy::math::Vec2::new(radial, relative.y.abs() - half.y);
                q.max(bevy::math::Vec2::ZERO).length() + q.max_element().min(0.)
            }
        };
        // 零等值面保持精确世界刷形，除以最大缩放得到保守的源距离，避免布尔过冲。
        world_distance / self.maximum_scale()
    }

    pub(crate) fn accepts(self, sdf: f32, step: f32) -> bool {
        sdf <= if matches!(self.kind, VolumeBrushKind::Dig | VolumeBrushKind::Fill) {
            3.0_f32.sqrt() * step
        } else {
            0.
        }
    }

    pub(crate) fn weight(self, relative: Vec3, step: f32) -> f32 {
        let falloff = match self.kind {
            VolumeBrushKind::Smooth => {
                let (relative, half) = self.local_geometry(relative);
                let normalized = match self.shape {
                    VolumeBrushShape::Sphere => relative.length() / self.radius,
                    VolumeBrushShape::Box => (relative.abs() / half).max_element(),
                    VolumeBrushShape::Cylinder => (if self.frame.is_some() {
                        (relative.xz() / half.xz()).length()
                    } else {
                        relative.xz().length() / self.radius
                    })
                    .max(relative.y.abs() / half.y),
                };
                (1. - normalized).max(0.)
            }
            // 两格边缘连接旧密度；内部完全朝固定平面收敛，避免硬截断制造额外悬空壳面。
            VolumeBrushKind::Flatten => {
                (-self.signed_distance(relative) / (2. * step)).clamp(0., 1.)
            }
            _ => 1.,
        };
        self.strength * falloff
    }

    /// 高度附着体积与独立体素共享同一密度编辑语义；邻域均值由拥有者从冻结源提供。
    pub fn edit_density_sample(self, current: i16, neighbor_average: f32, position: Vec3, center: Vec3, step: f32) -> Option<i16> {
        let relative = position - center;
        let sdf = self.signed_distance(relative);
        if !self.accepts(sdf, step) { return None; }
        let value = current as f32 * step / VOLUME_QUANTIZATION;
        let desired = match self.kind {
            VolumeBrushKind::Dig => value.max(-sdf),
            VolumeBrushKind::Fill => value.min(sdf),
            VolumeBrushKind::Flatten => self.flatten_distance(position),
            VolumeBrushKind::Smooth => neighbor_average * step / VOLUME_QUANTIZATION,
        };
        Some(quantize(value + (desired - value) * self.weight(relative, step), step))
    }

    fn local_geometry(self, relative: Vec3) -> (Vec3, Vec3) {
        let relative = self.source_to_world * relative; // 先恢复世界刷形，再进入路径截面。
        self.frame.map_or(
            (
                relative,
                Vec3::new(self.radius, self.half_height, self.radius),
            ),
            |frame| {
                (
                    Quat::from_array(frame.rotation).conjugate() * relative,
                    Vec3::from_array(frame.half_extents),
                )
            },
        )
    }

    /// 路径整平读取真正倾斜的底平面；普通笔刷继续使用原冻结世界Y。
    pub(crate) fn flatten_distance(self, position: Vec3) -> f32 {
        self.frame.map_or(position.y - self.plane_height, |frame| {
            (Vec3::from_array(frame.plane_normal).dot(self.source_to_world * position) - frame.plane_distance) / self.maximum_scale()
        })
    }

    /// 笔刷与保护带相交只裁剪写入范围；供作者端区分边界无改动与普通重复盖印。
    pub fn is_clipped_by_boundary(self, source: &VolumeData) -> bool {
        let margin = PROTECTED_CELLS as f32 * source.cell_size;
        let domain = self.sample_half_extents(source.cell_size);
        [self.from, self.to].iter().any(|p| {
            let p = Vec3::from_array(*p);
            (p - domain).min_element() <= margin
                || (p + domain).max_element() >= source.extent() - margin
        })
    }
}

pub fn volume_from_heights(cells: u32, step: f32, heights: Vec<f32>) -> Result<VolumeData, String> {
    let mut data = VolumeData {
        cells,
        cell_size: step,
        base_heights: heights,
        density: Vec::new(),
    };
    if !(4..=VOLUME_CELLS).contains(&cells)
        || !cells.is_multiple_of(4)
        || !step.is_finite()
        || !(0.01..=0.5).contains(&step)
        || data.base_heights.len() != data.side().pow(2)
    {
        return Err("三维区域规格无效".into());
    }
    data.density = vec![0; data.side().pow(3)];
    for z in 0..data.side() {
        for y in 0..data.side() {
            for x in 0..data.side() {
                let at = data.index(x, y, z);
                data.density[at] = quantize(
                    y as f32 * step - data.base_heights[z * data.side() + x],
                    step,
                );
            }
        }
    }
    data.validate()?;
    Ok(data)
}

/// 一段拖动在后台连续盖印；整段全部成功才返回新源，绝不写调用方快照。
pub fn apply_volume_brush(source: &VolumeData, brush: VolumeBrush) -> Result<VolumeData, String> {
    source.validate()?;
    let (from, to) = (Vec3::from_array(brush.from), Vec3::from_array(brush.to));
    if source.cells <= 2 * PROTECTED_CELLS
        || !brush.radius.is_finite()
        || brush.radius < brush.world_step(source.cell_size)
        || brush.radius > source.extent() * brush.maximum_scale() / 4.0
        || !brush.shape_valid(source.cell_size, source.extent() * brush.maximum_scale() / 4.0)
        || !brush.strength.is_finite()
        || !(0.01..=1.0).contains(&brush.strength)
    {
        return Err("三维笔刷参数或区域尺寸超出原型预算".into());
    }
    // 真正越出区域的中心仍失败关闭；球体/SDF邻域触边不能提前拒绝整笔或中断连续挖掘。
    if [from, to]
        .iter()
        .any(|p| !p.is_finite() || p.min_element() < 0.0 || p.max_element() > source.extent())
    {
        return Err("三维笔刷落点已越出本块区域；当前原型只能编辑有限区域内的地形".into());
    }
    let steps = (from.distance(to) / brush.stamp_spacing(source.cell_size)).ceil() as usize;
    if steps > 256 {
        return Err("三维笔刷单段超过256个连续刷印预算".into());
    }
    let mut next = source.clone(); // 唯一后台写侧；主线程和历史只持Arc。
    for i in 0..=steps {
        let p = from.lerp(to, i as f32 / steps.max(1) as f32);
        stamp(&mut next, p, brush);
    }
    Ok(next)
}

fn stamp(data: &mut VolumeData, center: Vec3, brush: VolumeBrush) {
    let step = data.cell_size;
    // 布尔SDF需要覆盖所有穿过球边界的单元角点。只改球内会把球外旧高度距离当成球的距离，压缩洞顶/洞底。
    let domain = brush.sample_half_extents(step);
    // 保护两格单元及其边界样本，六个方向统一裁剪；靠近底部/洞壁也继续编辑剩余内部。
    // 先限制格点再转无符号索引，避免球体越界导致负数转换、数组越界或平滑邻居下溢。
    let min = ((center - domain) / step)
        .floor()
        .max(Vec3::splat((PROTECTED_CELLS + 1) as f32))
        .as_uvec3();
    let max = ((center + domain) / step)
        .ceil()
        .min(Vec3::splat((data.cells - PROTECTED_CELLS - 1) as f32))
        .as_uvec3();
    let old = (brush.kind == VolumeBrushKind::Smooth).then(|| data.density.clone()); // 平滑同一刷印读取同一不可变邻域。
    for z in min.z as usize..=max.z as usize {
        for y in min.y as usize..=max.y as usize {
            for x in min.x as usize..=max.x as usize {
                let position = Vec3::new(x as f32, y as f32, z as f32) * step;
                let at = data.index(x, y, z);
                let average = old.as_ref().map_or(data.density[at] as f32, |old| {
                    [(x - 1, y, z), (x + 1, y, z), (x, y - 1, z), (x, y + 1, z),
                        (x, y, z - 1), (x, y, z + 1)].iter()
                        .map(|&(x, y, z)| old[data.index(x, y, z)] as f32).sum::<f32>() / 6.0
                });
                if let Some(value) = brush.edit_density_sample(data.density[at], average, position, center, step) {
                    data.density[at] = value;
                }
            }
        }
    }
}
