//! 局部体积原型的拥有源、笔刷及固定最坏预算。
pub const VOLUME_CELLS: u32 = 64;
pub const MAX_VOLUME_VERTICES: usize = 65_536;
pub const MAX_VOLUME_TRIANGLES: usize = 131_072;
pub const MAX_VOLUME_GLB_BYTES: usize = 8 * 1024 * 1024;
pub const VOLUME_QUANTIZATION: f32 = 256.0; // 一个格距有256级，最小0.125m格约0.5mm密度精度。

#[derive(Clone, Debug, PartialEq)]
pub struct VolumeData {
    pub cells: u32,
    pub cell_size: f32,
    pub base_heights: Vec<f32>, // 局部XZ共享基础高度；外缘保持其真实三角面。
    pub density: Vec<i16>,      // X最快，随后Y、Z；正值为空气，负值为岩土。
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeBrushKind {
    Dig,
    Fill,
    Smooth,
    Flatten,
}

/// 规则形状按世界轴对齐；横向半径与竖直半高独立，避免挖宽通道时连带挖深。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VolumeBrushShape {
    #[default]
    Sphere,
    Box,
    Cylinder,
}

#[derive(Clone, Copy, Debug)]
pub struct VolumeBrush {
    pub kind: VolumeBrushKind,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub radius: f32,
    pub strength: f32,
    pub shape: VolumeBrushShape,
    /// 方盒/竖直圆柱的半高；球形继续只消费半径。
    pub half_height: f32,
    /// 整平的固定水平面，随笔划冻结；与from/to使用同一坐标空间。
    pub plane_height: f32,
    /// 作者点位于源坐标，但笔刷尺寸仍是世界米；线性变换保持缩放/旋转后的实际刷形。
    pub source_to_world: bevy::math::Mat3,
    /// 空间路径的局部截面；None保持旧世界轴笔刷及固定水平面语义。
    pub frame: Option<VolumeBrushFrame>,
}

/// 路径笔刷的真实SDF姿态与底平面，同一数据同时供预览和密度计算消费。
#[derive(Clone, Copy, Debug)]
pub struct VolumeBrushFrame {
    pub rotation: [f32; 4],
    pub half_extents: [f32; 3],
    pub plane_normal: [f32; 3],
    pub plane_distance: f32,
}

/// 一个批次引用完整笔划的全局刷印序号，分批边界不重复盖印、不重新计算间距。
#[derive(Clone, Debug)]
pub struct VolumeBrushBatch {
    pub(crate) brush: VolumeBrush,
    pub(crate) first: u32,
    pub(crate) last: u32,
    pub(crate) steps: u32,
    pub(crate) cell_size: f32,
    /// 路径只冻结当前内部窗口，不为整条路径分配逐刷印数组。
    pub(crate) stamps: Option<std::sync::Arc<[VolumeBrush]>>,
    pub(crate) cached_bounds: Option<([f32; 3], [f32; 3])>,
}

/// 固定大小的长笔划游标；只持端点和序号，累计长度不增加驻留内存。
#[derive(Clone, Copy, Debug)]
pub struct VolumeBrushBatches {
    pub(crate) brush: VolumeBrush,
    pub(crate) steps: u32,
    pub(crate) next: u32,
    pub(crate) cell_size: f32,
}

impl Default for VolumeBrush {
    fn default() -> Self {
        Self {
            kind: VolumeBrushKind::Dig,
            from: [0.; 3],
            to: [0.; 3],
            radius: 1.,
            strength: 1.,
            shape: VolumeBrushShape::Sphere,
            half_height: 1.,
            plane_height: 0.,
            source_to_world: bevy::math::Mat3::IDENTITY,
            frame: None,
        }
    }
}

impl VolumeData {
    pub fn side(&self) -> usize {
        self.cells as usize + 1
    }
    pub fn extent(&self) -> f32 {
        self.cells as f32 * self.cell_size
    }
    pub fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.side() + y) * self.side() + x
    }
    pub fn value(&self, x: usize, y: usize, z: usize) -> f32 {
        self.density[self.index(x, y, z)] as f32 * self.cell_size / VOLUME_QUANTIZATION
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(0.01..=0.5).contains(&self.cell_size) {
            return Err("三维作者格距必须在0.01至0.5米之间".into());
        }
        self.validate_lattice()
    }
    /// 作者精度与显示采样步长是两个合同；粗LOD仍使用同样的有界样本数，允许更大的有限格距。
    pub(super) fn validate_lattice(&self) -> Result<(), String> {
        if !(4..=VOLUME_CELLS).contains(&self.cells)
            || !self.cells.is_multiple_of(4)
            || !self.cell_size.is_finite()
            || self.cell_size <= 0.0
            || !self.extent().is_finite()
            || self.density.len() != self.side().pow(3)
            || self.base_heights.len() != self.side().pow(2)
            || self.base_heights.iter().any(|h| {
                !h.is_finite()
                    || *h < 2.0 * self.cell_size
                    || *h > self.extent() - 2.0 * self.cell_size
            })
        {
            return Err("三维雕刻源尺寸、基础高度或密度预算无效".into());
        }
        Ok(())
    }
    pub fn memory_bytes(&self) -> usize {
        self.density.capacity() * 2 + self.base_heights.capacity() * 4
    }
}

pub(super) fn quantize(value: f32, step: f32) -> i16 {
    (value / step * VOLUME_QUANTIZATION)
        .round()
        .clamp(-32767.0, 32767.0) as i16
}
