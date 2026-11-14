//! 模型夹具的只读GPU就绪门：冻结实际根子树的四窗身份，禁止借地形计数或迟到回读通过。
use bevy::prelude::*;
use feathrise_scene::{DynamicModelVegetationStatus, ScenePlacementInstance, SubsceneActive, SubsceneInstance};
use feathrise_scene_content::{SceneId, SceneInstancePath};
use feathrise_vegetation::blade::{
    GpuBladeDensityMap, GpuBladeGpuReadbackStats, GpuBladePatch, GpuBladeReadbackConfig,
    GpuBladeRenderStats, GpuBladeSourceRevision, GpuBladeView, GpuBladeVolumeSource,
};
use serde_json::{Value, json};

const OBJECT_ID: &str = "model-surface-fixture";
const MODEL_ROOT_NAME: &str = "模型植被_model-surface-fixture";
const WINDOW_COUNT: usize = 4;
const NODE_BUDGET: usize = 256;
const DEPTH_BUDGET: usize = 32;
const WINDOW_MASK: u32 = (1 << WINDOW_COUNT) - 1;

/// 仅保存验收目标的小型身份，不持有生产文档、像素或模型几何。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    scene_id: String,
    instance_path: String,
    runtime_revision: u64,
    input_revision: Option<u64>,
    applied_revision: Option<u64>,
    applied_document: Option<usize>,
    placement: Entity,
    model_root: Entity,
    windows: [Entity; WINDOW_COUNT],
}

#[derive(Clone, Debug)]
struct Selection {
    identity: Identity,
    ticket: u64,
    sample_baseline: u64,
}

#[derive(Resource, Default)]
struct Probe {
    selection: Option<Selection>,
    next_ticket: u64,
    selection_active: bool,
    ready: bool,
    detail: String,
}

#[derive(Resource, Default)]
struct PipelineStages([u8; WINDOW_COUNT]);

pub(super) fn install(app: &mut App) {
    app.init_resource::<Probe>();
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        // 只发布固定四阶段的当前状态，不安装另一条渲染或输入路径。
        render.add_systems(bevy::render::ExtractSchedule, publish_pipeline_stages);
    }
}

fn publish_pipeline_stages(world: &mut World) {
    let stages = feathrise_vegetation::blade::gpu_blade_compute_pipeline_states(world);
    world.resource_mut::<bevy::render::MainWorld>().insert_resource(PipelineStages(stages));
}

impl Probe {
    fn wait(&mut self, detail: String) {
        self.ready = false;
        self.selection_active = false;
        self.detail = detail;
        // 保留上一目标供超时报告诊断；再次发现同一目标也必须换新票据。
    }

    fn freeze(&mut self, identity: Identity, sample_baseline: u64) -> Result<GpuBladeReadbackConfig, String> {
        let ticket = self.next_ticket.checked_add(1).ok_or("模型验收诊断票据已耗尽")?;
        let config = GpuBladeReadbackConfig::for_windows(1, ticket, &identity.windows)
            .ok_or("模型验收四窗身份重复或无效")?;
        self.next_ticket = ticket;
        self.selection = Some(Selection { identity, ticket, sample_baseline });
        self.selection_active = true;
        self.ready = false;
        Ok(config)
    }
}

