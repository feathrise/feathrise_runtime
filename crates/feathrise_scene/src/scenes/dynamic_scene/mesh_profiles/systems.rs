//! 动态场景正式Mesh profile的按需串行加载、登记、风更新与离场释放。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::{diagnostic::FrameCount, prelude::*};
use feathrise_environment::mesh_wind::{
    TreeWindMaterial,
    TreeWindNoiseTexture,
    update_tree_wind_material_with_phase, // 目录Mesh沿用同一相位补偿。
};
use feathrise_environment::vegetation::VegetationMeshBillboardMaterial;
use feathrise_environment::vegetation::blade::{
    GPU_GRASS_MAX_WIND_TIP_STRENGTH, GpuBladeInteraction,
};
use feathrise_environment::vegetation::{VegetationMeshPrototypeRegistry, VegetationPrototypeId};
use feathrise_scene_content::{VegetationPresetRegistry, VegetationPresetRuntime};

use super::resources::{
    DynamicMeshProfilePhase, DynamicMeshProfileRequests, DynamicMeshProfileState,
    DynamicMeshProfileStatus, DynamicMeshProfileCache, DynamicMeshProfileKey,
};

/// 非Mesh请求保留明确失败回执；它不能阻止其他合法预设进入各自的共享加载槽。
fn validate_requested_profiles(
    requests: &DynamicMeshProfileRequests,
    presets: &VegetationPresetRegistry,
) -> Result<(), String> {
    for profile in requests.iter() {
        let preset = presets
            .resolve(profile)
            .ok_or_else(|| format!("开发者植被预设{profile}不存在"))?;
        if !matches!(&preset.runtime, VegetationPresetRuntime::Mesh { .. }) {
            return Err(format!("植被预设{}不是Mesh后端", preset.id));
        }
    }
    Ok(()) // 数据协议只有Blade与Mesh，不再分派任何注册适配器。
}

pub(in crate::scenes::dynamic_scene) fn begin_dynamic_mesh_profiles(
    mut requests: InstanceResMut<DynamicMeshProfileRequests>,
    mut state: InstanceResMut<DynamicMeshProfileState>,
    mut status: InstanceResMut<DynamicMeshProfileStatus>,
) {
    requests.clear();
    *state = DynamicMeshProfileState::default();
    *status = DynamicMeshProfileStatus {
        detail: "尚未请求数据Mesh预设".to_owned(),
        ..default()
    };
}

