//! 开发者预设目录的原子结构、数值、路径与跨项身份校验。

use std::collections::BTreeSet;

use super::*; // 复用 MAX/MIN_BLADE_PRESET_LODS 与全部作者类型。

pub(super) fn validate_catalog(asset: &VegetationPresetCatalogAsset) -> Result<(), String> {
    if asset.schema_version != VEGETATION_PRESET_SCHEMA_VERSION {
        return Err(format!(
            "植被预设目录版本{}与运行时版本{}不一致",
            asset.schema_version, VEGETATION_PRESET_SCHEMA_VERSION
        )); // 不兼容目录不得部分采用。
    }
    let mut names = BTreeSet::new(); // 当前 ID 与历史别名共享唯一命名空间。
    let mut output_ids = BTreeSet::new(); // 默认正式输出 ID 不得让新建层互相覆盖。
    let mut runtime_ids = BTreeSet::new(); // 显式或派生层身份碰撞必须在加载目录时暴露。
    let mut prototype_ids = BTreeSet::new(); // Mesh 原型碰撞同样必须原子拒绝。
    for preset in &asset.presets {
        validate_preset(preset)?; // 先校验当前项全部字段，再登记跨项身份。
        if !names.insert(preset.id.as_str()) {
            return Err(format!("植被预设 ID 重复：{}", preset.id)); // 同一规范 ID 不能指向两个内容。
        }
        for alias in &preset.aliases {
            if !names.insert(alias.as_str()) {
                return Err(format!("植被预设历史别名重复或占用现有 ID：{alias}")); // 别名解析不得依赖声明顺序。
            }
        }
        if !output_ids.insert(preset.output_id.as_str()) {
            return Err(format!("植被预设默认输出 ID 重复：{}", preset.output_id)); // 懒创建正式输出需要无歧义路径。
        }
        let runtime_id = preset.resolved_runtime_layer_id(); // 显式和自动身份统一进入碰撞检查。
        if !runtime_ids.insert(runtime_id) {
            return Err(format!("植被预设运行时层 ID 冲突：{runtime_id}")); // 哈希碰撞可由开发者显式指定 ID 解决。
        }
        if let Some(prototype_id) = preset.resolved_prototype_id()
            && !prototype_ids.insert(prototype_id)
        {
            return Err(format!("植被预设 Mesh 原型 ID 冲突：{prototype_id}")); // 不允许注册表后写覆盖先写。
        }
    }
    Ok(()) // 预设数量只受实际资产与内存能力约束，不设置产品计数上限。
}

fn validate_preset(preset: &VegetationPresetDescriptor) -> Result<(), String> {
    if !safe_stable_name(&preset.id)
        || !safe_stable_name(&preset.output_id)
        || !safe_asset_path(&preset.author_asset_path)
    {
        return Err(format!("植被预设 {} 的 ID 或默认输出 ID 无效", preset.id)); // 稳定身份禁止路径字符和空白。
    }
    if preset.aliases.iter().any(|alias| !safe_stable_name(alias)) {
        return Err(format!("植被预设 {} 包含无效历史别名", preset.id)); // 别名服从同一稳定命名规则。
    }
    if preset.label.trim().is_empty()
        || preset.resolution.iter().any(|value| *value < 2)
        || !preset.spacing_meters.is_finite()
        || preset.spacing_meters <= 0.0
        || !finite_slice(&preset.scale_range)
        || preset.scale_range[0] <= 0.0
        || preset.scale_range[1] < preset.scale_range[0]
    {
        return Err(format!(
            "植被预设 {} 的名称、画布、间距或缩放无效",
            preset.id
        )); // 只拒绝不能安全表达的输入，不设置内容数量上限。
    }
    match &preset.runtime {
        VegetationPresetRuntime::Blade {
            lods,
            appearance,
            base_color_texture_path,
            motion,
            bounds,
            view_lod,
            density_gain,
            ultra_horizontal_sweep,
            ..
        } => validate_blade(
            preset,
            lods,
            appearance,
            base_color_texture_path.as_deref(),
            motion,
            bounds,
            view_lod,
            *density_gain,
            *ultra_horizontal_sweep,
        )?,
        VegetationPresetRuntime::Mesh {
            lods,
            ground_embed_depth,
            collision,
        } => validate_mesh(preset, lods, *ground_embed_depth, collision)?,
    }
    Ok(()) // 当前项全部结构和数值可进入运行时解析。
}

