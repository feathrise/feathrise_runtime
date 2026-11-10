//! 三维种植作者格接入正式GPU叶片：后台冻结格点，GPU负责候选、LOD与法线定向。
use bevy::{
    camera::primitives::Aabb,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_environment::vegetation::{
    VegetationBladeLayer,
    blade::{GpuBladePatch, GpuBladeProfileBankId, GpuBladeVolumeSource, GpuBladeProfileBanks, GpuBladeProfileBankAllocation},
};
use feathrise_scene_core::residency::{InstanceQuery, InstanceRes, InstanceResMut, SceneInstanceContextAppExt};
use std::sync::Arc;
mod boundary; // 两种草窗共享完整密度源和已采用地形，按真实根位置交接。
mod clearance;
mod collision;
mod source;
use super::super::{DynamicClipmapGroundSurface, VolumeSurfaceFields};
use source::Layer;

#[derive(Component)]
pub(super) struct VolumeVegetationLayer(pub String);
#[derive(Component)]
struct VolumeWindow;
#[derive(Component)]
struct VolumeVegetationRoot;
#[derive(Clone, PartialEq)]
struct Identity {
    roots: Vec<(Entity, usize)>,
    support: Vec<(([u8; 32], [u8; 32]), [u32; 3], u32)>,
    ground: [u64; 7],
    vegetation: Option<Entity>,
    prototypes: u64,
    presets: u64,
    clearances: u64,
    blade_spans: Vec<(String, u32)>, // 承托晕边随正式LOD母格宽度换代，不能复用过小的旧窗口。
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 运行真实采用系统；不创建渲染器或后台任务，验证完整旧集合的暂停与恢复。
    fn active_volume(
        visibility: Visibility,
        valid: bool,
    ) -> (App, Entity, crate::ClipmapGroundSnapshot) {
        let mut app = App::new();
        app.init_resource::<super::super::DynamicVegetationState>()
            .init_resource::<Assets<Image>>()
            .init_resource::<feathrise_environment::vegetation::VegetationMeshPrototypeRegistry>()
            .init_resource::<super::super::super::mesh_profiles::DynamicMeshProfileRequests>()
            .init_resource::<super::super::super::mesh_profiles::DynamicMeshProfileState>()
            .add_systems(Update, drive);
        let ground = crate::ClipmapGroundSnapshot::stacked_for_test();
        let root = app
            .world_mut()
            .spawn((VolumeVegetationRoot, visibility))
            .id();
        let prototypes = app
            .world()
            .resource::<feathrise_environment::vegetation::VegetationMeshPrototypeRegistry>()
            .generation();
        app.insert_resource(State {
            active: Some(Identity {
                roots: Vec::new(),
                support: Vec::new(),
                ground: ground.content_revision_words(),
                vegetation: None,
                prototypes,
                presets: 0,
                clearances: 0,
                blade_spans: Vec::new(),
            }),
            root: Some(root),
            valid,
            ..default()
        });
        let mut surface = DynamicClipmapGroundSurface::default();
        surface.snapshot = Some(ground.clone());
        app.insert_resource(surface);
        (app, root, ground)
    }

    #[test]
    fn volume_windows_share_the_outdoor_focus_instead_of_the_orbit_camera() {
        let (mut app, root, _) = active_volume(Visibility::Inherited, true);
        let player = Vec3::new(7.0, 2.0, -5.0);
        let mut prototype = GpuBladePatch::square(Vec3::ZERO, 16., 257, 17);
        prototype.set_focus(player);
        app.world_mut().spawn((
            VolumeVegetationLayer("grass".into()),
            Mesh3d::default(),
            prototype,
            GpuBladeProfileBankId::default(),
            VegetationBladeLayer(feathrise_environment::vegetation::VegetationLayerId(1)),
        ));
        let window = app
            .world_mut()
            .spawn((
                VolumeWindow,
                VolumeVegetationLayer("grass".into()),
                prototype.with_volume_cell_source(1, 0.25),
                ChildOf(root),
            ))
            .id();
        app.world_mut().spawn((
            feathrise_environment::vegetation::blade::GpuBladeView::default(),
            Transform::from_xyz(7.0, 50.0, -5.0),
        ));
        app.world_mut()
            .resource_mut::<State>()
            .active
            .as_mut()
            .unwrap()
            .blade_spans = vec![("grass".into(), 0.25f32.to_bits())];
        app.update();
        assert_eq!(
            app.world()
                .get::<GpuBladePatch>(window)
                .unwrap()
                .focus_radius_xyz(),
            player
        );
    }

    #[test]
    fn missing_ground_disables_volume_render_and_collision_ownership() {
        let (mut app, root, mut ground) = active_volume(Visibility::Inherited, true);
        ground.ready = false;
        app.world_mut()
            .resource_mut::<DynamicClipmapGroundSurface>()
            .snapshot = Some(ground);
        app.update();
        assert!(
            !app.world().resource::<State>().valid,
            "地形等待期间不能继续授权旧近场碰撞"
        );
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Hidden)
        );
        app.world_mut()
            .resource_mut::<DynamicClipmapGroundSurface>()
            .snapshot = None;
        app.update();
        assert!(!app.world().resource::<State>().valid);
    }

    #[test]
    fn restored_active_ground_reveals_the_complete_retained_volume_root() {
        let (mut app, root, _) = active_volume(Visibility::Hidden, false);
        app.update();
        assert!(app.world().resource::<State>().valid);
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Inherited)
        );
        assert!(
            app.world().resource::<State>().task.is_none(),
            "同一完整版本恢复不应重新散布"
        );
    }

    #[test]
    fn field_refresh_and_display_lod_keep_the_supported_old_grass_visible() {
        let (mut app, root, ground) = active_volume(Visibility::Inherited, true);
        let fields = VolumeSurfaceFields::default();
        let support = (
            fields.0.geometry_identity(),
            [0; 3],
            fields.0.cell_size.to_bits(),
        );
        let entity = app
            .world_mut()
            .spawn((fields.clone(), Transform::IDENTITY, Visibility::Inherited))
            .id();
        let mut current = app.world().resource::<State>().active.clone().unwrap();
        current.roots = vec![(entity, Arc::as_ptr(&fields.0) as usize)];
        current.support = vec![support];
        let mut old = current.clone();
        old.roots[0].1 ^= 1; // 新作者快照使用另一个Arc，承托根保持同一版本。
        old.ground[6] ^= 1; // 同一密度根的显示LOD也会发布新的拓扑修订。
        old.vegetation = Some(Entity::PLACEHOLDER); // R8草稿重新发布外部根也不能先清空洞内草。
        {
            let mut state = app.world_mut().resource_mut::<State>();
            state.active = Some(old);
            state.failed = Some(current); // 固定失败托底，避免本测试启动后台散布。
        }
        app.update();
        assert!(app.world().resource::<State>().valid);
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Inherited)
        );
        let mut changed = app.world().resource::<State>().active.clone().unwrap();
        changed.support[0].0.0[0] = 1; // 真正雕刻换代必须撤销旧承托，不可借保留显示漏授权碰撞。
        app.world_mut().resource_mut::<State>().active = Some(changed);
        app.update();
        assert!(!app.world().resource::<State>().valid);
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Hidden)
        );
        assert!(ground.ready);
    }

    #[test]
    fn complete_blade_refresh_reuses_the_existing_root_and_gpu_window_entities() {
        let (mut app, root, _) = active_volume(Visibility::Inherited, true);
        app.init_resource::<GpuBladeProfileBanks>();
        let bank = feathrise_environment::vegetation::blade::GpuBladeProfileBank::new(
            feathrise_environment::vegetation::blade::GpuBladeProfiles::default(), None);
        let allocation = app.world_mut().resource_mut::<GpuBladeProfileBanks>().allocate(vec![bank.clone()]).unwrap();
        let id = allocation.bank(0).unwrap();
        let outdoor = app.world_mut().spawn(allocation).id();
        let patch = GpuBladePatch::square(Vec3::ZERO, 16., 257, 17);
        app.world_mut().spawn((
            VolumeVegetationLayer("grass".into()),
            Mesh3d::default(),
            patch,
            id,
            ChildOf(outdoor),
            VegetationBladeLayer(feathrise_environment::vegetation::VegetationLayerId(1)),
        ));
        let window = app
            .world_mut()
            .spawn((
                VolumeWindow,
                VolumeVegetationLayer("grass".into()),
                patch.with_volume_cell_source(1, 0.25),
                ChildOf(root),
            ))
            .id();
        let cells: Arc<[[[f32; 4]; 3]]> =
            Arc::from(vec![
                [[0., 2., 0., 1.], [0., 1., 0., 0.25], [0., 2., 0., 1.]];
                2
            ]);
        let mut identity = app.world().resource::<State>().active.clone().unwrap();
        identity.blade_spans = vec![("grass".into(), 0.25f32.to_bits())];
        {
            let mut state = app.world_mut().resource_mut::<State>();
            state.blade_only = true;
            state.waiting = Some((
                identity,
                vec![Layer {
                    id: "grass".into(),
                    image: Image::default(),
                    cells,
                    count: 2,
                    step: 0.25,
                    min: Vec3::ZERO,
                    max: Vec3::splat(2.),
                }],
            ));
        }
        app.update();
        assert_eq!(app.world().resource::<State>().root, Some(root));
        assert_eq!(
            app.world()
                .get::<GpuBladePatch>(window)
                .unwrap()
                .instance_count(),
            patch.with_volume_cell_source(2, 0.25).instance_count()
        );
        assert!(app.world().get::<GpuBladeVolumeSource>(window).is_some());
        assert_eq!(app.world().get::<GpuBladeProfileBankId>(window), Some(&id));
        assert!(app.world().get::<GpuBladeProfileBankAllocation>(root).is_some());
        app.world_mut().despawn(outdoor); // 实际模板根退役后，三维草窗仍持有同一绘制槽。
        assert_eq!(app.world().resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1], bank);
        app.world_mut().despawn(root);
        assert!(app.world().resource::<GpuBladeProfileBanks>().is_empty());
    }
}
impl Identity {
    /// 作者种植与显示LOD更新时保留完整旧集合；雕刻、移位、净空或预设换代才失去承托资格。
    fn same_support(&self, other: &Self) -> bool {
        self.support == other.support
            && self.ground[..6] == other.ground[..6]
            && self.prototypes == other.prototypes
            && self.presets == other.presets
            && self.clearances == other.clearances
    }
    fn same_source(&self, other: &Self) -> bool {
        self.roots == other.roots
            && self.ground == other.ground
            && self.vegetation == other.vegetation
            && self.presets == other.presets
            && self.clearances == other.clearances
            && self.blade_spans == other.blade_spans
    }
}
#[derive(Resource, Default)]
struct State {
    active: Option<Identity>,
    building: Option<Identity>,
    failed: Option<Identity>,
    task: Option<Task<Result<Vec<Layer>, String>>>,
    waiting: Option<(Identity, Vec<Layer>)>,
    root: Option<Entity>,
    blade_sources: std::collections::BTreeMap<String, Handle<Image>>, // 完整采用后同时给室外/三维窗口绑定。
    boundary_revision: u64, // 仅完整源采用时推进；同步系统只在身份/输入换代时更新绑定。
    collision_sources: Arc<Vec<collision::Source>>,
    valid: bool,
    blade_only: bool,
}