/// 每帧只读固定预算的夹具实例/子树，GPU映射仍由生产诊断的异步三缓冲完成。
pub(super) fn observe(world: &mut World, scene: &SceneId) {
    let identity = match discover(world, scene) {
        Ok(identity) => identity,
        Err(error) => {
            world.resource_mut::<Probe>().wait(error);
            return;
        }
    };
    let gpu = world.get_resource::<GpuBladeGpuReadbackStats>().copied().unwrap_or_default();
    let config_ticket = world.get_resource::<GpuBladeReadbackConfig>().map(|config| config.selection_ticket());
    let changed = {
        let probe = world.resource::<Probe>();
        !probe.selection_active || probe.selection.as_ref().is_none_or(|selection| {
            selection.identity != identity || config_ticket != Some(selection.ticket)
        })
    };
    if changed {
        let config = match world.resource_mut::<Probe>().freeze(identity, gpu.sample_sequence) {
            Ok(config) => config,
            Err(error) => {
                world.resource_mut::<Probe>().wait(error);
                return;
            }
        };
        // 只筛选诊断拷贝；不修改密度、视图、草窗位置或生产可见性。
        world.insert_resource(config);
    }
    let selection = world.resource::<Probe>().selection.as_ref().unwrap();
    let result = gpu_matches(selection, &gpu).and_then(|()| {
        let render = world.get_resource::<GpuBladeRenderStats>().ok_or("尚无完整渲染帧统计")?;
        render_matches(render)?;
        if world.get_resource::<PipelineStages>().is_none_or(|stages| stages.0 != [3; WINDOW_COUNT]) {
            return Err("Compute四阶段尚未全部就绪");
        }
        Ok(())
    });
    let mut probe = world.resource_mut::<Probe>();
    probe.ready = result.is_ok();
    probe.detail = match result {
        Ok(()) => "当前模型四窗已完成同票据GPU分类、有效间接输出与真实绘制提交".into(),
        Err(error) => error.into(),
    };
}

pub(super) fn ready(world: &World) -> bool {
    world.get_resource::<Probe>().is_some_and(|probe| probe.selection_active && probe.ready)
}

fn discover(world: &mut World, scene: &SceneId) -> Result<Identity, String> {
    let status = world.get_resource::<DynamicModelVegetationStatus>()
        .filter(|status| status.scene_id.as_ref() == Some(scene)
            && status.instance_path.as_ref() == Some(&SceneInstancePath::root(scene.clone()))
            && status.ready && !status.failed)
        .cloned().ok_or("当前根场景模型植被尚未完整采用")?;
    let mut placements = world.query::<(Entity, &ScenePlacementInstance)>();
    let mut candidates = Vec::new();
    for (visited, (entity, placement)) in placements.iter(world).take(NODE_BUDGET + 1).enumerate() {
        if visited == NODE_BUDGET { return Err("模型验收实例枚举超出256项预算".into()); }
        if placement.id() == OBJECT_ID && placement.scene_id() == scene && placement.is_active()
            && in_root_scope(world, entity, scene)? {
            candidates.push(entity);
        }
    }
    let [placement] = candidates.as_slice() else {
        return Err(format!("当前根场景需唯一活动{OBJECT_ID}，实际{}项", candidates.len()));
    };
    let descendants = bounded_subtree(world, *placement)?;
    let roots = descendants.iter().copied().filter(|entity| {
        world.get::<Name>(*entity).is_some_and(|name| name.as_str() == MODEL_ROOT_NAME)
            && world.get::<ChildOf>(*entity).is_some_and(|parent| parent.parent() == *placement)
            && world.get::<InheritedVisibility>(*entity).is_some_and(|visibility|visibility.get())
            && world.get::<Visibility>(*entity) != Some(&Visibility::Hidden)
    }).collect::<Vec<_>>();
    let [model_root] = roots.as_slice() else {
        return Err(format!("夹具实际子树需唯一可见模型植被根，实际{}项", roots.len()));
    };
    let mut windows = bounded_subtree(world, *model_root)?.into_iter()
        .filter(|entity| world.get::<GpuBladePatch>(*entity).is_some()).collect::<Vec<_>>();
    windows.sort_unstable_by_key(|entity| entity.to_bits());
    let windows: [Entity; WINDOW_COUNT] = windows.try_into()
        .map_err(|windows: Vec<Entity>| format!("夹具模型根需四个真实GPU草窗，实际{}项", windows.len()))?;
    for (lod, entity) in windows.iter().enumerate() {
        let patch = world.get::<GpuBladePatch>(*entity).unwrap();
        if world.get::<GpuBladeVolumeSource>(*entity).is_none()
            || world.get::<Mesh3d>(*entity).is_none()
            || !world.get::<InheritedVisibility>(*entity).is_some_and(|visibility|visibility.get()) {
            return Err(format!("模型草窗{lod}尚缺实际表面源、网格或可见性"));
        }
        if patch.lod_level() >= WINDOW_COUNT as u32 {
            return Err("模型夹具草窗LOD超出四档".into());
        }
    }
    let lod_mask = windows.iter().fold(0u32, |mask, entity| {
        mask | (1 << world.get::<GpuBladePatch>(*entity).unwrap().lod_level())
    });
    if lod_mask != WINDOW_MASK { return Err("模型夹具草窗未覆盖完整四档LOD".into()); }
    Ok(Identity {
        scene_id: scene.as_str().into(), instance_path: status.instance_path.unwrap().to_string(),
        runtime_revision: status.runtime_revision, input_revision: status.input_revision,
        applied_revision: status.applied_revision, applied_document: status.applied_document,
        placement: *placement, model_root: *model_root, windows,
    })
}

