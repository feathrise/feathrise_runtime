//! 编辑器预览与动态场景运行时共享的确定性植被散布。

use bevy::prelude::Vec2;

use crate::SceneVegetationLayer;

/// 一个无需ECS或场景资产句柄的稳定散布结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneVegetationInstanceTransform {
    /// 全图候选格中的稳定身份；空间分块只改变何时常驻，不改变该实例身份。
    pub instance_id: u64,
    pub translation: [f32; 3],
    pub yaw_radians: f32,
    pub scale: f32,
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn unit_from_hash(value: u64) -> f32 {
    ((value >> 40) as u32) as f32 / 16_777_215.0
}

/// 从候选序号重建稳定哈希与抖动世界位置；分块前后必须逐位一致。
fn scatter_candidate_world(
    seed: u64,      // 当前正式层的稳定散布种子。
    candidate: u64, // 行优先候选序号，跨帧和重进保持不变。
    columns: u32,   // 用于从序号恢复二维候选格坐标。
    spacing: f32,   // 作者声明的真实候选格距，运行时不得擅自放大。
    minimum: Vec2,  // 当前场景世界X/Z最小角。
) -> (u64, Vec2) {
    let x = (candidate % u64::from(columns)) as u32; // 恢复当前候选列。
    let z = (candidate / u64::from(columns)) as u32; // 恢复当前候选行。
    let hash = splitmix64(seed ^ candidate); // 同一候选只使用一个稳定根哈希。
    let jitter_x = unit_from_hash(hash) - 0.5; // X轴在正负0.4格内错位。
    let jitter_z = unit_from_hash(splitmix64(hash)) - 0.5; // Z轴使用独立稳定错位。
    let world = minimum // 从场景最小角开始建立世界候选位置。
        + Vec2::new( // 二维格中心叠加稳定抖动。
            (x as f32 + 0.5 + jitter_x * 0.8) * spacing, // X轴最终世界偏移。
            (z as f32 + 0.5 + jitter_z * 0.8) * spacing, // Z轴最终世界偏移。
        ); // 完成当前候选世界位置。
    (hash, world) // 返回后续概率、优先级、朝向和缩放共用的稳定输入。
}

fn sample_density(
    density: &[u8],
    resolution: [u32; 2],
    world: Vec2,
    world_min: Vec2,
    world_max: Vec2,
) -> f32 {
    let uv = ((world - world_min) / (world_max - world_min)).clamp(Vec2::ZERO, Vec2::ONE);
    let x = (uv.x * (resolution[0] - 1) as f32).round() as u32;
    let z = (uv.y * (resolution[1] - 1) as f32).round() as u32;
    density[(z * resolution[0] + x) as usize] as f32 / 255.0
}

/// 在规则高度场上按正式三角形剖分采样世界高度。
///
/// 编辑器光标、植被散布和后续贴地消费者必须共用这一函数，避免同一位置因
/// 最近点、双线性和三角形插值不同而出现肉眼可见的悬空或穿模。
pub fn sample_heightfield_world(
    heights: &[u16],
    resolution: [u32; 2],
    world: [f32; 2],
    world_min: [f32; 2],
    world_max: [f32; 2],
    base_height: f32,
    height_scale: f32,
) -> Option<f32> {
    if resolution[0] < 2
        || resolution[1] < 2
        || heights.len() != resolution[0] as usize * resolution[1] as usize
        || !base_height.is_finite()
        || !height_scale.is_finite()
    {
        return None;
    }
    let world = Vec2::from_array(world);
    let world_min = Vec2::from_array(world_min);
    let world_max = Vec2::from_array(world_max);
    if !world_min.cmplt(world_max).all()
        || world.cmplt(world_min).any()
        || world.cmpgt(world_max).any()
    {
        return None;
    }
    let grid = ((world - world_min) / (world_max - world_min))
        * Vec2::new((resolution[0] - 1) as f32, (resolution[1] - 1) as f32);
    let x = grid.x.floor().clamp(0.0, (resolution[0] - 2) as f32) as u32;
    let z = grid.y.floor().clamp(0.0, (resolution[1] - 2) as f32) as u32;
    let fraction = grid - Vec2::new(x as f32, z as f32);
    let decode = |raw: u16| base_height + raw as f32 / u16::MAX as f32 * height_scale;
    let nw = decode(heights[(z * resolution[0] + x) as usize]);
    let ne = decode(heights[(z * resolution[0] + x + 1) as usize]);
    let sw = decode(heights[((z + 1) * resolution[0] + x) as usize]);
    let se = decode(heights[((z + 1) * resolution[0] + x + 1) as usize]);
    Some(if fraction.x + fraction.y <= 1.0 {
        nw + fraction.x * (ne - nw) + fraction.y * (sw - nw)
    } else {
        se + (1.0 - fraction.y) * (ne - se) + (1.0 - fraction.x) * (sw - se)
    })
}

