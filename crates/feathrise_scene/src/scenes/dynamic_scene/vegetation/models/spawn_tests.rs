//! 模型植被使用与高度表面一致的完整资产/顶点ABI门；坏LOD不能发布半份窗口或改写其他网格。
use super::*;
use bevy::ecs::system::SystemState;
use feathrise_scene_content::{
    ResolvedVegetationPresetCatalog, SceneId, SceneInstancePath, VegetationPresetCatalogAsset,
};
use std::collections::VecDeque;

fn test_identity() -> Identity {
    let scene = SceneId("MAP-model-profile-gate".into());
    Identity { path: SceneInstancePath::root(scene.clone()), scene, instance_generation: 0,
        document: 1, draft: None, document_epoch: 0, geometry_epoch: 0,
        placement_revision: 1, index_revision: 1, presets: 1 }
}

#[test]
fn all_model_blade_lods_pass_the_shared_vertex_gate_before_missing_uv_is_repaired() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_resource::<GpuBladeProfileBanks>()
        .init_resource::<VegetationMeshPrototypeRegistry>()
        .init_resource::<DynamicMeshProfileState>()
        .init_resource::<DynamicMeshProfileRequests>()
        .init_resource::<DynamicMeshProfileCache>();
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/vegetation_presets.json"
    )))
    .unwrap();
    let presets = VegetationPresetRegistry {
        catalog: Handle::default(),
        revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(),
        last_error: None,
    };
    let mut blade = load_blade_preset(app.world().resource::<AssetServer>(), &catalog.presets[0])
        .unwrap();
    blade.base_color_texture = None;
    blade.base_color_texture_path = None;
    // 使用已登记但暂未可见的独立句柄复现真实资产管线交接；不安装会自动补齐资源的测试加载器。
    let handles: Vec<_> = {
        let mut meshes = app.world_mut().resource_mut::<bevy::asset::Assets<Mesh>>();
        (0..blade.meshes.len())
            .map(|_| {
                let handle = meshes.add(Mesh::from(Rectangle::default()));
                meshes.remove(handle.id()).unwrap();
                handle
            })
            .collect()
    };
    assert!(handles.len() >= 2);
    blade.meshes = handles.clone();
    let checks = vec![ProfileCheck::Blade(blade.spec.id.clone())].into();
    let blades = [(blade.spec.id.clone(), blade)].into();
    let (banks, bindings) = super::super::super::helpers::pack_blade_profile_banks(&blades).unwrap();
    let scene = SceneId("MAP-model-profile-gate".into());
    let mesh_checks = handles.iter().enumerate().map(|(lod, handle)|
        (handle.clone(), format!("测试叶片 LOD{lod}"))).collect();
    let mut waiting = Waiting {
        identity: Identity {
            path: SceneInstancePath::root(scene.clone()),
            scene,
            instance_generation: 0,
            document: 1,
            draft: None,
            document_epoch: 0,
            geometry_epoch: 0,
            placement_revision: 1,
            index_revision: 1,
            presets: 1,
        },
        build: Build { objects: VecDeque::new(), warnings: Vec::new(), profiles: VecDeque::new(), banks: Vec::new() },
        blades,
        banks,
        bindings,
        staging: None,
        retained: BTreeMap::new(),
        blade_count: 0,
        mesh_count: 0,
        prepared: false,
        checks,
        mesh_checks,
        missing_uv: VecDeque::new(),
        seen_meshes: BTreeSet::new(),
    };
    let mut access = SystemState::<Assets>::new(app.world_mut());
    assert!(!ready(&mut access.get_mut(app.world_mut()).unwrap(), &mut waiting, &presets).unwrap());
    let valid = Mesh::from(Rectangle::default());
    {
        let mut meshes = app.world_mut().resource_mut::<bevy::asset::Assets<Mesh>>();
        for handle in &handles {
            meshes.insert(handle.id(), valid.clone()).unwrap();
        }
        meshes.get_mut(&handles[0]).unwrap().remove_attribute(Mesh::ATTRIBUTE_UV_0);
        meshes.get_mut(handles.last().unwrap()).unwrap().remove_attribute(Mesh::ATTRIBUTE_NORMAL);
    }
    let error = ready(&mut access.get_mut(app.world_mut()).unwrap(), &mut waiting, &presets).unwrap_err();
    assert!(error.contains("顶点布局无效"));
    assert!(app.world().resource::<bevy::asset::Assets<Mesh>>().get(&handles[0]).unwrap()
        .attribute(Mesh::ATTRIBUTE_UV_0).is_none(), "坏LOD存在时不得先修改另一份网格");
    app.world_mut().resource_mut::<bevy::asset::Assets<Mesh>>()
        .insert(handles.last().unwrap().id(), valid).unwrap();
    assert!(ready(&mut access.get_mut(app.world_mut()).unwrap(), &mut waiting, &presets).unwrap());
    assert!(app.world().resource::<bevy::asset::Assets<Mesh>>().get(&handles[0]).unwrap()
        .attribute(Mesh::ATTRIBUTE_UV_0).is_some());
    assert!(app.world().resource::<GpuBladeProfileBanks>().is_empty(),
        "准备门不能提前登记可见批次");
}

#[test]
fn model_profile_readiness_drains_a_fixed_prefix_and_keeps_passed_dependencies_cached() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
        .init_asset::<Mesh>().init_asset::<Image>()
        .init_resource::<GpuBladeProfileBanks>()
        .init_resource::<VegetationMeshPrototypeRegistry>()
        .init_resource::<DynamicMeshProfileState>()
        .init_resource::<DynamicMeshProfileRequests>()
        .init_resource::<DynamicMeshProfileCache>();
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/vegetation_presets.json"))).unwrap();
    let presets = VegetationPresetRegistry { catalog: Handle::default(), revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(), last_error: None };
    let mut blade = load_blade_preset(app.world().resource::<AssetServer>(), &catalog.presets[0]).unwrap();
    blade.base_color_texture = None;
    let handle = app.world_mut().resource_mut::<bevy::asset::Assets<Mesh>>()
        .add(Mesh::from(Rectangle::default()));
    blade.meshes = vec![handle.clone()]; blade.spec.lods.truncate(1);
    let mut waiting = prepare(test_identity(), Build { objects: VecDeque::new(), warnings: Vec::new(),
        profiles: VecDeque::new(), banks: Vec::new() });
    for index in 0..PROFILE_STEPS_PER_FRAME * 2 + 1 {
        let id = format!("profile-{index:03}");
        let mut profile = blade.clone(); profile.spec.id = id.clone();
        waiting.blades.insert(id.clone(), profile);
        waiting.checks.push_back(ProfileCheck::Blade(id));
    }
    waiting.mesh_checks.push_back((handle, "共享叶片".into()));
    let mut access = SystemState::<Assets>::new(app.world_mut());
    assert!(!ready(&mut access.get_mut(app.world_mut()).unwrap(), &mut waiting, &presets).unwrap());
    assert_eq!(waiting.checks.len(), PROFILE_STEPS_PER_FRAME + 1);
    assert!(!ready(&mut access.get_mut(app.world_mut()).unwrap(), &mut waiting, &presets).unwrap());
    assert_eq!(waiting.checks.len(), 1);
    assert!(ready(&mut access.get_mut(app.world_mut()).unwrap(), &mut waiting, &presets).unwrap());
    assert!(waiting.checks.is_empty() && waiting.mesh_checks.is_empty());
    assert!(app.world().resource::<GpuBladeProfileBanks>().is_empty(), "资源就绪不授予提前发布权");
}