#[allow(clippy::too_many_arguments)] // 参数与 JSON Blade 子结构一一对应，保持失败字段可定位。
fn validate_blade(
    preset: &VegetationPresetDescriptor,
    lods: &[BladePresetLod],
    appearance: &BladePresetAppearance,
    base_color_texture_path: Option<&str>,
    motion: &BladePresetMotion,
    bounds: &BladePresetBounds,
    view_lod: &BladePresetViewLod,
    density_gain: f32,
    ultra_horizontal_sweep: f32,
) -> Result<(), String> {
    if lods.len() < MIN_BLADE_PRESET_LODS || lods.len() > MAX_BLADE_PRESET_LODS {
        return Err(format!(
            "植被预设 {} 的 Blade LOD 档数必须在 {MIN_BLADE_PRESET_LODS}..={MAX_BLADE_PRESET_LODS}",
            preset.id
        )); // GPU 紧凑实例三位几何档号上限为八档。
    }
    let mut levels = BTreeSet::new(); // level 在档内唯一，并按升序映射到几何 0..n-1。
    for lod in lods {
        if lod.name.trim().is_empty()
            || lod.level as usize >= MAX_BLADE_PRESET_LODS
            || !levels.insert(lod.level)
            || !safe_asset_path(&lod.mesh_path)
            || lod.grid_side < 2
            || !finite_slice(&[
                lod.spacing,
                lod.inner_radius,
                lod.outer_radius,
                lod.inner_fade_width,
                lod.outer_fade_width,
            ])
            || lod.spacing <= 0.0
            || lod.inner_radius < 0.0
            || lod.outer_radius <= lod.inner_radius
            || lod.inner_fade_width < 0.0
            || lod.outer_fade_width <= 0.0
            || lod.snap_cells == 0
            || !lod.patch_extent().is_finite()
        {
            return Err(format!("植被预设 {} 包含无效 Blade LOD", preset.id)); // 任一档失败拒绝完整预设。
        }
    }
    let expected: BTreeSet<u32> = (0..lods.len() as u32).collect();
    if levels != expected {
        return Err(format!(
            "植被预设 {} 的 Blade LOD level 必须恰好覆盖 0..{}",
            preset.id,
            lods.len().saturating_sub(1)
        )); // 数组顺序可变但身份必须完整连续，运行时才能按下标建窗。
    }
    if !validate_view_lod(view_lod, lods.len()) {
        return Err(format!(
            "植被预设 {} 的 Blade 视图 LOD 与档数不一致或无效",
            preset.id
        )); // 屏幕阈值与常驻半径必须和档数对齐。
    }
    let appearance_values = [
        appearance.geometry,
        appearance.width_flexibility,
        appearance.bending,
        appearance.atlas_uv,
        appearance.root_color,
        appearance.tip_color,
        appearance.cool_tint,
        appearance.warm_tint,
    ]; // 八组向量必须逐项有限。
    if appearance_values
        .iter()
        .flatten()
        .any(|value| !value.is_finite())
        || base_color_texture_path.is_some_and(|path| !safe_asset_path(path))
        || !finite_slice(&[
            bounds.max_blade_height,
            bounds.max_horizontal_sweep,
            bounds.reference_height,
            bounds.root_penetration,
            density_gain,
            ultra_horizontal_sweep,
        ])
        || bounds.max_blade_height <= 0.0
        || bounds.max_horizontal_sweep < 0.0
        || bounds.reference_height <= 0.0
        || bounds.root_penetration < 0.0
        || density_gain < 0.0
        || ultra_horizontal_sweep < 0.0
        || !validate_motion(motion)
    {
        return Err(format!(
            "植被预设 {} 的 Blade 外观、运动或包围无效",
            preset.id
        )); // GPU 不得接收 NaN、负半径或坏路径。
    }
    Ok(()) // 数据 Blade 可安全转成 N 窗与 GPU profile。
}

fn validate_view_lod(view: &BladePresetViewLod, lod_count: usize) -> bool {
    if lod_count < MIN_BLADE_PRESET_LODS
        || lod_count > MAX_BLADE_PRESET_LODS
        || view.pixel_thresholds.len() + 1 != lod_count
        || view.residency_radii.len() != lod_count
        || view.residency_transition_widths.len() + 1 != lod_count
    {
        return false; // 相邻过渡数组长度必须恰好等于档数减一。
    }
    view.pixel_thresholds
        .iter()
        .all(|pair| finite_slice(pair) && pair[0] >= 0.0 && pair[1] >= pair[0])
        && finite_slice(&view.camera_fade_distances)
        && view.camera_fade_distances[0] >= 0.0
        && view.camera_fade_distances[1] >= view.camera_fade_distances[0]
        && finite_slice(&view.residency_radii)
        && view.residency_radii.iter().all(|value| *value > 0.0)
        && finite_slice(&view.residency_transition_widths)
        && view
            .residency_transition_widths
            .iter()
            .all(|value| *value >= 0.0)
        && view.reference_grass_height.is_finite()
        && view.reference_grass_height > 0.0
        && view.finest_grid_spacing.is_finite()
        && view.finest_grid_spacing > 0.0
        && view.residency_margin.is_finite()
        && view.residency_margin >= 0.0 // 只校验数学有效性，视觉阈值由内容验收负责。
}

fn validate_motion(motion: &BladePresetMotion) -> bool {
    match motion {
        BladePresetMotion::Wind => true, // 世界风参数由运行时公共资源继续校验。
        BladePresetMotion::WaterFlow {
            direction,
            speed,
            tip_strength,
            spatial_scale,
        } => {
            finite_slice(direction)
                && direction[0].hypot(direction[1]) > 0.000_001
                && speed.is_finite()
                && *speed >= 0.0
                && tip_strength.is_finite()
                && *tip_strength >= 0.0
                && spatial_scale.is_finite()
                && *spatial_scale > 0.0 // 水草明确由流向驱动，不接受零方向或负摆幅。
        }
    }
}

