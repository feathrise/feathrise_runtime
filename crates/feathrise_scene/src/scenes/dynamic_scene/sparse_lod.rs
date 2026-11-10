//! Clipmap统一观察相机与作者档位选择；只读固定大小范围，不生成配置之外的显示格距。

use super::sparse_runtime::DynamicClipmapAuthoringView;
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::{
    CompiledDisplayLodPlan, SparseHeightSpec, compile_display_lod_plan_suffix,
};
use feathrise_scene_core::residency::SceneSourceFrame;

const HYSTERESIS: f32 = 1.15; // 仅覆盖包络保留15%迟滞，不改写作者档位和切换半径。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ViewLodKey {
    pub minimum_multiplier: u32,
    pub coverage_cells: [u64; 2],
}

#[derive(Clone, Copy)]
pub(super) struct ClipmapView {
    pub position: Vec3,
}

#[derive(SystemParam)]
pub(super) struct ClipmapViews<'w, 's> {
    cameras: Query<
        'w,
        's,
        (
            &'static Camera,
            &'static GlobalTransform,
            &'static Projection,
            Option<&'static DynamicClipmapAuthoringView>,
        ),
        With<Camera3d>,
    >,
}

impl ClipmapViews<'_, '_> {
    pub fn resolve(&self, gameplay: Option<Entity>) -> Option<ClipmapView> {
        // 编辑器与游戏都从实际相机矩阵计算；不把缩略图、小场景或每个对象中心当观察点。
        let selected = self
            .cameras
            .iter()
            .find(|(camera, _, _, editor)| camera.is_active && editor.is_some())
            .or_else(|| {
                self.cameras
                    .get(gameplay?)
                    .ok()
                    .filter(|(camera, _, _, _)| camera.is_active)
            })?;
        let (camera, transform, projection, _) = selected;
        let viewport = camera.physical_viewport_size()?.as_vec2();
        ClipmapView::new(transform, projection, viewport)
    }
}

impl ClipmapView {
    pub(super) fn new(
        transform: &GlobalTransform,
        projection: &Projection,
        viewport: Vec2,
    ) -> Option<Self> {
        let clip = projection.get_clip_from_view() * transform.affine().inverse();
        if !clip.is_finite()
            || !viewport.is_finite()
            || viewport.min_element() <= 0.0
            || !clip.inverse().is_finite()
        {
            return None;
        }
        Some(Self {
            position: transform.translation(),
        })
    }

    pub(super) fn choice(
        &self,
        spec: &SparseHeightSpec,
        frame: SceneSourceFrame,
        height_bounds: [f32; 2],
        previous: Option<ViewLodKey>,
        authored: &CompiledDisplayLodPlan,
    ) -> Option<(Vec2, ViewLodKey)> {
        let camera = frame.to_local(self.position);
        let min = Vec2::from_array(spec.world_min);
        let max = Vec2::from_array(spec.world_max);
        let spacing = Vec2::new(
            spec.actual_spacing_meters[0] as f32,
            spec.actual_spacing_meters[1] as f32,
        );
        if !camera.is_finite()
            || height_bounds.iter().any(|height| !height.is_finite())
            || height_bounds[0] > height_bounds[1]
        {
            return None;
        }
        // 作者倍率和世界半径是显示合同。仅剔除连来源包围盒都触及不到的细层；
        // 不再以Y=0平面的投影、镜头朝向或窗口大小派生任意新档位。
        let last_multiplier = authored.bands.last()?.spacing_multiplier;
        let horizontal_gap = (camera.xz() - camera.xz().clamp(min, max)).abs();
        let vertical_gap = (camera.y - camera.y.clamp(height_bounds[0], height_bounds[1])).abs();
        let first = authored.bands.iter().enumerate().find(|(index, band)| {
            if *index + 1 == authored.bands.len() { return true; }
            let outer = Vec2::from_array(authored.actual_half_extents_meters[*index]);
            let guard = spacing * (band.spacing_multiplier as f32 + 1.0);
            horizontal_gap.cmple(outer + guard).all() && vertical_gap <= outer.max_element()
        })?.1;
        let multiplier = first.spacing_multiplier;
        // 有细环时，全部网格继续共用实际世界相机，不能各自钳制出一个近场中心。
        // 全源都已落入最后均匀档时，固定布局在来源中心；图外无效环不能迫使预算再造更粗档。
        let center = if multiplier == last_multiplier { (min + max) * 0.5 } else { camera.xz() };
        let focus = min + (((center - min) / spacing) + Vec2::splat(0.5)).floor() * spacing;
        let radius = (focus - min).abs().max((focus - max).abs()) / spacing;
        let coverage = |axis: usize| -> Option<u64> {
            // 二次幂包络复用同一Mesh；预留独立粗层吸附误差，镜头平移不会逐细格重建。
            let required = (radius[axis].ceil() as u64).checked_add(u64::from(last_multiplier) * 2)?;
            // 仍含细环时，相机不可能比最后有限环更远。把二次幂包络限制到这个保守上界，
            // 避免刚出边界就将整个源外空环翻倍，导致本来合法的双网格首载超过16MiB。
            let maximum = if multiplier != last_multiplier {
                let fine = authored.bands.get(authored.bands.len().checked_sub(2)?)?;
                spec.cells[axis]
                    .checked_add(u64::from(fine.outer_half_cells[axis]) * u64::from(fine.spacing_multiplier))?
                    .checked_add(u64::from(fine.spacing_multiplier) + 2)?
                    .checked_add(u64::from(last_multiplier) * 2)?
                    .max(required)
            } else { u64::MAX };
            if let Some(old) = previous.filter(|key| key.minimum_multiplier == multiplier)
                .map(|key| key.coverage_cells[axis])
                .filter(|old| *old <= maximum)
                && required <= old
                && required as f64 > old as f64 / (2.0 * f64::from(HYSTERESIS))
            {
                return Some(old); // 扩展始终立即进行；缩回需跨过内侧迟滞门，防止边界抖动反复重建/上传。
            }
            Some(required.checked_next_power_of_two()?.min(maximum))
        };
        Some((
            focus,
            ViewLodKey {
                minimum_multiplier: multiplier,
                coverage_cells: [coverage(0)?, coverage(1)?],
            },
        ))
    }
}

#[cfg(test)]
#[path = "sparse_lod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sparse_lod_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "sparse_lod_authored_tests.rs"]
mod authored_tests;

pub(super) fn compile_view_plan(
    spec: &SparseHeightSpec,
    authored: &CompiledDisplayLodPlan,
    key: ViewLodKey,
) -> Result<CompiledDisplayLodPlan, String> {
    // 预算失败保留上一完整版本，不能用翻倍倍率偷偷突破用户设置的最后一档。
    // 复用已量化的每轴边界，不能取X/Z较大值再量化一次而偷偷扩大矩形源的有限半径。
    compile_display_lod_plan_suffix(spec, authored, key.minimum_multiplier, key.coverage_cells)
}
