//! 动态水域后台任务的不可变输入与纯网格构建。

use std::sync::Arc; // 无Mip R8作者资产以Arc快照进入任务，全部静水构建共享且不深拷贝整图。

#[cfg(test)]
use feathrise_scene_content::{SceneStillWaterKind, SceneWaterSegmentKind, SceneWaterSplinePoint};
use feathrise_scene_content::{SceneWaterLayer, SceneWaterRiver, SceneWaterfall}; // single-flight冻结静水层、河流与瀑布的完整作者值。 // 测试静水字面量显式锁定旧文件Lake缺省语义。

use crate::shared::large_scene::{
    BuiltWaterSurface, PaintedWaterTerrainInput, chunk_water_surfaces,
    WaterSurfaceDescriptor, build_painted_water_mesh_with_depth, build_river_water_surface,
    build_waterfall_water_surface,
}; // 唯一湖面/带状水构建实现和全层共享的GLB派生高度输入。

/// 单层受检R8快照；不借用主世界Image或Assets。
pub(super) struct DynamicWaterBuildInput {
    pub(super) layer: SceneWaterLayer,
    pub(super) mask: Arc<[u8]>,
    pub(super) descriptor: WaterSurfaceDescriptor, // 主线程已按同一目录修订解析预设和图片强句柄。
}

/// 构建当前场景全部静水、河流和瀑布；任一水体失败即整批失败，主世界不得采用部分水面。
pub(super) fn build_dynamic_water_network(
    inputs: Vec<DynamicWaterBuildInput>, // 全部层只各自持有R8，避免为每层复制派生高度网格。
    rivers: Vec<SceneWaterRiver>,          // 三维河流作者值已经脱离主世界资产借用。
    waterfalls: Vec<SceneWaterfall>,       // 瀑布作者值已经脱离主世界资产借用。
    world_min: [f32; 2],                 // 当前受检清单的世界XZ最小值。
    world_max: [f32; 2],                 // 当前受检清单的世界XZ最大值。
    terrain: Option<PaintedWaterTerrainInput>, // 有地形时共享真实高度；独立根画布明确没有河床依赖。
) -> Result<Vec<BuiltWaterSurface>, String> {
    let mut builds = Vec::with_capacity(inputs.len() + rivers.len() + waterfalls.len()); // 仅在后台任务内持有下一完整批次。
    for input in inputs {
        let minimum_depth = input.descriptor.minimum_stable_depth();
        let (layer_min, layer_max) = input.layer.effective_canvas_bounds(world_min, world_max);
        let mesh = if let Some(terrain) = terrain.as_ref() { build_painted_water_mesh_with_depth(
            input.mask.as_ref(),     // 当前层受检R8基础级快照。
            input.layer.resolution,  // 遮罩尺寸仍由水JSON定义。
            layer_min,               // 旧层沿用来源范围；独立层使用自己的R8画布。
            layer_max,
            terrain,                 // 借用同一派生高度，不发生逐层Arc克隆或整图复制。
            input.layer.water_level, // 深度核心按每层独立水位裁掉无效高地。
            minimum_depth,           // 大风浪预设自动要求更深的稳定核心，波谷不会露底。
        )? } else {
            crate::shared::large_scene::PaintedWaterMeshData::from_independent_mask(
                input.mask.as_ref(), input.layer.resolution, layer_min, layer_max, input.layer.water_level,
            )?
        }; // 独立画布只省略河床判断；动态材质、R8边缘、自由波和后续GPU采用保持同一条链。
        builds.push(BuiltWaterSurface {
            descriptor: input.descriptor,
            mesh,
        }); // 合法空静水也保留至主线程统一跳过。
    }
    builds.extend(build_dynamic_spline_water_network(rivers, waterfalls)?); // 两种地形路线共用世界XYZ样条后端。
    chunk_water_surfaces(builds) // 完整几何在后台分块，再分帧上传并原子采用。
}

