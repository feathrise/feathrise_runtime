//! 通用高度场边界约束；地形、CPU拾取与GPU草根共用固定布局，不依赖地图作者协议。
use bevy::{prelude::*, render::render_resource::ShaderType};

/// 插件启动时装配统一绑定与采样正文；测试也调用本入口，避免维护第二套GPU算法。
pub fn clipmap_stitch_shader_source(compute: bool) -> String {
    let mut enabled = vec![true];
    let mut output = String::new();
    for line in include_str!("../shaders/clipmap_stitches.wgsl").lines() {
        let trimmed = line.trim();
        if let Some(symbol) = trimmed.strip_prefix("#ifdef ") {
            enabled.push(*enabled.last().unwrap() && symbol == "CLIPMAP_STITCH_COMPUTE" && compute);
            continue;
        }
        if trimmed == "#else" {
            let old = enabled.pop().unwrap();
            enabled.push(*enabled.last().unwrap() && !old);
            continue;
        }
        if trimmed == "#endif" {
            enabled.pop();
            continue;
        }
        if !trimmed.starts_with('#') && *enabled.last().unwrap() {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct GpuBladeClipmapStitches {
    /// 西、东、南、北；有效采样数、源格距、混合宽度、共享LOD步幅。
    pub edges: [Vec4; 4],
    pub heights: [[Vec4; 65]; 4],
}
impl Default for GpuBladeClipmapStitches {
    fn default() -> Self {
        Self {
            edges: [Vec4::ZERO; 4],
            heights: [[Vec4::ZERO; 65]; 4],
        }
    }
}
impl GpuBladeClipmapStitches {
    pub fn valid(&self) -> bool {
        self.edges.iter().all(|info| {
            info.is_finite()
                && (info.x == 0.0 || {
                    let count = info.x as usize;
                    let stride = info.w as u32;
                    (2..=257).contains(&count)
                        && info.x == count as f32
                        && (count - 1).is_power_of_two()
                        && info.y > 0.0
                        && info.z > 0.0
                        && info.w == stride as f32
                        && stride.is_power_of_two()
                        && (stride as usize) < count
                })
        })
    }
    pub fn height_range(&self) -> Option<Vec2> {
        let mut range = Vec2::new(f32::INFINITY, f32::NEG_INFINITY);
        for (edge, info) in self.edges.iter().enumerate() {
            for i in 0..info.x as usize {
                let h = self.heights[edge][i / 4][i % 4];
                range.x = range.x.min(h);
                range.y = range.y.max(h);
            }
        }
        range.is_finite().then_some(range)
    }
    pub fn boundary_height(&self, p: Vec2, min: Vec2, max: Vec2) -> Option<f32> {
        for (e, info) in self.edges.iter().enumerate() {
            let n = if e < 2 { 0 } else { 1 };
            let bound = if e % 2 == 0 { min[n] } else { max[n] };
            if info.x >= 2.0 && (p[n] - bound).abs() <= 0.00001 {
                return Some(self.sample(e, p[1 - n] - min[1 - n]));
            }
        }
        None
    }
    pub fn revision(&self) -> u64 {
        if !self.active() {
            return 0;
        }
        let mut hash = self
            .edges
            .iter()
            .flat_map(|e| e.to_array())
            .fold(0x7263_7374_6974_6368_u64, |h, v| {
                h.rotate_left(9) ^ u64::from(v.to_bits())
            });
        // 高度曲线更新也必须换代草根投射；最多四边各257值，与地图规模无关。
        for (edge, info) in self.edges.iter().enumerate() {
            for i in 0..info.x as usize {
                hash = hash.rotate_left(9) ^ u64::from(self.heights[edge][i / 4][i % 4].to_bits());
            }
        }
        hash
    }
    /// 逆映射边界收敛前的有限候选格；四种X和四种Z组合，不随格距比例扩大搜索数组。
    pub fn candidate(
        &self,
        point: Vec2,
        min: Vec2,
        max: Vec2,
        step: Vec2,
        center: Vec2,
        index: usize,
    ) -> Vec2 {
        let axis = |a: usize, n: usize| {
            if n == 0 {
                return point[a];
            }
            let normal = 1 - a;
            for edge in if a == 0 { 2..4 } else { 0..2 } {
                let info = self.edges[edge];
                if info.x < 2.0 {
                    continue;
                }
                let bound = if edge % 2 == 0 {
                    min[normal]
                } else {
                    max[normal]
                };
                let grid = (bound - center[normal]) / step[normal];
                let inner = if edge % 2 == 0 {
                    grid.floor() + 1.0
                } else {
                    grid.ceil() - 1.0
                };
                let width = (center[normal] + inner * step[normal] - bound)
                    .abs()
                    .max(0.00001);
                let t = (point[normal] - bound).abs() / width;
                if t > 1.00001 {
                    continue;
                }
                let common = info.y * info.w.max(1.0);
                let lo = min[a] + ((point[a] - min[a]) / common).floor() * common;
                let hi = (lo + common).min(max[a]);
                return match n {
                    1 => (point[a] - (1.0 - t) * lo) / t.max(0.000001),
                    2 => (point[a] - (1.0 - t) * hi) / t.max(0.000001),
                    _ => (lo + hi) * 0.5,
                }
                .clamp(min[a], max[a]);
            }
            point[a]
        };
        Vec2::new(axis(0, index % 4), axis(1, index / 4))
    }
    pub fn active(&self) -> bool {
        self.edges.iter().any(|e| e.x >= 2.0)
    }
    pub fn sample(&self, edge: usize, distance: f32) -> f32 {
        let info = self.edges[edge];
        let stride = info.w.max(1.0) as usize;
        let last = info.x.max(1.0) as usize - 1;
        let at = (distance.max(0.0) / info.y).min(last as f32);
        let lo = (at as usize / stride * stride).min(last);
        let hi = (lo + stride).min(last);
        let t = if hi == lo {
            0.0
        } else {
            (at - lo as f32) / (hi - lo) as f32
        };
        self.heights[edge][lo / 4][lo % 4].lerp(self.heights[edge][hi / 4][hi % 4], t)
    }
    /// 先裁到真实矩形，再把边点收敛到双方共同的二次幂格点。细侧重复点只生成退化三角。
    pub fn position(&self, point: Vec2, min: Vec2, max: Vec2) -> Vec2 {
        if !self.active() {
            return point;
        }
        let mut p = point.clamp(min, max);
        for edge in 0..4 {
            let info = self.edges[edge];
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
            if (p[normal] - bound).abs() <= 0.00001 {
                let step = info.y * info.w.max(1.0);
                p[axis] = (min[axis] + ((p[axis] - min[axis]) / step + 0.5).floor() * step)
                    .clamp(min[axis], max[axis]);
            }
        }
        p
    }
    pub fn height(&self, point: Vec2, base: f32, min: Vec2, max: Vec2) -> f32 {
        let mut total = 0.0;
        let mut weighted = 0.0;
        let mut remain = 1.0;
        for edge in 0..4 {
            let info = self.edges[edge];
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
            let distance = (point[normal] - bound).abs();
            if distance >= info.z && distance > 0.00001 {
                continue;
            }
            let value = self.sample(edge, point[axis] - min[axis]);
            if distance <= 0.00001 {
                return value;
            }
            let t = (distance / info.z).clamp(0.0, 1.0);
            let weight = 1.0 - t * t * (3.0 - 2.0 * t);
            total += weight;
            weighted += weight * value;
            remain *= 1.0 - weight;
        }
        if total == 0.0 {
            base
        } else {
            base * remain + (1.0 - remain) * weighted / total
        }
    }
}