fn in_root_scope(world: &World, mut entity: Entity, scene: &SceneId) -> Result<bool, String> {
    let root_path = SceneInstancePath::root(scene.clone());
    for _ in 0..DEPTH_BUDGET {
        if let Some(instance) = world.get::<SubsceneInstance>(entity) {
            if instance.path != root_path || world.get::<SubsceneActive>(instance.batch).is_none() {
                return Ok(false);
            }
        }
        let Some(parent) = world.get::<ChildOf>(entity) else { return Ok(true); };
        entity = parent.parent();
    }
    Err("模型验收实例祖先超出32层预算".into())
}

fn bounded_subtree(world: &World, root: Entity) -> Result<Vec<Entity>, String> {
    let mut stack = vec![(root, 0usize)];
    let mut result = Vec::new();
    while let Some((entity, depth)) = stack.pop() {
        if result.len() == NODE_BUDGET || depth >= DEPTH_BUDGET {
            return Err("模型验收子树超出256节点或32层预算".into());
        }
        result.push(entity);
        if let Some(children) = world.get::<Children>(entity) {
            if stack.len() + result.len() + children.len() > NODE_BUDGET {
                return Err("模型验收子树超出256节点预算".into());
            }
            stack.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    Ok(result)
}

/// 与同次GPU copy冻结的完整身份匹配；源修订为零合法，不能把未提供的生产字段伪造为必填。
fn gpu_matches(selection: &Selection, gpu: &GpuBladeGpuReadbackStats) -> Result<(), &'static str> {
    if gpu.sample_sequence <= selection.sample_baseline { return Err("等待选择目标后的新GPU样本"); }
    if gpu.selection_ticket != selection.ticket { return Err("GPU样本票据不属于当前模型目标"); }
    if gpu.sampled_windows != WINDOW_COUNT as u32 || gpu.truncated_windows != 0 {
        return Err("GPU样本未完整覆盖当前模型四窗");
    }
    let mut actual = [0u64; WINDOW_COUNT];
    actual.copy_from_slice(&gpu.sampled_main_entities[..WINDOW_COUNT]);
    actual.sort_unstable();
    let mut expected = selection.identity.windows.map(Entity::to_bits);
    expected.sort_unstable();
    if actual != expected { return Err("GPU样本混入其他窗口或旧实体代次"); }
    if gpu.sampled_output_valid_mask & WINDOW_MASK != WINDOW_MASK {
        return Err("当前模型四窗尚未全部具有有效GPU输出");
    }
    if gpu.scene_errors != 0 || gpu.overflow_instances != 0 { return Err("GPU回读错误或实例容量溢出"); }
    if gpu.surface_survivors == 0 || gpu.rendered_instances == 0 {
        return Err("当前模型GPU表面存活数或实际间接实例数仍为零");
    }
    Ok(())
}

fn render_matches(render: &GpuBladeRenderStats) -> Result<(), &'static str> {
    if render.compute_pipeline_error_stages != 0 || render.pipeline_error_draws != 0
        || render.specialization_errors != 0 { return Err("Compute或绘制管线已报告错误"); }
    if render.compute_pipeline_ready_stages != 4 || render.compute_pipeline_waiting_stages != 0 {
        return Err("Compute四阶段仍在等待异步编译");
    }
    if render.target_views == 0 || render.missing_phase_views != 0
        || render.drawn_draws == 0 || render.pipeline_ready_draws == 0
        || render.queued_draws == 0 || render.missing_mesh_instances != 0
        || render.missing_gpu_meshes != 0 {
        return Err("目标视图尚未完成真实叶片绘制提交");
    }
    Ok(())
}