/// 构建不依赖整图地形高度的河流、急流与瀑布批次。
///
/// 【功能设计】控制点已经冻结完整世界XYZ；Clipmap与GLB运行时必须调用同一纯函数，
/// 以共享采样、样式编码、互动足迹和分块上传，禁止为分页地形维护第二套Ribbon。
pub(super) fn build_dynamic_spline_water_network(
    rivers: Vec<SceneWaterRiver>,
    waterfalls: Vec<SceneWaterfall>,
) -> Result<Vec<BuiltWaterSurface>, String> {
    let mut builds = Vec::with_capacity(rivers.len() + waterfalls.len());
    let mut spline_triangle_count = 0usize; // 只检查实际整数累加溢出，不限制全图三角数量。
    for river in &rivers {
        let build = build_river_water_surface(river)?; // 按完整路径采样并建立同源局部足迹。
        spline_triangle_count = checked_spline_triangle_total(
            spline_triangle_count,       // 当前已经完成的河流与瀑布三角数。
            build.mesh.triangle_count(), // 当前河流实际三角数。
        )?;
        builds.push(build); // 仅本地追加，任务成功前主世界仍不可见。
    }
    for waterfall in &waterfalls {
        let build = build_waterfall_water_surface(waterfall)?; // 按完整幕布采样且不建玩家互动足迹。
        spline_triangle_count = checked_spline_triangle_total(
            spline_triangle_count,       // 当前已经完成的河流与瀑布三角数。
            build.mesh.triangle_count(), // 当前瀑布实际三角数。
        )?;
        builds.push(build); // 最后一类同样只进入未发布后台批次。
    }
    chunk_water_surfaces(builds) // 分块只改变上传粒度，保留全部样条几何。
}

