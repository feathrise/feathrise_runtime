//! 在 Solari 建立管线前登记地形射线、样本复用及环境光兼容规则。

use std::path::{Path, PathBuf};
use bevy::{
    asset::{AssetServer, io::embedded::EmbeddedAssetRegistry},
    prelude::App,
    shader::Shader,
};

const SOLARI_REALTIME_PATH: &str = "bevy_solari/realtime";

#[cfg(test)]
#[path = "shadow_terminator_tests.rs"]
mod projection_tests;

pub(crate) fn install_solari_realtime_shaders(app: &mut App) {
    // 官方库在插件注册时已发起异步读取。项目库使用独立模块名，计算入口在 RenderStartup 读取前覆盖。
    let registry = app.world().resource::<EmbeddedAssetRegistry>();
    let assets = app.world().resource::<AssetServer>();
    for (name, source, bytes, library) in [
        (
            "feathrise_gbuffer.wesl",
            "gbuffer_shadow_terminator.wesl",
            &include_bytes!("shaders/gbuffer_shadow_terminator.wesl")[..],
            true,
        ),
        (
            "feathrise_initial_path.wesl",
            "initial_path_stability.wesl",
            &include_bytes!("shaders/initial_path_stability.wesl")[..],
            true,
        ),
        (
            "restir.wesl",
            "restir_stability.wesl",
            &include_bytes!("shaders/restir_stability.wesl")[..],
            false,
        ),
        (
            "no_restir.wesl",
            "no_restir_stability.wesl",
            &include_bytes!("shaders/no_restir_stability.wesl")[..],
            false,
        ),
        (
            "world_cache_update.wesl",
            "world_cache_update_no_environment.wesl",
            &include_bytes!("shaders/world_cache_update_no_environment.wesl")[..],
            false,
        ),
    ] {
        let disk_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/denoising/shaders")
            .join(source);
        let embedded_path = Path::new(SOLARI_REALTIME_PATH).join(name);
        registry.insert_asset(disk_path, &embedded_path, bytes);
        if library {
            // 同 Bevy 的 load_shader_library!，固定全局库的资产生命周期。
            let handle = assets.load::<Shader>(format!("embedded://{SOLARI_REALTIME_PATH}/{name}"));
            std::mem::forget(handle);
        }
    }
}

#[cfg(test)]
mod shader_contract_tests {
    const RESTIR: &str = include_str!("shaders/restir_stability.wesl");
    const INITIAL: &str = include_str!("shaders/initial_path_stability.wesl");
    const GBUFFER: &str = include_str!("shaders/gbuffer_shadow_terminator.wesl");

    #[test]
    fn indirect_history_and_direct_neighbors_are_disabled_before_sampling() {
        let temporal = RESTIR.split("fn load_temporal_reservoir").nth(1).unwrap();
        assert!(temporal.find("is_indirect_reservoir(canonical)").unwrap() < temporal.find("load_motion_vector(pixel_id)").unwrap());
        assert!(temporal.contains("is_indirect_reservoir(temporal.reservoir)"));
        let spatial = RESTIR.split("fn load_spatial_reservoir").nth(1).unwrap();
        assert!(spatial.find("!is_indirect_reservoir(canonical)").unwrap() < spatial.find("get_neighbor_pixel_id").unwrap());
        assert!(spatial.find("surface.gpu_vegetation != spatial_surface.gpu_vegetation").unwrap() < spatial.find("reservoirs_b[spatial_pixel_index]").unwrap());
        assert!(INITIAL.contains("LightSample(NULL_LIGHT_ID, FEATHRISE_INDIRECT_RESERVOIR_SEED)"));
    }

    #[test]
    fn terrain_keeps_standard_packed_normal_and_lazy_geometry() {
        assert!(GBUFFER.contains("octahedral_decode(unpack_24bit_normal(gpixel.a))"));
        assert!(GBUFFER.contains("DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK: u32 = 0x7FFFFu"));
        assert!(GBUFFER.find("if surface.dynamic_terrain == 0u").unwrap() < GBUFFER.find("load_vertices(instance_geometry, surface.primitive_index)").unwrap());
        assert!(INITIAL.contains("path.ray_origin = ray_surface.ray_origin"));
        assert!(INITIAL.contains("evaluate_diffuse_brdf(wo, wi, n, material, f_ab)"));
    }

    #[test]
    fn realtime_paths_leave_sky_lighting_to_the_separate_ibl_pass() {
        let cache = include_str!("shaders/world_cache_update_no_environment.wesl");
        assert!(!INITIAL.contains("sample_environment_map_light"));
        assert!(!cache.contains("sample_environment_map_light"));
        assert!(INITIAL.contains("psr_environment_miss(pixel_id, wo)"));
        assert!(cache.contains("query_world_cache(ray_hit.world_position"));
    }
}