pub(super) fn install(app: &mut App) {
    collision::install(app);
    app.init_instance_resource::<State>()
        .add_systems(
            Update,
            (drive, boundary::sync_inputs)
                .chain()
                .after(super::super::TerrainTopologySystems::Publish)
                .after(super::super::ClipmapGroundPublishSet)
                .after(super::drive_dynamic_blade_clearance)
                .run_if(in_state(feathrise_scene_core::state::ScenePhase::Active)),
        )
        .add_systems(OnExit(feathrise_scene_core::state::ScenePhase::Active), reset);
}
fn reset(mut state: InstanceResMut<State>) {
    let old = std::mem::take(&mut *state);
    AsyncComputeTaskPool::get()
        .spawn(async move { drop(old) })
        .detach();
}

/// 只检查完成源里实际使用的Mesh层；空室外画布也可在洞内首次种植并按需加载同一原型。
fn mesh_ready(
    layers: &[Layer],
    vegetation: &super::DynamicVegetationState,
    presets: Option<&feathrise_scene_content::VegetationPresetRegistry>,
    registry: &feathrise_environment::vegetation::VegetationMeshPrototypeRegistry,
    profiles: &super::super::mesh_profiles::DynamicMeshProfileState,
    requests: &mut super::super::mesh_profiles::DynamicMeshProfileRequests,
) -> Result<bool, String> {
    let mut ready = true;
    for layer in layers {
        let Some(def) = vegetation
            .mesh_source
            .as_ref()
            .and_then(|s| s.volume_mesh_layers.iter().find(|d| d.id == layer.id))
        else {
            continue;
        };
        let profile = def
            .runtime_profile
            .as_deref()
            .ok_or("三维Mesh层缺少正式原型")?;
        if !requests.contains(profile) {
            requests.request(profile)?;
        }
        ready &= presets.is_some_and(|p| profiles.ready_for(profile, p, registry))
            && def.prototype_id.is_some_and(|id| {
                registry.contains(feathrise_environment::vegetation::VegetationPrototypeId(id))
            });
    }
    Ok(ready)
}
fn drive(
    mut commands: Commands,
    mut state: InstanceResMut<State>,
    vegetation: InstanceRes<super::DynamicVegetationState>,
    ground: InstanceRes<DynamicClipmapGroundSurface>,
    fields: InstanceQuery<(Entity, &VolumeSurfaceFields, &Transform, &Visibility)>,
    prototypes: InstanceQuery<
        (
            &VolumeVegetationLayer,
            &Mesh3d,
            &GpuBladePatch,
            &GpuBladeProfileBankId,
            &VegetationBladeLayer,
        ),
        Without<VolumeWindow>,
    >,
    mut windows: InstanceQuery<
        (Entity, &VolumeVegetationLayer, &mut GpuBladePatch),
        With<VolumeWindow>,
    >,
    mut images: ResMut<Assets<Image>>,
    registry: Res<feathrise_environment::vegetation::VegetationMeshPrototypeRegistry>,
    presets: Option<Res<feathrise_scene_content::VegetationPresetRegistry>>,
    clearances: Option<Res<crate::BuildingVegetationClearanceState>>,
    mut profile_requests: InstanceResMut<super::super::mesh_profiles::DynamicMeshProfileRequests>,
    profile_state: InstanceRes<super::super::mesh_profiles::DynamicMeshProfileState>,
    work: Option<Res<super::super::voxel_objects::VoxelWorkBudget>>,
    mut banks: Option<ResMut<GpuBladeProfileBanks>>,
) {
    let Some(ground) = ground.snapshot.as_ref().filter(|g| g.ready) else {
        state.valid = false; // 同时撤销近场碰撞的授权，不能只暂停源派生。
        if let Some(root) = state.root {
            commands.entity(root).insert(Visibility::Hidden);
        }
        return;
    };
    let roots: Vec<_> = fields
        .iter()
        .filter(|(_, _, _, v)| **v != Visibility::Hidden)
        .take(32)
        .collect();
    let clearances = clearances
        .as_deref()
        .and_then(|c| c.snapshot_for(&ground.scene_id));
    let mut blade_spans = std::collections::BTreeMap::<String, f32>::new();
    for (id, _, patch, _, _) in &prototypes {
        let span = patch
            .grid_spacing()
            .max(patch.grid_spacing() / (1u32 << patch.lod_level()) as f32 * 4.0);
        blade_spans
            .entry(id.0.clone())
            .and_modify(|s| *s = s.max(span))
            .or_insert(span);
    }
    let mut identity = Identity {
        roots: roots
            .iter()
            .map(|(e, f, _, _)| (*e, Arc::as_ptr(&f.0) as usize))
            .collect(),
        support: roots
            .iter()
            .map(|(_, f, t, _)| {
                (
                    f.0.geometry_identity(),
                    t.translation.to_array().map(f32::to_bits),
                    f.0.cell_size.to_bits(),
                )
            })
            .collect(),
        ground: ground.content_revision_words(),
        vegetation: vegetation.active_root,
        prototypes: registry.generation(),
        presets: presets.as_ref().map_or(0, |p| p.revision),
        clearances: clearances.as_ref().map_or(0, |c| c.revision),
        blade_spans: blade_spans
            .iter()
            .map(|(id, span)| (id.clone(), span.to_bits()))
            .collect(),
    };
    identity.roots.sort_by_key(|v| v.0);
    identity.support.sort();
    let was_valid = state.valid;
    state.valid = state
        .active
        .as_ref()
        .is_some_and(|active| active.same_support(&identity));
    if was_valid != state.valid
        && let Some(root) = state.root
    {
        commands.entity(root).insert(if state.valid {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    } // 暂停时停用旧显示/碰撞；同一完整版本恢复时必须一起恢复，不能留下永久隐藏根。
    // 每窗只同步固定参数，作者源像素只在完整身份变化时派生。
    for (_, layer, mut window) in &mut windows {
        if let Some((_, _, prototype, _, _)) = prototypes
            .iter()
            .find(|(id, _, patch, _, _)| id.0 == layer.0 && patch.lod_level() == window.lod_level())
        {
            // 原室外窗口已选择玩家/编辑视口的正式焦点；相机只负责投影，不能另作三维LOD距离中心。
            window.set_focus(prototype.focus_radius_xyz());
        }
    }
    if state
        .waiting
        .as_ref()
        .is_some_and(|(built, _)| !built.same_source(&identity))
    {
        state.waiting = None;
    }
    let completion = if let Some(result) = state.task.as_mut().and_then(check_ready) {
        let built = state.building.take().unwrap();
        state.task = None;
        Some((built, result))
    } else if state.waiting.as_ref().is_some_and(|(_, layers)| {
        mesh_ready(
            layers,
            &vegetation,
            presets.as_deref(),
            &registry,
            &profile_state,
            &mut profile_requests,
        )
        .unwrap_or(false)
    }) {
        state
            .waiting
            .take()
            .map(|(built, layers)| (built, Ok(layers)))
    } else {
        None
    };
    if let Some((built, result)) = completion {
        if built.same_source(&identity) {
            match result {
                Ok(layers) => {
                    match mesh_ready(
                        &layers,
                        &vegetation,
                        presets.as_deref(),
                        &registry,
                        &profile_state,
                        &mut profile_requests,
                    ) {
                        Ok(true) => {}
                        Ok(false) => {
                            state.waiting = Some((identity.clone(), layers));
                            return;
                        }
                        Err(error) => {
                            warn!("三维植被未采用：{error}");
                            state.failed = Some(identity.clone());
                            return;
                        }
                    }
                    let blade_only = layers.iter().all(|layer| {
                        vegetation
                            .mesh_source
                            .as_ref()
                            .is_none_or(|s| !s.volume_mesh_layers.iter().any(|d| d.id == layer.id))
                    });
                    let reuse =
                        blade_only && state.blade_only && state.valid && state.root.is_some();
                    let layer_ids: std::collections::BTreeSet<_> = layers.iter().map(|layer| layer.id.as_str()).collect();
                    let ids = prototypes.iter().filter(|(id, _, _, bank, _)|
                        layer_ids.contains(id.0.as_str()) && bank.index() != 0)
                        .map(|(_, _, _, bank, _)| (bank.index(), *bank))
                        .collect::<std::collections::BTreeMap<_, _>>().into_values().collect::<Vec<_>>();
                    let allocation = if ids.is_empty() { None } else {
                        let Some(allocation) = banks.as_mut().and_then(|banks| banks.retain(ids)) else {
                            state.failed = Some(identity.clone());
                            return; // 模板已退役时保留上一完整三维根，不能借用已被其他网格复用的绘制槽。
                        };
                        Some(allocation)
                    }; // 洞壁持有自己的引用，室外根热更或删除后仍能维持上一完整画面。
                    // 叶片草窗保留实体与GPU历史输出。换掉根会先丢失旧缓冲，再等待新图片上传。
                    let root = if reuse {
                        state.root.unwrap()
                    } else {
                        commands
                            .spawn((
                                VolumeVegetationRoot,
                                Name::new("三维GPU植被"),
                                Transform::IDENTITY,
                                Visibility::Inherited,
                                DespawnOnExit(feathrise_scene_core::state::ScenePhase::Active),
                            ))
                            .id()
                    };
                    if let Some(allocation) = allocation { commands.entity(root).insert(allocation); }
                    else { commands.entity(root).remove::<GpuBladeProfileBankAllocation>(); }
                    let mut retained = std::collections::HashSet::new();
                    let mut collision_sources = Vec::new();
                    let mut blade_sources = std::collections::BTreeMap::new();
                    for layer in layers {
                        let seed = layer.id.bytes().fold(2166136261u32, |h, b| {
                            (h ^ u32::from(b)).wrapping_mul(16777619)
                        });
                        if let Some(definition) = vegetation
                            .mesh_source
                            .as_ref()
                            .and_then(|s| s.volume_mesh_layers.iter().find(|d| d.id == layer.id))
                        {
                            if let Some(prototype) = definition.prototype_id.and_then(|id| {
                                registry.get(feathrise_environment::vegetation::VegetationPrototypeId(id))
                            }) {
                                let physics =
                                    definition.runtime_profile.as_deref().and_then(|id| {
                                        presets.as_deref().and_then(|p| {
                                            super::collision::resolve_mesh_preset_physics(p, id)
                                                .ok()
                                        })
                                    });
                                let cells = feathrise_environment::vegetation::VolumeMeshCells {
                                    cells: layer.cells.clone(),
                                    seed: definition.seed as u32,
                                    spacing: definition.spacing_meters,
                                    scale_range: definition.scale_range,
                                    ground_embed_depth: physics
                                        .as_ref()
                                        .map_or(0., |p| p.ground_embed_depth),
                                    min: layer.min,
                                    max: layer.max,
                                };
                                if let Some(physics) = physics {
                                    collision_sources.push(collision::Source {
                                        cells: cells.clone(),
                                        collision: physics.collision.clone(),
                                    });
                                }
                                let batch = feathrise_environment::vegetation::spawn_volume_mesh_batch(
                                    &mut commands,
                                    feathrise_environment::vegetation::VegetationLayerId(
                                        definition.runtime_layer_id.unwrap_or(0),
                                    ),
                                    prototype,
                                    0,
                                    cells,
                                );
                                commands.entity(batch).insert(ChildOf(root));
                            }
                        }
                        let source = images.add(layer.image);
                        blade_sources.insert(layer.id.clone(), source.clone());
                        for (id, mesh, patch, bank, binding) in &prototypes {
                            if id.0 != layer.id {
                                continue;
                            }
                            let center = (layer.min + layer.max) * 0.5;
                            let components = (
                                VolumeWindow,
                                VolumeVegetationLayer(layer.id.clone()),
                                Mesh3d(mesh.0.clone()),
                                (*patch)
                                    .with_volume_cell_source(layer.count, layer.step)
                                    .with_volume_seed(seed),
                                *bank,
                                *binding,
                                GpuBladeVolumeSource(source.clone()),
                                Transform::from_translation(center),
                                Visibility::Inherited,
                                Aabb {
                                    center: Vec3A::ZERO,
                                    half_extents: Vec3A::from(
                                        (layer.max - layer.min) * 0.5 + Vec3::splat(4.),
                                    ),
                                },
                                bevy::camera::visibility::NoAutoAabb,
                                ChildOf(root),
                            );
                            let existing = if reuse {
                                windows
                                    .iter()
                                    .find(|(_, id, p)| {
                                        id.0 == layer.id && p.lod_level() == patch.lod_level()
                                    })
                                    .map(|(entity, _, _)| entity)
                            } else {
                                None
                            };
                            if let Some(entity) = existing {
                                commands.entity(entity).insert(components);
                                retained.insert(entity);
                            } else {
                                commands.spawn(components);
                            }
                        }
                    }
                    state.collision_sources = Arc::new(collision_sources);
                    state.blade_sources = blade_sources;
                    state.boundary_revision = state.boundary_revision.wrapping_add(1);
                    if reuse {
                        for (entity, _, _) in &windows {
                            if !retained.contains(&entity) {
                                commands.entity(entity).try_despawn();
                            }
                        }
                    } else if let Some(old) = state.root.replace(root) {
                        commands.entity(old).try_despawn();
                    }
                    state.blade_only = blade_only;
                    state.active = Some(identity.clone());
                    state.valid = true;
                }
                Err(error) => {
                    warn!("三维植被未采用：{error}");
                    state.failed = Some(identity.clone());
                }
            }
        } // 迟到结果不替换当前显示。
    }
    if state.task.is_none()
        && state.waiting.is_none()
        && state.active.as_ref() != Some(&identity)
        && state.failed.as_ref() != Some(&identity)
    {
        let credit = if ground.is_density_surface() {
            if let Some(work) = work.as_ref() {
                let Some(credit) = work.acquire() else {
                    return;
                };
                Some(credit)
            } else {
                None
            }
        } else {
            None
        };
        let input = roots
            .into_iter()
            .map(|(_, f, t, _)| (f.0.clone(), t.translation))
            .collect();
        let snapshot = ground.clone();
        let native = snapshot
            .is_density_surface()
            .then(|| vegetation.mesh_source.clone())
            .flatten()
            .map(|source| {
                let names = prototypes
                    .iter()
                    .map(|(id, _, _, _, binding)| (binding.0.0, id.0.clone()))
                    .collect();
                (source, names)
            });
        state.building = Some(identity);
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _credit = credit;
            source::build_with_native_density(input, snapshot, clearances, blade_spans, native)
        }));
    }
}

pub(super) fn add_instance_systems(schedule: &mut Schedule) {
    schedule.add_systems(
        (drive, boundary::sync_inputs, collision::drive)
            .chain()
            .after(super::drive_dynamic_mesh_residency)
            .after(super::super::clipmap::sync_clipmap_ground_surface),
    );
}