/// 汇总全图样条水三角数量，只在实际整数溢出时拒绝整批。
fn checked_spline_triangle_total(current: usize, added: usize) -> Result<usize, String> {
    let total = current
        .checked_add(added)
        .ok_or_else(|| "场景样条水三角形总数溢出".to_owned())?; // 损坏完成值不能绕过usize边界。
    Ok(total) // 返回下一次累计使用的受检总数。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接验证动态运行时批次把同一派生高度传给每个独立水位层。

    fn layer(id: &str, water_level: f32) -> SceneWaterLayer {
        SceneWaterLayer {
            id: id.to_owned(),    // 测试层使用稳定且互不重复的身份。
            label: id.to_owned(), // 诊断名不影响纯网格结果。
            mask_source_path: format!("scenes/MAP-test/water/{id}.r8"), // 保持正式作者源路径形状。
            mask_texture_path: format!("scenes/MAP-test/water/{id}.ktx2"), // 保持运行时派生路径形状。
            resolution: [2, 2],   // 四个R8像素覆盖最小合法测试网格。
            canvas_bounds: None,
            water_level,          // 两层通过不同静止水位验证共享河床的独立深度判定。
            still_kind: SceneStillWaterKind::Lake, // 测试层沿用旧文件缺省的湖泊静水语义。
            preset_id: String::new(), // 纯构建测试固定走兼容外观。
            flow_direction: [1.0, 0.0], // 流向只进入材质，不影响深度核心。
            flow_speed: 1.0,      // 使用正式默认流速。
            base_color_srgba: [0.05, 0.24, 0.29, 0.72], // 有效PBR水色。
            perceptual_roughness: 0.22, // 有效正式粗糙度。
        }
    }

    #[test]
    fn one_in_memory_height_snapshot_keeps_dry_and_safe_layers_in_the_same_batch() {
        let terrain = PaintedWaterTerrainInput::new(
            Arc::<[u16]>::from(vec![u16::MAX; 4]), // 四个地形顶点全部位于世界Y十米。
            [2, 2],                                // 量化高度与测试世界角点一一对应。
            [-1.0, -1.0],                          // 测试场景XZ最小角。
            [1.0, 1.0],                            // 测试场景XZ最大角。
            0.0,                                   // u16零值对应世界Y零米。
            10.0,                                  // u16最大值对应世界Y十米。
        )
        .expect("测试派生高度输入应有效"); // 构造阶段先验证尺寸、范围和米制参数。
        let full_mask = Arc::<[u8]>::from(vec![u8::MAX; 4]); // 两层都请求覆盖完整测试范围。
        let builds = build_dynamic_water_network(
            vec![
                DynamicWaterBuildInput {
                    layer: layer("below-terrain", 1.0), // 静水位低于河床，应被深度核心全部裁掉。
                    mask: Arc::clone(&full_mask),       // 共享R8只为隔离水位差异。
                    descriptor: WaterSurfaceDescriptor::from_layer(&layer("below-terrain", 1.0)),
                },
                DynamicWaterBuildInput {
                    layer: layer("above-terrain", 12.0), // 静水位高于河床，应保留有效水面。
                    mask: full_mask,                     // 最后一层取得同一不可变遮罩所有权。
                    descriptor: WaterSurfaceDescriptor::from_layer(&layer("above-terrain", 12.0)),
                },
            ],
            Vec::new(),   // 本回归只隔离静水派生高度原子语义。
            Vec::new(),   // 本回归只隔离静水派生高度原子语义。
            [-1.0, -1.0], // 全部层使用同一场景范围。
            [1.0, 1.0],   // 全部层使用同一场景范围。
            Some(terrain), // 整批只传入一份Arc<u16>封装。
        )
        .expect("无安全交集的静水层应干涸，但不能阻止同批其他安全水面");

        assert_eq!(builds.len(), 2); // 两个稳定层身份都保留到主线程统一采用边界。
        assert!(builds[0].mesh.is_empty()); // 低水位层不生成穿地几何或互动足迹。
        assert!(!builds[1].mesh.is_empty()); // 高水位层仍从同一派生高度快照生成有效水面。
    }

    #[test]
    fn saved_independent_water_reopens_with_its_own_bounds_and_no_height_cache() {
        let mut independent = layer("standalone", -4.);
        independent.canvas_bounds = Some([[-128., -64.], [128., 64.]]);
        let independent: SceneWaterLayer = serde_json::from_slice(&serde_json::to_vec(&independent).unwrap()).unwrap();
        let builds = build_dynamic_water_network(vec![DynamicWaterBuildInput {
            descriptor: WaterSurfaceDescriptor::from_layer(&independent), layer: independent,
            mask: Arc::from([255; 4]),
        }], Vec::new(), Vec::new(), [-0.5; 2], [0.5; 2], None).unwrap();
        assert_eq!(builds.len(), 1);
        assert!(!builds[0].mesh.is_empty());
        assert_eq!(builds[0].descriptor.translation_y, -4.);
        assert_eq!(builds[0].descriptor.kind, crate::shared::large_scene::WaterSurfaceKind::Still);
    }

    #[test]
    fn spline_triangle_total_accepts_the_old_budget_and_checks_integer_overflow() {
        use crate::MAX_SCENE_SPLINE_WATER_TRIANGLES;
        let within_budget = checked_spline_triangle_total(
            MAX_SCENE_SPLINE_WATER_TRIANGLES - 2, // 模拟同批已完成的全部河流三角数。
            2,                                  // 跨过旧固定预算边界的前一项。
        )
        .expect("等于旧预算阈值的完整批次保持合法");

        assert_eq!(within_budget, MAX_SCENE_SPLINE_WATER_TRIANGLES); // 边界值必须可确定采用。
        assert_eq!(checked_spline_triangle_total(within_budget, 1).unwrap(), within_budget + 1);
        assert!(checked_spline_triangle_total(usize::MAX, 1).is_err()); // 只拒绝真实整数格式溢出。
    }

    #[test]
    fn spline_batch_builds_world_xyz_ribbon_without_terrain_input() {
        let river = SceneWaterRiver {
            id: "river-clipmap".to_owned(),
            label: "Clipmap急流".to_owned(),
            source_layer_id: None,
            target_layer_id: None,
            points: vec![
                SceneWaterSplinePoint {
                    id: "upstream".to_owned(),
                    position: [4.0, 8.0, 6.0],
                    width: 6.0,
                    depth: 1.5,
                    flow_speed: 2.0,
                    outgoing: SceneWaterSegmentKind::Rapid,
                },
                SceneWaterSplinePoint {
                    id: "downstream".to_owned(),
                    position: [12.0, 5.0, 18.0],
                    width: 7.0,
                    depth: 1.8,
                    flow_speed: 3.0,
                    outgoing: SceneWaterSegmentKind::River,
                },
            ],
            base_color_srgba: [0.05, 0.24, 0.29, 0.72],
            perceptual_roughness: 0.22,
        };
        let builds = build_dynamic_spline_water_network(vec![river], Vec::new())
            .expect("世界XYZ样条不应要求GLB或Clipmap整图高度输入");
        assert_eq!(builds.len(), 1);
        assert!(!builds[0].mesh.is_empty());
        assert_eq!(
            builds[0].descriptor.kind,
            crate::shared::large_scene::WaterSurfaceKind::River
        );
    }
}