/// 从一层R8密度与高度场生成全图世界稳定实例；作者密度不会被数量预算截断。
#[allow(clippy::too_many_arguments)]
pub fn generate_vegetation_instances(
    layer: &SceneVegetationLayer,
    density: &[u8],
    heights: &[u16],
    terrain_resolution: [u32; 2],
    world_min: [f32; 2],
    world_max: [f32; 2],
    base_height: f32,
    height_scale: f32,
) -> Result<Vec<SceneVegetationInstanceTransform>, String> {
    generate_vegetation_instances_in_region(
        layer,              // 使用作者声明的格距、缩放和稳定种子。
        density,            // 完整R8密度图决定每个候选是否存在。
        heights,            // 完整R16高度场负责最终贴地。
        terrain_resolution, // 高度场顶点尺寸。
        world_min,          // 全图世界最小角同时作为稳定候选原点。
        world_max,          // 全图世界最大角。
        base_height,        // R16零值对应世界高度。
        height_scale,       // R16完整量程。
        world_min,          // 全图入口直接请求场景最小角。
        world_max,          // 全图入口直接请求场景最大角。
    ) // 复用分块纯函数，保证全图和按需常驻得到完全相同的实例集合。
}

/// 只生成与一个半开世界矩形相交的稳定实例；相邻分块合并后严格等于全图结果。
#[allow(clippy::too_many_arguments)]
pub fn generate_vegetation_instances_in_region(
    layer: &SceneVegetationLayer,
    density: &[u8],
    heights: &[u16],
    terrain_resolution: [u32; 2],
    world_min: [f32; 2],
    world_max: [f32; 2],
    base_height: f32,
    height_scale: f32,
    region_min: [f32; 2],
    region_max: [f32; 2],
) -> Result<Vec<SceneVegetationInstanceTransform>, String> {
    if density.len() != layer.resolution[0] as usize * layer.resolution[1] as usize {
        return Err(format!("植被层{}密度源尺寸错误", layer.id));
    }
    if heights.len() != terrain_resolution[0] as usize * terrain_resolution[1] as usize
        || terrain_resolution[0] < 2
        || terrain_resolution[1] < 2
    {
        return Err("植被散布缺少完整高度场".to_owned());
    }
    let min = Vec2::from_array(world_min);
    let max = Vec2::from_array(world_max);
    let extent = max - min;
    if !min.cmplt(max).all() || !layer.spacing_meters.is_finite() || layer.spacing_meters <= 0.0 {
        return Err("植被散布范围或间距无效".to_owned());
    }
    let requested_min = Vec2::from_array(region_min); // 调用方可请求跨出场景的相机常驻矩形。
    let requested_max = Vec2::from_array(region_max); // 请求边界只影响当前返回集合，不参与稳定哈希。
    if !requested_min.is_finite() || !requested_max.is_finite() {
        return Err("植被散布分块范围包含非有限值".to_owned()); // NaN或无穷不能进入格坐标换算。
    }
    let region_min = requested_min.max(min); // 常驻矩形在场景外的部分没有候选。
    let region_max = requested_max.min(max); // 场景最大角是最后一个分块唯一允许的闭边界。
    if !region_min.cmplt(region_max).all() {
        return Ok(Vec::new()); // 完全位于场景外的分块是合法空结果，不制造失败重试。
    }
    let spacing = layer.spacing_meters; // 密度语义只使用作者间距，绝不因场景大小或硬件自动变稀。
    let columns = (extent.x / spacing).ceil().max(1.0) as u32;
    let rows = (extent.y / spacing).ceil().max(1.0) as u32;
    let axis_range = |region_start: f32, region_end: f32, scene_start: f32, count: u32| {
        let first =
            (((region_start - scene_start) / spacing).floor() as i64 - 1).clamp(0, i64::from(count)); // 抖动最多跨零点四格，向前扩一格即可覆盖跨分块候选。
        let end =
            (((region_end - scene_start) / spacing).ceil() as i64 + 1).clamp(first, i64::from(count)); // 向后同样扩一格，再由最终世界坐标执行半开过滤。
        first as u32..end as u32 // 返回的候选格范围始终落在全图网格内。
    };
    let x_range = axis_range(region_min.x, region_max.x, min.x, columns); // 取得可能抖入当前分块的列。
    let z_range = axis_range(region_min.y, region_max.y, min.y, rows); // 取得可能抖入当前分块的行。
    let candidate_capacity = u64::from(x_range.end - x_range.start)
        .saturating_mul(u64::from(z_range.end - z_range.start))
        .min(16_384) as usize; // 预分配只是降低小块扩容，绝不限制最终作者实例数量。
    let mut transforms = Vec::with_capacity(candidate_capacity); // 每个通过密度的候选都会进入结果。
    for z in z_range {
        for x in x_range.clone() {
            let candidate = u64::from(z) * u64::from(columns) + u64::from(x); // 身份始终使用全图行优先序号。
            let (hash, world) = scatter_candidate_world(
                layer.seed, // 当前正式层种子。
                candidate,  // 当前行优先候选身份。
                columns,    // 二维格列数。
                spacing,    // 作者声明的真实格距。
                min,        // 当前场景世界最小角。
            ); // 完成稳定候选位置重建。
            let inside_x = world.x >= region_min.x
                && (world.x < region_max.x || (region_max.x == max.x && world.x <= max.x)); // 最东分块独占场景最大X边界。
            let inside_z = world.y >= region_min.y
                && (world.y < region_max.y || (region_max.y == max.y && world.y <= max.y)); // 最南分块独占场景最大Z边界。
            if !inside_x || !inside_z || world.cmplt(min).any() || world.cmpgt(max).any() {
                continue; // 半开矩形保证跨块无重复，场景边界过滤抖出范围的候选。
            }
            let probability = sample_density(density, layer.resolution, world, min, max); // 读取作者R8在当前稳定世界点的概率。
            let probability_roll = unit_from_hash(splitmix64(hash ^ 0xA5A5_A5A5_A5A5_A5A5)); // 概率随机域与姿态保持独立。
            if probability <= 0.0 || probability_roll > probability {
                continue; // 零密度严格为空，其余密度逐候选稳定判定且不参与数量裁剪。
            }
            let y = sample_heightfield_world(
                heights,            // 完整R16高度顶点。
                terrain_resolution, // 高度场顶点尺寸。
                world.to_array(),   // 当前候选世界X/Z。
                world_min,          // 高度场世界最小角。
                world_max,          // 高度场世界最大角。
                base_height,        // R16零值对应高度。
                height_scale,       // R16完整量程高度。
            )
            .expect("候选点已经通过高度场尺寸和世界范围校验"); // 首遍范围校验保证采样成功。
            let scale_t = unit_from_hash(splitmix64(hash ^ 0xC3C3_C3C3_C3C3_C3C3)); // 独立稳定体量随机值。
            let scale = // 把零至一稳定值映射到作者缩放范围。
            layer.scale_range[0] + (layer.scale_range[1] - layer.scale_range[0]) * scale_t; // 完成统一缩放。
            transforms.push(SceneVegetationInstanceTransform {
                instance_id: candidate,             // 分块卸载和重载后继续使用同一全图稳定ID。
                translation: [world.x, y, world.y], // 保持世界X/Y/Z并使用正式贴地高度。
                yaw_radians: unit_from_hash(splitmix64(hash ^ 0x5A5A_5A5A_5A5A_5A5A)) // 使用独立稳定朝向值。
                * std::f32::consts::TAU, // 映射到完整一周弧度。
                scale: scale.max(0.01),             // 防御坏旧范围产生零或负体量。
            }); // 完成当前实例姿态。
        }
    }
    Ok(transforms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SceneVegetationKind;

    #[test]
    fn scatter_is_deterministic_and_ignores_legacy_instance_limit() {
        let layer = SceneVegetationLayer {
            id: "tree".to_owned(),
            label: "树".to_owned(),
            kind: SceneVegetationKind::Mesh,
            runtime_profile: None,
            runtime_layer_id: None,
            prototype_id: None,
            asset_path: "tree.glb".to_owned(),
            density_source_path: "tree.r8".to_owned(),
            density_texture_path: "tree.ktx2".to_owned(),
            resolution: [2, 2],
            spacing_meters: 1.0,
            scale_range: [1.0, 1.0],
            max_instances: 3, // 动态通用散布不得再把兼容字段当成作者数量上限。
            seed: 7,
        };
        let build = || {
            generate_vegetation_instances(
                &layer,
                &[255; 4],
                &[u16::MAX / 2; 9],
                [3, 3],
                [0.0, 0.0],
                [10.0, 10.0],
                -10.0,
                20.0,
            )
            .unwrap()
        };
        assert_eq!(build(), build());
        assert_eq!(build().len(), 100); // 一百个满密度候选全部保留，不读取旧三根声明。
    }

    #[test] // 相邻常驻分块必须无重复、无遗漏地重建全图作者结果。
    fn regional_scatter_reconstructs_the_full_layout() {
        let layer = SceneVegetationLayer {
            // 构造一百米方形场景上的一米满密度候选层。
            id: "dense-tree".to_owned(),               // 稳定测试层身份。
            label: "密集树木".to_owned(),              // 仅用于失败诊断。
            kind: SceneVegetationKind::Mesh,             // 使用CPU Mesh散布路径。
            runtime_profile: None,                     // 纯算法测试不依赖场景profile。
            runtime_layer_id: None,                    // 纯算法测试不需要运行时层ID。
            prototype_id: None,                        // 纯算法测试不生成原型。
            asset_path: "tree.glb".to_owned(),         // 保持完整正式层结构。
            density_source_path: "tree.r8".to_owned(), // 保持完整作者源字段。
            density_texture_path: "tree.ktx2".to_owned(), // 保持完整运行时纹理字段。
            resolution: [2, 2],                        // 四个满值纹素覆盖整个场景。
            spacing_meters: 1.0,                       // 生成一万个候选，远高于兼容字段声明的一根。
            scale_range: [1.0, 1.0],                   // 排除体量变化干扰。
            max_instances: 1,                          // 兼容字段故意很小，证明分块同样不会截断。
            seed: 0xD3E5_E7E5,                         // 固定种子锁定稳定空间排名。
        }; // 完成密集层fixture。
        let full = generate_vegetation_instances(
            // 执行作者精确全图散布。
            &layer,             // 当前密集测试层。
            &[u8::MAX; 4],      // 全图百分之百密度。
            &[u16::MAX / 2; 9], // 三乘三平坦高度场。
            [3, 3],             // 高度场顶点尺寸。
            [0.0, 0.0],         // 世界最小角。
            [100.0, 100.0],     // 世界最大角。
            -10.0,              // 高度解码基线。
            20.0,               // 高度解码量程。
        )
        .expect("密集满图散布应保持合法"); // 后台算法不得因作者实例很多而静默抽样。
        let build_region = |minimum: [f32; 2], maximum: [f32; 2]| {
            generate_vegetation_instances_in_region(
                &layer,             // 使用同一正式层与稳定种子。
                &[u8::MAX; 4],      // 同一满密度作者图。
                &[u16::MAX / 2; 9], // 同一平坦高度场。
                [3, 3],             // 同一高度场尺寸。
                [0.0, 0.0],         // 全图稳定原点保持不变。
                [100.0, 100.0],     // 全图边界保持不变。
                -10.0,              // 同一高度基线。
                20.0,               // 同一高度量程。
                minimum,            // 当前半开常驻块最小角。
                maximum,            // 当前半开常驻块最大角。
            )
            .expect("合法分块应可独立生成")
        };
        let mut tiled = build_region([0.0, 0.0], [50.0, 100.0]); // 生成西半个场景。
        tiled.extend(build_region([50.0, 0.0], [100.0, 100.0])); // 生成东半张并合并。
        tiled.sort_unstable_by_key(|transform| transform.instance_id); // 还原全图行优先身份顺序。
        assert_eq!(full.len(), 10_000); // 一米满密度百米方图必须完整生成一万个实例。
        assert_eq!(tiled, full); // 两块合并必须与一次全图生成逐值相同且不存在重复。
    }

    #[test]
    fn shared_height_sample_matches_the_mesh_triangle_diagonal() {
        // 西北、东北、西南为零，东南为最高；中心恰落在东南三角形的顶点权重上。
        let heights = [0, 0, 0, u16::MAX];
        assert_eq!(
            sample_heightfield_world(
                &heights,
                [2, 2],
                [0.5, 0.5],
                [0.0, 0.0],
                [1.0, 1.0],
                -2.0,
                10.0,
            ),
            Some(-2.0)
        );
        assert_eq!(
            sample_heightfield_world(
                &heights,
                [2, 2],
                [1.0, 1.0],
                [0.0, 0.0],
                [1.0, 1.0],
                -2.0,
                10.0,
            ),
            Some(8.0)
        );
    }
}
