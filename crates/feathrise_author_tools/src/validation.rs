//! 固定工具数量和语义数值校验；不访问资产、地图、实体或操作系统。

use crate::*;

// 空间线段笔刷原有10个固定盖印轮廓；16项预算保留完整原交互，不按地图规模扩张。
pub const MAX_AUTHOR_BRUSHES: usize = 16;
pub const MAX_AUTHOR_TRANSFORMS: usize = 8;
pub const MAX_AUTHOR_SELECTIONS: usize = 256;
pub const MAX_AUTHOR_TOOLS: usize =
    MAX_AUTHOR_BRUSHES + MAX_AUTHOR_TRANSFORMS + MAX_AUTHOR_SELECTIONS;
/// 每个笔刷/手柄最多256段，仿射选择框固定12段；所有消费者共用这个完整帧几何硬预算。
pub const MAX_AUTHOR_TOOL_SEGMENTS: usize =
    256 * (MAX_AUTHOR_BRUSHES + MAX_AUTHOR_TRANSFORMS) + 12 * MAX_AUTHOR_SELECTIONS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorToolValidationError {
    Limit(&'static str),
    Invalid(&'static str),
}

impl std::fmt::Display for AuthorToolValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit(message) | Self::Invalid(message) => formatter.write_str(message),
        }
    }
}
impl std::error::Error for AuthorToolValidationError {}

impl AuthorToolPreview {
    pub fn validate(&self) -> Result<(), AuthorToolValidationError> {
        use AuthorToolValidationError::{Invalid, Limit};
        if self.items.len() > MAX_AUTHOR_TOOLS {
            return Err(Limit("作者工具总数超过280项预算"));
        }
        let (mut brushes, mut transforms, mut selections) = (0, 0, 0);
        for item in &self.items {
            let valid = match item {
                AuthorTool::Brush(brush) => {
                    brushes += 1;
                    brush.valid()
                }
                AuthorTool::Transform(transform) => {
                    transforms += 1;
                    transform.valid()
                }
                AuthorTool::Selection(selection) => {
                    selections += 1;
                    selection.valid()
                }
            };
            if !valid {
                return Err(Invalid("作者工具包含无效数值、身份、颜色、尺寸或方向"));
            }
        }
        if brushes > MAX_AUTHOR_BRUSHES
            || transforms > MAX_AUTHOR_TRANSFORMS
            || selections > MAX_AUTHOR_SELECTIONS
        {
            return Err(Limit("作者工具超过16个笔刷、8个手柄或256个选择框预算"));
        }
        Ok(())
    }

    /// 返回生成端应预留的最坏段数；校验失败时调用者不能部分采用或截断工具列表。
    pub fn segment_budget(&self) -> Result<usize, AuthorToolValidationError> {
        self.validate()?;
        Ok(self
            .items
            .iter()
            .map(|item| match item {
                AuthorTool::Selection(_) => 12,
                _ => 256,
            })
            .sum())
    }
}

fn finite<const N: usize>(values: &[f32; N]) -> bool {
    values.iter().all(|value| value.is_finite())
}
fn size(value: f32) -> bool {
    value.is_finite() && value >= 0.
}
fn color(values: &[f32; 4]) -> bool {
    finite(values)
        && values[..3].iter().all(|value| *value >= 0.)
        && (0. ..=1.).contains(&values[3])
}
fn identity(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn unit<const N: usize>(values: &[f32; N]) -> bool {
    finite(values)
        && (values
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
            - 1.)
            .abs()
            <= 0.01
}

impl BrushPreview {
    fn valid(&self) -> bool {
        finite(&self.center)
            && unit(&self.rotation)
            && size(self.radius)
            && size(self.half_height)
            && self
                .strength
                .is_none_or(|value| value.is_finite() && (0. ..=1.).contains(&value))
            && color(&self.color)
            && self.clipmap.as_ref().is_none_or(ClipmapBrushSurface::valid)
    }
}

impl ClipmapBrushSurface {
    fn valid(&self) -> bool {
        identity(&self.map_id, 256)
            && self.terrain_id.as_ref().is_none_or(|id| identity(id, 256))
            && identity(&self.height_path, 4096)
        // 作者高度初始修订可以为0；接收端按稳定来源及作者时钟映射自己的实际高度票据。
    }
}

impl TransformPreview {
    fn valid(&self) -> bool {
        match self {
            Self::Move {
                origin,
                length,
                axes,
                active_axes,
                plane_color,
                color_active,
                axis_colors,
                anchor,
                marker_radius,
                ..
            } => {
                finite(origin)
                    && size(*length)
                    && *axes & !0b111 == 0
                    && *active_axes & !*axes == 0
                    && color(plane_color)
                    && color(color_active)
                    && axis_colors.iter().all(color)
                    && anchor.as_ref().is_none_or(finite)
                    && marker_radius.is_none_or(size)
            }
            Self::Rotate {
                origin,
                radius,
                yaw_degrees,
                before_yaw_degrees,
                color: rgba,
            } => {
                finite(origin)
                    && size(*radius)
                    && yaw_degrees.is_none_or(f32::is_finite)
                    && before_yaw_degrees.is_none_or(f32::is_finite)
                    && color(rgba)
            }
            Self::Scale {
                origin,
                right,
                up,
                half_size,
                color: rgba,
                ..
            } => {
                finite(origin)
                    && unit(right)
                    && unit(up)
                    && size(*half_size)
                    && color(rgba)
                    && right
                        .iter()
                        .zip(up)
                        .map(|(right, up)| f64::from(*right) * f64::from(*up))
                        .sum::<f64>()
                        .abs()
                        <= 0.01
            }
        }
    }
}

impl SelectionPreview {
    fn valid(&self) -> bool {
        identity(&self.target, 1024)
            && finite(&self.center)
            && self.half_axes.iter().all(finite)
            && color(&self.color)
            && (0..3).all(|axis| {
                f64::from(self.center[axis]).abs()
                    + self
                        .half_axes
                        .iter()
                        .map(|half| f64::from(half[axis]).abs())
                        .sum::<f64>()
                    <= f64::from(f32::MAX)
            })
    }
}