/// 成功与失败共用真实快照；本函数不更新就绪标记、不修改作者或GPU数据。
pub(super) fn snapshot(world: &mut World) -> Value {
    let probe = world.resource::<Probe>();
    let ready = probe.ready;
    let selection_active = probe.selection_active;
    let detail = probe.detail.clone();
    let selection = probe.selection.clone();
    let gpu = world.get_resource::<GpuBladeGpuReadbackStats>().map(gpu_evidence);
    let render = world.get_resource::<GpuBladeRenderStats>().map(render_evidence);
    let stages = world.get_resource::<PipelineStages>().map(|stages| {
        ["cull_tiles", "finalize_tiles", "classify_and_compact", "finalize_indirect"]
            .into_iter().zip(stages.0).map(|(stage, code)| json!({
                "stage":stage, "code":code, "state":match code {
                    0 => "not-installed", 1 => "queued", 2 => "creating", 3 => "ready",
                    4 => "shader-not-loaded", 5 => "shader-import-unavailable", _ => "permanent-error",
                },
            })).collect::<Vec<_>>()
    });
    let windows = selection.as_ref().map(|selection| selection.identity.windows.into_iter()
        .map(|entity| window_evidence(world, entity, &selection.identity)).collect::<Vec<_>>());
    let source = selection.as_ref().map(|selection| entity_evidence(world, selection.identity.placement));
    let model_root = selection.as_ref().map(|selection| entity_evidence(world, selection.identity.model_root));
    let selection = selection.map(|selection| json!({
        "object_id":OBJECT_ID, "ticket":selection.ticket, "sample_baseline":selection.sample_baseline,
        "scene_id":selection.identity.scene_id, "instance_path":selection.identity.instance_path,
        "runtime_revision":selection.identity.runtime_revision, "input_revision":selection.identity.input_revision,
        "applied_revision":selection.identity.applied_revision, "applied_document":selection.identity.applied_document,
        "placement_entity":selection.identity.placement.to_bits(), "model_root_entity":selection.identity.model_root.to_bits(),
        "window_entities":selection.identity.windows.map(Entity::to_bits),
    }));
    let views = world.query::<(Entity, &GpuBladeView, Option<&Camera>, Option<&Projection>, Option<&GlobalTransform>)>()
        .iter(world).take(8).map(|(entity, view, camera, projection, global)| json!({
            "entity":entity.to_bits(), "gpu_view_debug":format!("{view:?}"),
            "primary":world.get::<feathrise_scene_core::view::PrimarySceneView>(entity).is_some(),
            "active":camera.map(|camera|camera.is_active), "projection_debug":projection.map(|projection|format!("{projection:?}")),
            "world_matrix":global.map(|transform|transform.to_matrix().to_cols_array()),
        })).collect::<Vec<_>>();
    json!({"ready":ready,"detail":detail,"selection_active":selection_active,"selection":selection,
        "gpu":gpu,"render":render,"compute_stages":stages,"source":source,"model_root":model_root,
        "windows":windows,"views":views,
        "scope":"explicit-model-fixture; GPU selection is limited to this root subtree; render totals are supporting evidence"})
}

fn entity_evidence(world: &World, entity: Entity) -> Value {
    json!({"entity":entity.to_bits(),"alive":world.get_entity(entity).is_ok(),
        "name":world.get::<Name>(entity).map(Name::as_str),
        "parent":world.get::<ChildOf>(entity).map(|parent|parent.parent().to_bits()),
        "local_matrix":world.get::<Transform>(entity).map(|transform|transform.to_matrix().to_cols_array()),
        "world_matrix":world.get::<GlobalTransform>(entity).map(|transform|transform.to_matrix().to_cols_array()),
        "visibility":world.get::<Visibility>(entity).map(|visibility|format!("{visibility:?}")),
        "inherited_visibility":world.get::<InheritedVisibility>(entity).map(|visibility|visibility.get()),
        "view_visibility":world.get::<ViewVisibility>(entity).map(|visibility|visibility.get())})
}

