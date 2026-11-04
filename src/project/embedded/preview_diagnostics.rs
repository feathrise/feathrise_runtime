//! 验收等待门的真实只读快照；两秒最多一次、单后台写槽、固定文件，不参与任何成功判定。
use super::*;
use bevy::ecs::system::SystemState;
use feathrise_map_content::{ActiveDynamicMap, TerrainId};
use feathrise_scene::{DynamicBladeDraft, DynamicMapBoundaryStatus, DynamicMapRuntimeStatus, DynamicVegetationStatus,
    DynamicWaterStatus, MapPlacementRuntimeStatus, TerrainObjectAuthoringTarget, TerrainObjectQueries};
use feathrise_scene_core::residency::InstanceResourceTable;
use feathrise_vegetation::blade::{GpuBladePatch, GpuBladeView};
use feathrise_scene_core::state::{LoadingProgress, PendingSceneTransition, ScenePhase, SceneStartupGate};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Resource,Default)]
struct PipelineStages([u8;4]);

type ViewQueries = (
    Query<'static,'static,(Entity,&'static GpuBladePatch,Option<&'static Transform>,Option<&'static GlobalTransform>)>,
    Query<'static,'static,(Entity,&'static Transform,Option<&'static GlobalTransform>,Option<&'static Projection>,Has<feathrise_scene_core::view::PrimarySceneView>),With<GpuBladeView>>,
    Query<'static,'static,(Entity,&'static Transform),With<feathrise_scene::SceneRuntimeObserver>>,
);

pub(super) fn install_render(app:&mut App) {
    if let Some(render)=app.get_sub_app_mut(bevy::render::RenderApp) {
        render.add_systems(bevy::render::ExtractSchedule,publish_pipeline_stages);
    }
}
fn publish_pipeline_stages(world:&mut World) {
    let stages=feathrise_vegetation::blade::gpu_blade_compute_pipeline_states(world);
    world.resource_mut::<bevy::render::MainWorld>().insert_resource(PipelineStages(stages));
}

#[derive(Default)]
pub(super) struct State {
    next: Option<Instant>,
    writer: Option<Task<Result<(), String>>>,
    terrain: Option<SystemState<TerrainObjectQueries<'static, 'static>>>,
    view: Option<SystemState<ViewQueries>>,
}

pub(super) fn record(world: &mut World, evidence: &mut Evidence) {
    if let Some(writer) = evidence.diagnostics.writer.as_mut() {
        let Some(result) = check_ready(writer) else { return; };
        evidence.diagnostics.writer = None;
        if let Err(error) = result { error!("实时预览等待记录失败：{error}"); }
    }
    let now = Instant::now();
    if evidence.diagnostics.next.is_some_and(|next| now < next) { return; }
    evidence.diagnostics.next = Some(now + Duration::from_secs(2));
    let view = evidence.diagnostics.view.get_or_insert_with(||SystemState::new(world));
    let view = match view.get(world) {
        Ok((patches,cameras,observers)) => json!({
            // 只读实际采用的最多四窗及唯一观察相机；不按地图规模遍历或重新推算GPU逐株结果。
            "patches":patches.iter().take(4).map(|(entity,patch,local,global)|json!({
                "entity":entity.to_bits(),"origin":patch.origin().to_array(),"focus":patch.focus().to_array(),
                "grid_spacing":patch.grid_spacing(),"candidate_count":patch.instance_count(),
                "visible_radius":patch.visible_radius(),"lod_level":patch.lod_level(),"lod_count":patch.lod_count(),
                "hierarchical":patch.uses_hierarchical_lod(),"ultra_budget_block_side":patch.ultra_lod_budget_block_side(),
                "source_to_world":local.zip(global).map(|(local,global)|(global.to_matrix()*local.to_matrix().inverse()).to_cols_array())
            })).collect::<Vec<_>>(),
            "cameras":cameras.iter().take(2).map(|(entity,local,global,projection,primary)|json!({
                "entity":entity.to_bits(),"primary":primary,"local_position":local.translation.to_array(),
                "world_position":global.map(|global|global.translation().to_array()),
                "projection":projection.map(debug_status)
            })).collect::<Vec<_>>(),
            "observers":observers.iter().take(2).map(|(entity,transform)|json!({
                "entity":entity.to_bits(),"position":transform.translation.to_array()
            })).collect::<Vec<_>>()
        }),
        Err(error)=>json!({"error":error.to_string()}),
    };
    let terrain = evidence.diagnostics.terrain.get_or_insert_with(|| SystemState::new(world));
    let queries = terrain.get(world);
    let runtime = world.non_send::<Runtime>();
    let state = world.resource::<EmbeddedProjectState>();
    let gpu = world.get_resource::<GpuBladeGpuReadbackStats>().copied().unwrap_or_default();
    let render = world.get_resource::<GpuBladeRenderStats>().copied().unwrap_or_default();
    let content = runtime.previews.evidence(world, PreviewKind::Vegetation);
    let map = world.get_resource::<ActiveDynamicMap>().and_then(|active| active.map_id.as_ref());
    let active_canvases = content["canvases"].as_array().into_iter().flatten().take(4).map(|canvas| {
        let id = canvas["terrain_id"].as_str().and_then(|id| TerrainId::parse(id).ok());
        let entity = map.zip(id.as_ref()).and_then(|(map, id)| queries.as_ref().ok()?.instance(map, id));
        let root_source = id.is_none() || map.zip(id.as_ref()).is_some_and(|(map, id)|
            world.get_resource::<TerrainObjectAuthoringTarget>().is_some_and(|target| target.applied_to(map,id)));
        let draft = if root_source { world.get_resource::<DynamicBladeDraft>() } else {
            entity.and_then(|entity| world.get_resource::<InstanceResourceTable<DynamicBladeDraft>>()?.values.get(&entity))
        };
        // 状态属于实际运行实例；作者根资源可能只借给该实例，不能把容器Ready冒充其草窗Ready。
        let status = if id.is_none() { world.get_resource::<DynamicVegetationStatus>() } else {
            entity.and_then(|entity| world.get_resource::<InstanceResourceTable<DynamicVegetationStatus>>()?.values.get(&entity))
        };
        json!({"terrain_id":id.as_ref().map(TerrainId::as_str),"entity":entity.map(Entity::to_bits),
            "root_source":root_source,"draft_revision":map.and_then(|map|draft?.revision_for_map(map)),
            "layout_revision":map.and_then(|map|draft?.layout_revision_for_map(map)),
            "source_revision":map.and_then(|map|draft?.for_map(map)).map(|snapshot|snapshot.source_revision),
            "status":status.map(vegetation_status)})
    }).collect::<Vec<_>>();
    let report = json!({"source":"real-project-preview-wait","not_capture":true,
        "pid":std::process::id(),"world_id":state.world_id,"generation":runtime.transport.identity.generation,
        "frame":runtime.frames,"revision":state.revision,"mode":format!("{:?}",state.mode),
        "applied":runtime.previews.applied,"captured":evidence.done,"observed_kind":evidence.observed.as_ref().map(|identity|super::super::preview::kind_name(identity.kind)),
        "observed_sequence":evidence.observed.as_ref().map(|identity|identity.sequence),"stable_frames":evidence.stable,
        "gates":{"ready":runtime.ready,"failed":runtime.failed,"full_snapshot_pending":runtime.pending.is_some(),
            "screenshot_pending":evidence.pending.is_some(),"screenshot_writer":evidence.writer.is_some(),
            "scene_ready":super::super::systems::scene_ready(world),"source_matches_gpu":vegetation_source_ready(&content,gpu.source_revision),
            "fresh_gpu_sample":gpu.sample_sequence>evidence.sample_baseline,"sample_baseline":evidence.sample_baseline,
            "sources_ready":runtime.sources_ready,"sources_settled":runtime.sources_settled,"source_failed":runtime.source_failed,
            "height_sources_ready":world.get_resource::<super::super::systems::HeightSources>().is_some_and(|sources|sources.ready)},
        "scene":{"map":map.map(|map|map.as_str()),"phase":world.get_resource::<bevy::prelude::State<ScenePhase>>().map(|phase|format!("{:?}",phase.get())),
            "loading":world.contains_resource::<LoadingProgress>(),"transition":world.contains_resource::<PendingSceneTransition>(),
            "startup_ready":world.get_resource::<SceneStartupGate>().map(|gate|gate.ready),
            "map_status":world.get_resource::<DynamicMapRuntimeStatus>().map(debug_status),
            "placements":world.get_resource::<MapPlacementRuntimeStatus>().map(debug_status),
            "root_vegetation":world.get_resource::<DynamicVegetationStatus>().map(vegetation_status),
            "water":world.get_resource::<DynamicWaterStatus>().map(debug_status),
            "boundary":world.get_resource::<DynamicMapBoundaryStatus>().map(debug_status)},
        "content":content,"active_canvases":active_canvases,"view":view,"gpu":gpu_status(&gpu),"render":render_status(&render),
        "compute_stages":world.get_resource::<PipelineStages>().map(|stages|[
            "cull_tiles","finalize_tiles","classify_and_compact","finalize_indirect"].into_iter().zip(stages.0)
            .map(|(stage,code)|json!({"stage":stage,"code":code,"state":match code {0=>"not-installed",1=>"queued",2=>"creating",3=>"ready",4=>"shader-not-loaded",5=>"shader-import-unavailable",_=>"permanent-error"}})).collect::<Vec<_>>())});
    let directory = evidence.directory.clone();
    evidence.diagnostics.writer = Some(IoTaskPool::get().spawn(async move {
        let name = format!("{}-preview-wait",std::process::id());
        let temporary = directory.join(format!("{name}.pending.json"));
        std::fs::write(&temporary,serde_json::to_vec_pretty(&report).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
        std::fs::rename(temporary,directory.join(format!("{name}.json"))).map_err(|error|error.to_string())
    }));
}

fn debug_status(value: &impl std::fmt::Debug) -> String { format!("{value:?}").chars().take(2048).collect() }
fn vegetation_status(status: &DynamicVegetationStatus) -> Value {
    json!({"phase":format!("{:?}",status.phase),"map":status.map_id.as_ref().map(|map|map.as_str()),
        "revision":status.revision,"input_draft_revision":status.input_draft_revision,
        "applied_draft_revision":status.applied_draft_revision,"applied_density_revision":status.applied_density_revision,
        "layers":status.layer_count,"blade_layers":status.blade_layer_count,"blade_windows":status.blade_window_count,
        "mesh_instances":status.mesh_instance_count,"detail":status.detail.chars().take(512).collect::<String>()})
}
fn gpu_status(gpu: &GpuBladeGpuReadbackStats) -> Value {
    json!({"source_revision":gpu.source_revision,"sample_sequence":gpu.sample_sequence,"sampled_windows":gpu.sampled_windows,
        "truncated_windows":gpu.truncated_windows,"candidate_count":gpu.candidate_count,"active_tiles":gpu.active_tiles,
        "scanned_candidates":gpu.scanned_candidates,"distance_survivors":gpu.distance_survivors,
        "density_survivors":gpu.density_survivors,"surface_survivors":gpu.surface_survivors,"rendered_instances":gpu.rendered_instances,
        "rendered_instances_by_lod":gpu.rendered_instances_by_lod,"dropped_samples":gpu.dropped_samples,"map_errors":gpu.map_errors})
}
fn render_status(render: &GpuBladeRenderStats) -> Value {
    json!({"target_views":render.target_views,"missing_phase_views":render.missing_phase_views,"extracted_windows":render.extracted_windows,
        "input_ready_windows":render.input_ready_windows,"height_waiting_windows":render.height_waiting_windows,
        "density_waiting_windows":render.density_waiting_windows,"uniform_waiting_windows":render.uniform_waiting_windows,
        "prepared_bind_groups":render.prepared_bind_groups,"resident_reused_windows":render.resident_reused_windows,
        "compute_pipeline_ready_stages":render.compute_pipeline_ready_stages,"compute_pipeline_waiting_stages":render.compute_pipeline_waiting_stages,
        "compute_pipeline_error_stages":render.compute_pipeline_error_stages,"visible_windows":render.visible_windows,
        "queued_draws":render.queued_draws,"drawn_draws":render.drawn_draws,"pipeline_ready_draws":render.pipeline_ready_draws,
        "pipeline_waiting_draws":render.pipeline_waiting_draws,"pipeline_error_draws":render.pipeline_error_draws,
        "missing_mesh_instances":render.missing_mesh_instances,"missing_gpu_meshes":render.missing_gpu_meshes,
        "specialization_errors":render.specialization_errors})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vegetation_wait_evidence_cannot_promote_ack_or_old_gpu_to_readiness() {
        let mut content=json!({"canvases":[{"source_revision":179,"adopted_source_revision":179,"draft_revision":63}]});
        assert!(!vegetation_source_ready(&content,0));assert!(!vegetation_source_ready(&content,62));
        assert!(vegetation_source_ready(&content,63));
        content["canvases"][0]["adopted_source_revision"]=json!(178);
        assert!(!vegetation_source_ready(&content,63));
        let status=DynamicVegetationStatus{phase:feathrise_scene::DynamicVegetationPhase::Waiting,blade_window_count:0,
            detail:"实际高度尚未就绪".into(),..default()};
        let evidence=vegetation_status(&status);
        assert_eq!(evidence["phase"],"Waiting");assert_eq!(evidence["blade_windows"],0);
        assert_eq!(evidence["detail"],"实际高度尚未就绪");
    }
}
