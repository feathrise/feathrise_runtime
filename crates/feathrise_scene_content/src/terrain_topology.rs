//! 静态洞穴、倒扣和竖直壁的作者声明，以及所有高度面消费者共用的洞口范围。
//!
//! 三维拓扑仍由独立 GLB 提供。首版洞口使用世界轴矩形，局部尺寸随正缩放和
//! 四分之一圈旋转变换；任意朝向的无洞口悬崖不受此限制。避免粗 LOD 整格删除
//! 扩大洞口，物理在后台按矩形精确裁切，光栅按同一范围裁切片元。

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// 每个场景最多三十二个洞口；统一限制 GPU 常量和光追局部加密的最坏开销。
pub const MAX_TERRAIN_CUTOUTS: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScenePlacementTerrainPatch {
    /// 显式关联一块Clipmap；缺省仅兼容旧主地形，不能作用于同图其他地形。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_id: Option<crate::TerrainId>,
    /// 缺省表示只增加三维地形，不移除高度面；悬崖和倒扣通常使用此模式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutout: Option<SceneTerrainCutout>,
    /// 分块作者层合并后的多个局部覆盖矩形；同一层仍只占一个地形/光追根。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<SceneTerrainCutout>,
    /// F12局部体积原型自动生成的作者源；源密度与网格共同保存在对应GLB。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sculpted_volume: bool,
    /// v3版本载体只声明作者包围范围；真实裁切由附近完整驻留网格提供。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub streamed_volume: bool,
    /// 与v3版本根一致，驻留焦点按真实块长吸附；迁移旧源时保留原格距。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_cell_size: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneTerrainCutout {
    /// 相对 GLB 实例原点的局部 X/Z 下界，单位米；作者可把开口移离模型原点。
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl SceneTerrainCutout {
    pub fn contains(self, point: Vec2) -> bool {
        // 严格内部才是洞；边界仍属于地形，避免相邻模型的公共边出现不确定缝隙。
        point.cmpgt(Vec2::from_array(self.min)).all()
            && point.cmplt(Vec2::from_array(self.max)).all()
    }

    pub fn intersects(self, min: Vec2, max: Vec2) -> bool {
        Vec2::from_array(self.max).cmpgt(min).all() && Vec2::from_array(self.min).cmplt(max).all()
    }

    pub fn validate(self) -> Result<(), String> {
        let (min, max) = (Vec2::from_array(self.min), Vec2::from_array(self.max));
        if !min.is_finite() || !max.is_finite() || !min.cmplt(max).all() {
            return Err("三维地形洞口必须具有有限、正面积的 X/Z 范围".into());
        }
        Ok(())
    }
}

impl crate::ScenePlacementRecord {
    /// 同一稳定实例的模型、碰撞和洞口共同进入摆放历史，删除或撤销不留下孤立洞。
    pub fn terrain_cutout(&self, bounds: Rect) -> Result<Option<SceneTerrainCutout>, String> {
        let holes = self.terrain_cutouts(bounds)?;
        let Some(first) = holes.first().copied() else {
            return Ok(None);
        };
        // 旧作者索引/需求预取使用包围矩形；高度、物理和Shader必须使用完整覆盖列表。
        Ok(Some(holes.iter().skip(1).fold(first, |a, b| {
            SceneTerrainCutout {
                min: Vec2::from_array(a.min)
                    .min(Vec2::from_array(b.min))
                    .to_array(),
                max: Vec2::from_array(a.max)
                    .max(Vec2::from_array(b.max))
                    .to_array(),
            }
        })))
    }

    pub fn terrain_cutouts(&self, bounds: Rect) -> Result<Vec<SceneTerrainCutout>, String> {
        let Some(patch) = &self.terrain_patch else {
            return Ok(Vec::new());
        };
        if patch.cutouts.len() > MAX_TERRAIN_CUTOUTS
            || (patch.streamed_volume && !patch.sculpted_volume)
            || (patch.streamed_volume
                && patch
                    .volume_cell_size
                    .is_none_or(|v| !v.is_finite() || !(0.01..=0.5).contains(&v)))
            || (!patch.streamed_volume && patch.volume_cell_size.is_some())
            || (patch.cutout.is_some() && !patch.cutouts.is_empty())
            || (!patch.cutouts.is_empty() && !patch.sculpted_volume)
        {
            return Err("三维地形覆盖列表过大、混用了单矩形或缺少作者层身份".into());
        }
        if patch.sculpted_volume
            && (self.scale != [1.0; 3]
                || self.yaw_degrees != 0.0
                || (patch.cutout.is_none() && patch.cutouts.is_empty()))
        {
            return Err("三维雕刻原型须保留单位缩放、零旋转及自动替换区域".into());
        }
        if self.height_mode.follows_terrain()
            || self.building_preset_id.is_some()
            || self.courtyard.is_some()
            || !self.water_surfaces.is_empty()
        {
            return Err("三维地形必须使用固定高度，且不能同时作为建筑、院落或网格水".into());
        }
        let cutouts: Vec<_> = patch.cutout.iter().chain(&patch.cutouts).copied().collect();
        if cutouts.is_empty() {
            return Ok(Vec::new());
        }
        let quarter = (self.yaw_degrees / 90.0).round();
        if (self.yaw_degrees - quarter * 90.0).abs() > 0.001 {
            return Err(
                "带矩形洞口的三维地形旋转须为 90° 的整数倍；其他朝向请在 Blender 中应用旋转".into(),
            );
        }
        // 使用整数轴交换，避免 sin(90°) 的浮点残差使物理和片元边界不一致。
        let turn = (quarter as i32).rem_euclid(4);
        let scene_point = |point: [f32; 2]| {
            let x = point[0] * self.scale[0];
            let z = point[1] * self.scale[2];
            let point = match turn {
                0 => Vec2::new(x, z),
                1 => Vec2::new(z, -x),
                2 => Vec2::new(-x, -z),
                _ => Vec2::new(-z, x),
            };
            point + Vec2::new(self.translation[0], self.translation[2])
        };
        cutouts
            .into_iter()
            .map(|cutout| {
                cutout.validate()?;
                let (a, b) = (scene_point(cutout.min), scene_point(cutout.max));
                let result = SceneTerrainCutout {
                    min: a.min(b).to_array(),
                    max: a.max(b).to_array(),
                };
                result.validate()?;
                if Vec2::from_array(result.min).cmplt(bounds.min).any()
                    || Vec2::from_array(result.max).cmpgt(bounds.max).any()
                {
                    return Err("三维地形洞口超出当前场景范围".into());
                }
                Ok(result)
            })
            .collect()
    }
}

/// 从一个三角面中扣除多个矩形，保留原平面和绕序；仅供后台物理构建。
pub fn subtract_terrain_cutouts(triangle: [Vec3; 3], holes: &[SceneTerrainCutout]) -> Vec<[Vec3; 3]> {
    let mut polygons = vec![triangle.to_vec()];
    for hole in holes {
        let mut remaining = Vec::new();
        for polygon in polygons {
            let mut min = Vec2::splat(f32::INFINITY);
            let mut max = Vec2::splat(f32::NEG_INFINITY);
            for p in &polygon {
                min = min.min(p.xz());
                max = max.max(p.xz());
            }
            if !hole.intersects(min, max) {
                remaining.push(polygon);
                continue;
            }
            // 依次剥离四个外侧半平面，剩余内部最后丢弃；外侧多边形互不重叠。
            let mut inside = polygon;
            for (axis, edge, sign) in [
                (0, hole.min[0], 1.0),
                (0, hole.max[0], -1.0),
                (2, hole.min[1], 1.0),
                (2, hole.max[1], -1.0),
            ] {
                let (inner, outer) = split_polygon(&inside, axis, edge, sign);
                if outer.len() >= 3 {
                    remaining.push(outer);
                }
                inside = inner;
                if inside.len() < 3 {
                    break;
                }
            }
        }
        polygons = remaining;
    }
    let mut result = Vec::new();
    for polygon in polygons {
        for i in 1..polygon.len().saturating_sub(1) {
            let t = [polygon[0], polygon[i], polygon[i + 1]];
            if (t[1] - t[0]).cross(t[2] - t[0]).length_squared() > 1.0e-12 {
                result.push(t);
            }
        }
    }
    result
}

fn split_polygon(input: &[Vec3], axis: usize, edge: f32, sign: f32) -> (Vec<Vec3>, Vec<Vec3>) {
    let (mut inside, mut outside) = (Vec::new(), Vec::new());
    if input.is_empty() {
        return (inside, outside);
    }
    let mut previous = input[input.len() - 1];
    let mut d0 = (previous[axis] - edge) * sign;
    for &current in input {
        let d1 = (current[axis] - edge) * sign;
        if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
            let mut p = previous.lerp(current, d0 / (d0 - d1));
            p[axis] = edge; // 共边采用权威矩形坐标，避免两个邻接三角面的插值误差形成裂缝。
            inside.push(p);
            outside.push(p);
        }
        if d1 >= 0.0 {
            inside.push(current);
        }
        if d1 <= 0.0 {
            outside.push(current);
        }
        previous = current;
        d0 = d1;
    }
    (inside, outside)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_patch_roundtrip_and_scaled_quarter_turn_share_exact_world_rect() {
        let mut record:crate::ScenePlacementRecord=serde_json::from_str(r#"{"id":"cave","asset_path":"scenes/cave.glb","translation":[10,2,20],"yaw_degrees":90,"scale":[2,3,4],"terrain_patch":{"cutout":{"min":[-1,-2],"max":[3,4]}}}"#).unwrap();
        let bounds = Rect::from_corners(Vec2::splat(-100.0), Vec2::splat(100.0));
        let hole = record.terrain_cutout(bounds).unwrap().unwrap();
        assert_eq!(
            hole,
            SceneTerrainCutout {
                min: [2.0, 14.0],
                max: [26.0, 22.0]
            }
        );
        let json = serde_json::to_string(&record).unwrap();
        let restored: crate::ScenePlacementRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.terrain_patch, record.terrain_patch);
        record.yaw_degrees = 45.0;
        assert!(record.terrain_cutout(bounds).is_err());
        record.terrain_patch.as_mut().unwrap().cutout = None;
        assert_eq!(record.terrain_cutout(bounds).unwrap(), None); // 无洞口悬崖允许任意朝向。
        record.terrain_patch = None;
        assert!(
            !serde_json::to_string(&record)
                .unwrap()
                .contains("terrain_patch")
        ); // 历史数据不增加空字段。
    }
    #[test]
    fn cutout_preserves_area_plane_and_winding() {
        let holes = [SceneTerrainCutout {
            min: [-1.0, -1.0],
            max: [1.0, 1.0],
        }];
        let v = [
            Vec3::new(-2.0, -2.0, -2.0),
            Vec3::new(-2.0, -2.0, 2.0),
            Vec3::new(2.0, 2.0, 2.0),
            Vec3::new(2.0, 2.0, -2.0),
        ];
        let triangles = [[v[0], v[1], v[2]], [v[0], v[2], v[3]]];
        let out: Vec<_> = triangles
            .into_iter()
            .flat_map(|t| subtract_terrain_cutouts(t, &holes))
            .collect();
        let area: f32 = out
            .iter()
            .map(|t| ((t[1] - t[0]).cross(t[2] - t[0])).y * 0.5)
            .sum();
        assert!((area - 12.0).abs() < 0.0001);
        for t in out {
            assert!((t[1] - t[0]).cross(t[2] - t[0]).y > 0.0);
            for p in t {
                assert!((p.y - p.x).abs() < 0.0001);
                assert!(!holes[0].contains(p.xz()));
            }
        }
    }
    #[test]
    fn full_cutout_produces_no_fake_ground() {
        let t = [Vec3::ZERO, Vec3::Z, Vec3::X];
        assert!(
            subtract_terrain_cutouts(
                t,
                &[SceneTerrainCutout {
                    min: [-1.0, -1.0],
                    max: [2.0, 2.0]
                }]
            )
            .is_empty()
        );
        assert_eq!(subtract_terrain_cutouts(t, &[]), vec![t]);
    }
    #[test]
    fn volume_multiple_cutouts_preserve_disconnected_middle_and_validate_the_full_list() {
        let mut record: crate::ScenePlacementRecord = serde_json::from_value(serde_json::json!({
            "id":"volume-layer","asset_path":"scenes/volume.glb","translation":[10,0,20],"yaw_degrees":0,"scale":[1,1,1],
            "terrain_patch":{"sculpted_volume":true,"cutouts":[{"min":[0,0],"max":[8,8]},{"min":[32,0],"max":[40,8]}]}
        })).unwrap();
        let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(100.));
        let holes = record.terrain_cutouts(bounds).unwrap();
        assert_eq!(holes.len(), 2);
        assert!(holes.iter().any(|h| h.contains(Vec2::new(14., 24.))));
        assert!(!holes.iter().any(|h| h.contains(Vec2::new(30., 24.))));
        assert!(
            record
                .terrain_cutout(bounds)
                .unwrap()
                .unwrap()
                .contains(Vec2::new(30., 24.)),
            "旧包围盒仅用于作者索引，不得用于裁切"
        );
        let json = serde_json::to_vec(&record).unwrap();
        assert_eq!(record, serde_json::from_slice(&json).unwrap());
        record.terrain_patch.as_mut().unwrap().cutout = Some(holes[0]);
        assert!(record.terrain_cutouts(bounds).is_err());
        record.terrain_patch.as_mut().unwrap().cutout = None;
        record.terrain_patch.as_mut().unwrap().sculpted_volume = false;
        assert!(record.terrain_cutouts(bounds).is_err());
    }
}
