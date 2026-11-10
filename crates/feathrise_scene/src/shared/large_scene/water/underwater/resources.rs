//! 本帧唯一水下视图及最深有效覆盖；水材质既有遍历只额外做常数次足迹采样。
use bevy::prelude::*;
use super::types::WaterViewSample;

#[derive(Resource, Default)]
pub(in super::super) struct UnderwaterViewState {
    pub(super) camera: Option<Entity>,
    pub(in super::super) position: Option<Vec3>,
    pub(super) sample: Option<WaterViewSample>,
    pub(super) floor_y: Option<f32>, // 本帧相机下方真实河床；缺少下界的薄水面不能授权无限水下体积。
    pub(super) ceiling_y: Option<f32>, // 相机与水面之间的实际地形会阻断水体，包含从地底向上看的视角。
}

impl UnderwaterViewState {
    pub(in super::super) fn observe_surface(&mut self, surface: &super::super::components::DynamicWaterSurface,
        source_frame: Mat4, color: Color) {
        let Some(world) = self.position else { return; };
        let inverse = source_frame.inverse();
        if matches!(surface.interaction_footprint, super::super::footprint::WaterInteractionFootprint::Painted(_)) {
            // 静水互动接口的无穷深度仅表示“未记录河床”，不能直接用作相机介质范围。
            // 沿世界竖直线求实际水面高度，再使用同帧地形柱确认相机位于水面与河床之间。
            let normal = inverse.transpose().transform_vector3(Vec3::Y).try_normalize();
            let Some(normal) = normal.filter(|normal| normal.y > 0.001) else { return; };
            let plane = source_frame.transform_point3(Vec3::Y * surface.rest_water_level);
            let water_y = world.y + normal.dot(plane - world) / normal.y;
            let Some(floor_y) = self.floor_y else { return; };
            if !water_y.is_finite() || floor_y >= world.y
                || self.ceiling_y.is_some_and(|ceiling| ceiling <= water_y) {
                return;
            }
            let point = inverse.transform_point3(Vec3::new(world.x, water_y, world.z));
            if super::super::footprint::water_interaction_sample(
                &surface.interaction_footprint, surface.rest_water_level, point.xz()).is_none() {
                return;
            }
            self.observe(water_y - world.y, water_y - floor_y, color);
            return;
        }
        let local = inverse.transform_point3(world);
        let Some(sample) = super::super::footprint::water_interaction_sample(
            &surface.interaction_footprint, surface.rest_water_level, local.xz()) else { return; };
        let scale = source_frame.transform_vector3(Vec3::Y).length();
        self.observe((sample.surface_local_y - local.y) * scale, sample.author_depth * scale, color);
    }

    /// 相机必须在有限且已证明的水深内部；缺少下界的互动样本不能把地底误判为水下。
    pub(in super::super) fn observe(&mut self, depth: f32, author_depth: f32, color: Color) {
        if !depth.is_finite() || !author_depth.is_finite() || depth <= 0.0 || depth >= author_depth {
            return;
        }
        if self.sample.is_none_or(|sample| depth > sample.depth) {
            self.sample = Some(WaterViewSample { depth, color });
        }
    }
}