#[allow(clippy::too_many_arguments)]
pub(in crate::scenes::dynamic_scene) fn drive_dynamic_mesh_profiles(
    asset_server: Res<AssetServer>,
    frame: Res<FrameCount>,
    requests: InstanceRes<DynamicMeshProfileRequests>,
    presets: Option<Res<VegetationPresetRegistry>>,
    mut state: InstanceResMut<DynamicMeshProfileState>,
    mut status: InstanceResMut<DynamicMeshProfileStatus>,
    mut cache: ResMut<DynamicMeshProfileCache>,
    mut registry: ResMut<VegetationMeshPrototypeRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut wind_materials: ResMut<Assets<TreeWindMaterial>>,
    mut billboard_materials: ResMut<Assets<VegetationMeshBillboardMaterial>>,
    noise: Res<TreeWindNoiseTexture>,
) {
    status.requested_profiles = requests.requested_count();
    let Some(presets) = presets.as_deref() else {
        if requests.requested_count() > 0 {
            status.phase = DynamicMeshProfilePhase::Loading;
            status.detail = "正在等待开发者植被预设目录".to_owned();
        }
        return;
    };
    // 根图没有请求也须轮询退役网格的真实加载，全场景始终只占一个槽。
    cache.advance(frame.0, &asset_server, presets, &mut registry, &mut meshes,
        &mut standard_materials, &mut wind_materials, &mut billboard_materials, &noise);
    if requests.requested_count() == 0 { return; }
    let mut failure = validate_requested_profiles(&requests, presets).err();
    // 只由显式重试清共享错误。稳定失败读同一回执，目录换代或兄弟成功后仍须重新读取ready。
    let retry = state.failed_request_revision.is_some_and(|failed| failed != requests.revision());
    state.failed_request_revision = None;
    for profile in requests.iter() {
        let Some(preset) = presets.resolve(profile) else { continue; };
        if !matches!(&preset.runtime, VegetationPresetRuntime::Mesh { .. }) { continue; }
        let key = DynamicMeshProfileKey { catalog_revision: presets.revision,
            preset_id: preset.id.clone() };
        if state.leases.get(&preset.id).is_none_or(|lease| lease.key != key) {
            let lease = cache.lease(key.clone(), VegetationPrototypeId(
                preset.resolved_prototype_id().expect("Mesh预设始终拥有原型ID")));
            state.leases.insert(preset.id.clone(), lease);
        }
        let entry = cache.entries.get_mut(&key).expect("本实例租约已取得缓存条目");
        if retry { entry.failure = None; } // 显式重试沿用同一来源键，不重复加载同一份GLTF。
        if let Some(error) = &entry.failure {
            if failure.is_none() { failure = Some(error.clone()); }
        }
        if entry.ready(&registry) {
            state.available_profiles.insert(preset.id.clone());
            state.generic_catalog_revisions.insert(preset.id.clone(), presets.revision);
        } else {
            state.available_profiles.remove(&preset.id);
            state.generic_catalog_revisions.remove(&preset.id);
        }
    }
    status.ready_profiles = state.available_profiles.len();
    if let Some(error) = failure {
        state.failed_request_revision = Some(requests.revision());
        status.phase = DynamicMeshProfilePhase::Failed;
        status.detail = error;
        return; // 所有合法profile已登记租约；无关失败不会饿死本次场景实际必需的profile。
    }
    if status.ready_profiles == status.requested_profiles {
        status.phase = DynamicMeshProfilePhase::Ready;
        status.detail = format!("{}个正式Mesh预设已登记：任意LOD、部件、风材质与贴地参数可用",
            status.ready_profiles);
    } else {
        status.phase = DynamicMeshProfilePhase::Loading;
        status.detail = cache.current_generic.as_ref().map_or_else(
            || "正在等待共享Mesh预设加载槽".to_owned(),
            |load| format!("正在按需加载通用Mesh预设{}", load.preset_id));
    }
}

/// 父世界统一更新一次，所有网格共享同一风绑定，不重复更新也不漏独立网格。
pub(in crate::scenes::dynamic_scene) fn update_dynamic_mesh_profile_wind(
    interaction: Res<GpuBladeInteraction>,
    cache: Res<DynamicMeshProfileCache>,
    mut materials: ResMut<Assets<TreeWindMaterial>>,
) {
    let strength = (interaction.wind_tip_strength() / GPU_GRASS_MAX_WIND_TIP_STRENGTH).clamp(0.0, 1.0);
    for (_, built) in cache.entries.values().filter_map(|entry| entry.loaded.as_ref()) {
        for binding in &built.wind {
            if let Some(mut material) = materials.get_mut(&binding.handle) {
                update_tree_wind_material_with_phase(&mut material, binding.lod, binding.part,
                    interaction.wind_direction(), strength * binding.strength_scale,
                    interaction.wind_speed() * binding.frequency_scale,
                    interaction.wind_phase_offset() * binding.frequency_scale);
            }
        }
    }
}

/// 最后实例租约退出后只回收自己的登记版本；渲染强句柄继续安全保活。
pub(in crate::scenes::dynamic_scene) fn collect_dynamic_mesh_profiles(
    server: Res<AssetServer>,
    mut cache: ResMut<DynamicMeshProfileCache>,
    mut registry: ResMut<VegetationMeshPrototypeRegistry>,
) {
    cache.reap_abandoned_load(&server);
    cache.collect_one(&mut registry);
}

pub(in crate::scenes::dynamic_scene) fn clear_dynamic_mesh_profiles(
    mut requests: InstanceResMut<DynamicMeshProfileRequests>,
    mut state: InstanceResMut<DynamicMeshProfileState>,
    mut status: InstanceResMut<DynamicMeshProfileStatus>,
) {
    requests.clear();
    *state = DynamicMeshProfileState::default(); // 只归还本scope租约，兄弟和驻留实例仍持有同一原型。
    *status = DynamicMeshProfileStatus {
        detail: "已离开动态场景并归还正式Mesh预设租约".to_owned(),
        ..default()
    };
}