fn window_evidence(world: &World, entity: Entity, identity: &Identity) -> Value {
    let mut evidence = entity_evidence(world, entity);
    evidence["placement_entity"] = json!(identity.placement.to_bits());
    evidence["model_root_entity"] = json!(identity.model_root.to_bits());
    evidence["belongs_to_model_root"] = json!(bounded_subtree(world, identity.model_root)
        .is_ok_and(|descendants| descendants.contains(&entity)));
    evidence["patch_debug"] = json!(world.get::<GpuBladePatch>(entity).map(|patch|format!("{patch:?}")));
    evidence["lod_level"] = json!(world.get::<GpuBladePatch>(entity).map(|patch|patch.lod_level()));
    evidence["density_map_debug"] = json!(world.get::<GpuBladeDensityMap>(entity).map(|map|format!("{map:?}")));
    evidence["has_density_map"] = json!(world.get::<GpuBladeDensityMap>(entity).is_some());
    evidence["source_revision"] = json!(world.get::<GpuBladeSourceRevision>(entity).map(|revision|revision.0));
    evidence["mesh"] = json!(world.get::<Mesh3d>(entity).map(|mesh|format!("{:?}",mesh.0)));
    evidence["source_to_world"] = json!(world.get::<Transform>(entity)
        .zip(world.get::<GlobalTransform>(entity))
        .map(|(local,global)|(global.to_matrix()*local.to_matrix().inverse()).to_cols_array()));
    evidence["volume_source"] = json!(world.get::<GpuBladeVolumeSource>(entity).map(|source| {
        let image = world.get_resource::<Assets<Image>>().and_then(|images|images.get(&source.0));
        json!({"handle":format!("{:?}",source.0),"image":image.map(|image|json!({
            "size":[image.width(),image.height(),image.texture_descriptor.size.depth_or_array_layers],
            "format":format!("{:?}",image.texture_descriptor.format),"cpu_bytes":image.data.as_ref().map(Vec::len),
            // 仅观察首个作者样本的三个RGBA，不扫描整张纹理；RenderWorld独占资产可能已经释放CPU副本。
            "first_sample_f32":image.data.as_ref().filter(|_|image.texture_descriptor.format
                == bevy::render::render_resource::TextureFormat::Rgba32Float)
                .map(|bytes|bytes.chunks_exact(4).take(12).map(|chunk|f32::from_le_bytes(chunk.try_into().unwrap())).collect::<Vec<_>>()),
        }))})
    }));
    evidence
}

fn gpu_evidence(gpu: &GpuBladeGpuReadbackStats) -> Value {
    let count = (gpu.sampled_windows as usize).min(gpu.sampled_main_entities.len());
    json!({"sample_sequence":gpu.sample_sequence,"source_revision":gpu.source_revision,"selection_ticket":gpu.selection_ticket,
        "sampled_main_entities":gpu.sampled_main_entities[..count],"sampled_source_revisions":gpu.sampled_source_revisions[..count],
        "sampled_output_valid_mask":gpu.sampled_output_valid_mask,"sampled_windows":gpu.sampled_windows,
        "truncated_windows":gpu.truncated_windows,"candidate_count":gpu.candidate_count,"total_tiles":gpu.total_tiles,
        "active_tiles":gpu.active_tiles,"scanned_candidates":gpu.scanned_candidates,"compact_capacity":gpu.compact_capacity,
        "distance_survivors":gpu.distance_survivors,"density_survivors":gpu.density_survivors,"surface_survivors":gpu.surface_survivors,
        "rendered_instances":gpu.rendered_instances,"rendered_instances_by_lod":gpu.rendered_instances_by_lod,
        "overflow_instances":gpu.overflow_instances,"dropped_samples":gpu.dropped_samples,"scene_errors":gpu.scene_errors})
}