fn validate_mesh(
    preset: &VegetationPresetDescriptor,
    lods: &[MeshPresetLod],
    ground_embed_depth: f32,
    collision: &VegetationPresetCollision,
) -> Result<(), String> {
    if lods.is_empty() || !ground_embed_depth.is_finite() || ground_embed_depth < 0.0 {
        return Err(format!("植被预设 {} 的 Mesh LOD 或贴地深度无效", preset.id)); // 通用原型至少提供一档真实几何。
    }
    let mut levels = BTreeSet::new(); // 档位身份允许任意数量但不得重复。
    for lod in lods {
        if !levels.insert(lod.level)
            || lod.parts.is_empty()
            || !finite_slice(&lod.start)
            || !finite_slice(&lod.end)
            || lod.start[0] < 0.0
            || lod.start[1] < lod.start[0]
            || lod.end[0] < lod.start[1]
            || lod.end[1] < lod.end[0]
        {
            return Err(format!("植被预设 {} 包含无效 Mesh LOD 距离", preset.id)); // 宽域 LOD 仍需单调可解释。
        }
        let mut part_ids = BTreeSet::new(); // 同档部件技术身份必须唯一。
        for part in &lod.parts {
            if !part_ids.insert(part.part_id)
                || !safe_asset_path(&part.mesh_path)
                || !safe_asset_path(&part.material_path)
                || !validate_mesh_derivation(&part.mesh_derivation)
                || !validate_material_overrides(&part.material_overrides)
                || !finite_slice(&part.transform.translation)
                || !part.transform.yaw_degrees.is_finite()
                || !finite_slice(&part.transform.scale)
                || part.transform.scale.contains(&0.0)
                || part.wind.as_ref().is_some_and(|wind| {
                    !finite_slice(&[
                        wind.root_height,
                        wind.tip_height,
                        wind.strength,
                        wind.frequency,
                    ]) || wind.tip_height <= wind.root_height
                        || wind.strength < 0.0
                        || wind.frequency < 0.0
                })
            {
                return Err(format!("植被预设 {} 包含无效 Mesh 部件", preset.id)); // 资产路径和全部变换必须可安全加载。
            }
        }
    }
    if !validate_collision(collision) {
        return Err(format!("植被预设 {} 的碰撞规格无效", preset.id)); // 碰撞只由显式预设参数决定。
    }
    Ok(()) // 数据 Mesh 可进入异步资产加载和通用原型登记。
}

/// 校验运行时派生网格的有限工作预算；数量不设产品上限，但零输入没有几何意义。
fn validate_mesh_derivation(derivation: &MeshPresetMeshDerivation) -> bool {
    match derivation {
        MeshPresetMeshDerivation::Direct => true, // 直接源网格不需要额外参数。
        MeshPresetMeshDerivation::DispersedTriangles { max_triangles } => *max_triangles > 0, // 薄片代理至少保留一个三角形。
        MeshPresetMeshDerivation::ConvexHull { max_sample_points } => *max_sample_points >= 4, // 三维封闭凸包至少需要四点。
    }
}

/// 校验材质覆盖不会向 GPU 提交非法 Alpha 阈值。
fn validate_material_overrides(overrides: &MeshPresetMaterialOverrides) -> bool {
    match overrides.alpha_mode {
        None | Some(MeshPresetAlphaMode::Opaque | MeshPresetAlphaMode::Blend) => true, // 无阈值模式始终有效。
        Some(MeshPresetAlphaMode::Mask { cutoff }) => {
            cutoff.is_finite() && (0.0..=1.0).contains(&cutoff)
        } // Alpha Mask 阈值限定为标准归一化范围。
    }
}

fn validate_collision(collision: &VegetationPresetCollision) -> bool {
    match collision {
        VegetationPresetCollision::None => true, // 无碰撞是草、花和软灌木的合法策略。
        VegetationPresetCollision::Cylinder {
            radius,
            height,
            center_y,
            enter_distance,
            exit_distance,
            ..
        } => {
            finite_slice(&[*radius, *height, *center_y, *enter_distance, *exit_distance])
                && *radius > 0.0
                && *height > 0.0
                && *enter_distance > 0.0
                && *exit_distance >= *enter_distance // 形状必须有正体积，常驻滞回不得反向。
        }
        VegetationPresetCollision::Cuboid {
            size,
            center,
            enter_distance,
            exit_distance,
            ..
        } => {
            finite_slice(size)
                && finite_slice(center)
                && finite_slice(&[*enter_distance, *exit_distance])
                && size.iter().all(|value| *value > 0.0)
                && *enter_distance > 0.0
                && *exit_distance >= *enter_distance // 长方体三轴与常驻滞回都必须有效。
        }
    }
}

fn safe_stable_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) // 稳定 ID 不含路径、空格或本地化字符。
}

fn safe_asset_path(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('/') && !value.contains("..") && !value.contains('\\') // 所有预设资产必须位于当前 assets 根。
}

fn finite_slice(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite()) // 集中执行 GPU/物理纯值有限性检查。
}