fn render_evidence(render: &GpuBladeRenderStats) -> Value {
    json!({"target_views":render.target_views,"missing_phase_views":render.missing_phase_views,
        "extracted_windows":render.extracted_windows,"input_ready_windows":render.input_ready_windows,
        "height_waiting_windows":render.height_waiting_windows,"density_waiting_windows":render.density_waiting_windows,
        "uniform_waiting_windows":render.uniform_waiting_windows,"prepared_bind_groups":render.prepared_bind_groups,
        "prepared_lod_mask":render.prepared_lod_mask,"prepared_profile_lod_mask":render.prepared_profile_lod_mask,
        "resident_reused_windows":render.resident_reused_windows,"compute_pipeline_ready_stages":render.compute_pipeline_ready_stages,
        "compute_pipeline_waiting_stages":render.compute_pipeline_waiting_stages,"compute_pipeline_error_stages":render.compute_pipeline_error_stages,
        "visible_windows":render.visible_windows,"queued_draws":render.queued_draws,"queued_candidates":render.queued_candidates,
        "queued_shadow_cascades":render.queued_shadow_cascades,"drawn_shadow_commands":render.drawn_shadow_commands,
        "pipeline_ready_draws":render.pipeline_ready_draws,"pipeline_ready_lod_mask":render.pipeline_ready_lod_mask,
        "pipeline_ready_profile_lod_mask":render.pipeline_ready_profile_lod_mask,"pipeline_waiting_draws":render.pipeline_waiting_draws,
        "pipeline_error_draws":render.pipeline_error_draws,"missing_mesh_instances":render.missing_mesh_instances,
        "missing_gpu_meshes":render.missing_gpu_meshes,"specialization_errors":render.specialization_errors,
        "drawn_draws":render.drawn_draws,"drawn_candidates":render.drawn_candidates,"drawn_lod_mask":render.drawn_lod_mask,
        "drawn_profile_lod_mask":render.drawn_profile_lod_mask})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(world: &mut World) -> Identity {
        Identity { scene_id:"MODEL-FIXTURE".into(),instance_path:"MODEL-FIXTURE".into(),runtime_revision:3,
            input_revision:None,applied_revision:None,applied_document:Some(7),
            placement:world.spawn_empty().id(),model_root:world.spawn_empty().id(),
            windows:std::array::from_fn(|_|world.spawn_empty().id()) }
    }

    fn current_sample(selection: &Selection) -> GpuBladeGpuReadbackStats {
        let mut gpu = GpuBladeGpuReadbackStats { sample_sequence:selection.sample_baseline+1,
            selection_ticket:selection.ticket,sampled_windows:4,sampled_output_valid_mask:WINDOW_MASK,
            surface_survivors:11,rendered_instances:11,..default() };
        gpu.sampled_main_entities[..WINDOW_COUNT].copy_from_slice(&selection.identity.windows.map(Entity::to_bits));
        gpu
    }

    #[test]
    fn model_surface_current_complete_gpu_sample_does_not_require_optional_source_revision() {
        let mut world = World::new();
        let mut probe = Probe::default();
        probe.freeze(identity(&mut world), 40).unwrap();
        let selection = probe.selection.as_ref().unwrap();
        let mut gpu = current_sample(selection);
        assert_eq!(gpu.source_revision,0);
        assert!(gpu_matches(selection,&gpu).is_ok());
        gpu.sampled_main_entities[..WINDOW_COUNT].reverse();
        assert!(gpu_matches(selection,&gpu).is_ok(),"回读顺序可以变化，实体集合必须相同");
    }

    #[test]
    fn model_surface_old_sequence_and_old_ticket_cannot_pass() {
        let mut world = World::new();
        let mut probe = Probe::default();
        probe.freeze(identity(&mut world), 40).unwrap();
        let selection = probe.selection.as_ref().unwrap();
        let mut gpu = current_sample(selection);
        gpu.sample_sequence = selection.sample_baseline;
        assert!(gpu_matches(selection,&gpu).is_err());
        gpu.sample_sequence += 1;
        gpu.selection_ticket += 1;
        assert!(gpu_matches(selection,&gpu).is_err());
    }

    #[test]
    fn model_surface_missing_mixed_and_invalid_windows_cannot_pass() {
        let mut world = World::new();
        let mut probe = Probe::default();
        probe.freeze(identity(&mut world), 40).unwrap();
        let selection = probe.selection.as_ref().unwrap();
        let gpu = current_sample(selection);
        for incomplete in [
            GpuBladeGpuReadbackStats {sampled_windows:3,..gpu},
            GpuBladeGpuReadbackStats {truncated_windows:1,..gpu},
            GpuBladeGpuReadbackStats {sampled_output_valid_mask:7,..gpu},
        ] { assert!(gpu_matches(selection,&incomplete).is_err()); }
        let mut mixed = gpu;
        mixed.sampled_main_entities[0] = world.spawn_empty().id().to_bits();
        assert!(gpu_matches(selection,&mixed).is_err(),"其他地形窗口不能补齐模型四窗");
        mixed.sampled_main_entities[0] = mixed.sampled_main_entities[1];
        assert!(gpu_matches(selection,&mixed).is_err(),"重复窗口不能补齐四窗");
    }

    #[test]
    fn model_surface_zero_instances_and_gpu_errors_cannot_pass() {
        let mut world = World::new();
        let mut probe = Probe::default();
        probe.freeze(identity(&mut world), 40).unwrap();
        let selection = probe.selection.as_ref().unwrap();
        let gpu = current_sample(selection);
        for invalid in [
            GpuBladeGpuReadbackStats {surface_survivors:0,..gpu},
            GpuBladeGpuReadbackStats {rendered_instances:0,..gpu},
            GpuBladeGpuReadbackStats {overflow_instances:1,..gpu},
            GpuBladeGpuReadbackStats {scene_errors:1,..gpu},
        ] { assert!(gpu_matches(selection,&invalid).is_err()); }
    }

    #[test]
    fn model_surface_rebuild_and_entity_generation_replace_ticket_and_baseline() {
        let mut world = World::new();
        let mut probe = Probe::default();
        let mut target = identity(&mut world);
        probe.freeze(target.clone(), 40).unwrap();
        let old = current_sample(probe.selection.as_ref().unwrap());
        target.runtime_revision += 1;
        probe.freeze(target.clone(), old.sample_sequence).unwrap();
        assert_eq!(probe.selection.as_ref().unwrap().ticket,2);
        assert!(gpu_matches(probe.selection.as_ref().unwrap(),&old).is_err());
        let old_entity = target.windows[0];
        world.despawn(old_entity);
        target.windows[0] = world.spawn_empty().id();
        assert_ne!(old_entity.to_bits(),target.windows[0].to_bits());
        probe.freeze(target, 50).unwrap();
        let selection = probe.selection.as_ref().unwrap();
        let mut replacement = current_sample(selection);
        replacement.sampled_main_entities[0] = old_entity.to_bits();
        assert!(gpu_matches(selection,&replacement).is_err());
        assert_eq!(selection.sample_baseline,50);
        assert_eq!(selection.ticket,3);
        probe.wait("重建等待".into());
        assert!(!probe.selection_active);
        assert!(!probe.ready);
        assert!(probe.selection.is_some(),"超时仍保留上一身份的诊断");
    }

    #[test]
    fn model_surface_ready_gpu_does_not_replace_real_render_pipeline_gate() {
        let complete = GpuBladeRenderStats {target_views:1,compute_pipeline_ready_stages:4,
            queued_draws:4,pipeline_ready_draws:4,drawn_draws:32,..default()};
        assert!(render_matches(&complete).is_ok());
        for incomplete in [
            GpuBladeRenderStats {compute_pipeline_ready_stages:3,..complete},
            GpuBladeRenderStats {compute_pipeline_waiting_stages:1,..complete},
            GpuBladeRenderStats {compute_pipeline_error_stages:1,..complete},
            GpuBladeRenderStats {pipeline_error_draws:1,..complete},
            GpuBladeRenderStats {specialization_errors:1,..complete},
            GpuBladeRenderStats {drawn_draws:0,..complete},
            GpuBladeRenderStats {missing_gpu_meshes:1,..complete},
        ] { assert!(render_matches(&incomplete).is_err()); }
    }

    #[test]
    fn model_surface_subtree_does_not_include_sibling_terrain_windows_and_has_fixed_budget() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let model = world.spawn(ChildOf(root)).id();
        let model_window = world.spawn(ChildOf(model)).id();
        let terrain = world.spawn(ChildOf(root)).id();
        let terrain_window = world.spawn(ChildOf(terrain)).id();
        assert_eq!(bounded_subtree(&world,model).unwrap(),vec![model,model_window]);
        assert!(!bounded_subtree(&world,model).unwrap().contains(&terrain_window));
        for _ in 0..NODE_BUDGET { world.spawn(ChildOf(model)); }
        assert!(bounded_subtree(&world,model).is_err());
    }
}
