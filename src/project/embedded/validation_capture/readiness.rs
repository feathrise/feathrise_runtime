//! 仅显式验收安装的RenderApp回执；先核对实际提取视图及GPU管线，再允许窗口截图。
//!
//! 一次Render之后发布固定小快照，下次Extract交回MainWorld。没有跨线程等待、生产渲染改写或
//! “多睡几帧就成功”；最多检查2048条缓存，超出预算明确拒绝取证，错误/编译中状态随报告保留。
use bevy::{
    core_pipeline::prepass::ViewPrepassTextures,
    prelude::*,
    render::{
        ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems,
        camera::ExtractedCamera,
        render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor},
        sync_world::{MainEntity, RenderEntity},
        view::{ExtractedView, ViewTarget},
    },
};
use feathrise_scene::author_view::{GridView, ShadingView};
use serde_json::{Value, json};

const MAX_PIPELINES: usize = 2048;
const MAX_ERRORS: usize = 8;

#[derive(Resource, Clone, PartialEq)]
struct Request {
    ticket: u64,
    camera: Entity,
    render_camera: Entity,
    shading: Option<ShadingView>,
    grid: Option<GridView>,
    msaa: Msaa,
    size: UVec2,
    camera_world: Mat4,
    clip_from_view: Mat4,
}

#[derive(Clone, Default, PartialEq)]
struct Inventory {
    total: usize,
    waiting: usize,
    queued: usize,
    creating: usize,
    errors: Vec<String>,
    overflow: bool,
    solid: usize,
    wire: usize,
    grid: usize,
}
impl Inventory {
    fn ready_for(&self, request: &Request) -> bool {
        let needs_solid = request.shading.is_some_and(|view| view.options.x <= 1. || view.options.w <= 0.5);
        let needs_wire = request.shading.is_some_and(|view| view.options.x == 0.);
        !self.overflow && self.total > 0 && self.waiting == 0 && self.queued == 0 && self.creating == 0
            && self.errors.is_empty() && (!needs_solid || self.solid > 0)
            && (!needs_wire || self.wire > 0) && (request.grid.is_none() || self.grid > 0)
    }
    fn report(&self) -> Value {
        json!({"total":self.total,"waiting":self.waiting,"queued":self.queued,"creating":self.creating,
            "errors":self.errors,"overflow":self.overflow,"limit":MAX_PIPELINES,
            "ready_author_pipelines":{"solid":self.solid,"wire":self.wire,"grid":self.grid}})
    }
}

#[derive(Resource, Clone, Default)]
struct Observation {
    request: Option<Request>,
    inventory: Inventory,
    view_matches: bool,
    settled: u8,
}
impl Observation {
    fn advance(&mut self, request: Request, inventory: Inventory, view_matches: bool) {
        // draw阶段才入队的新变体在下一次process_queue才进入公开缓存。要求两次实际Render的
        // 相同身份与完整管线清单均就绪，避免首次空队列或历史同名视图回执被当成新变体已经提交。
        let unchanged = self.request.as_ref() == Some(&request) && self.inventory == inventory;
        self.settled = if view_matches && inventory.ready_for(&request) {
            if unchanged { self.settled.saturating_add(1) } else { 1 }
        } else { 0 };
        self.request = Some(request);
        self.inventory = inventory;
        self.view_matches = view_matches;
    }
    fn ready_for(&self, request: &Request) -> bool {
        self.request.as_ref() == Some(request) && self.view_matches
            && self.inventory.ready_for(request) && self.settled >= 2
    }
}

pub(in crate::project::embedded) struct ReadinessPlugin;
impl Plugin for ReadinessPlugin {
    fn build(&self, app: &mut App) { app.init_resource::<Observation>(); }
    fn finish(&self, app: &mut App) {
        // prepare早于DefaultPlugins；finish时才有RenderApp，普通构建不会安装这个插件。
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.init_resource::<Observation>()
                .add_systems(ExtractSchedule, exchange)
                .add_systems(Render, observe.in_set(RenderSystems::Cleanup));
        }
    }
}

fn exchange(world: &mut World) {
    let observation = world.resource::<Observation>().clone();
    let request = {
        let mut main = world.resource_mut::<MainWorld>();
        main.insert_resource(observation);
        main.get_resource::<Request>().cloned()
    };
    if let Some(request) = request { world.insert_resource(request); }
    else { world.remove_resource::<Request>(); }
}

fn inventory(cache: &PipelineCache) -> Inventory {
    let mut result = Inventory {
        waiting: cache.waiting_pipelines().take(MAX_PIPELINES + 1).count(),
        ..default()
    };
    for pipeline in cache.pipelines().take(MAX_PIPELINES + 1) {
        result.total += 1;
        if result.total > MAX_PIPELINES { result.overflow = true; break; }
        let label = match &pipeline.descriptor {
            PipelineDescriptor::RenderPipelineDescriptor(value) => value.label.as_deref(),
            PipelineDescriptor::ComputePipelineDescriptor(value) => value.label.as_deref(),
            PipelineDescriptor::MeshPipelineDescriptor(value) => value.label.as_deref(),
        }.unwrap_or("unnamed");
        match &pipeline.state {
            CachedPipelineState::Queued => result.queued += 1,
            CachedPipelineState::Creating(_) => result.creating += 1,
            CachedPipelineState::Err(error) => {
                if result.errors.len() < MAX_ERRORS {
                    result.errors.push(format!("{label}: {error}").chars().take(1024).collect());
                }
            }
            CachedPipelineState::Ok(_) => match label {
                "editor_solid_pipeline" => result.solid += 1,
                "editor_view_wireframe" => result.wire += 1,
                "editor_infinite_grid" => result.grid += 1,
                _ => {}
            },
        }
    }
    result.overflow |= result.waiting > MAX_PIPELINES;
    result
}

fn observe(
    request: Option<Res<Request>>,
    cache: Res<PipelineCache>,
    cameras: Query<(&MainEntity, &ExtractedCamera, &Msaa, Option<&ShadingView>, Option<&GridView>,
        &ViewTarget, Option<&ViewPrepassTextures>, &ExtractedView)>,
    mut observation: ResMut<Observation>,
) {
    let Some(request) = request else { *observation = default(); return; };
    let view_matches = cameras.get(request.render_camera).is_ok_and(|(main, camera, msaa, shading, grid, target, prepass, view)| {
        let needs_prepass = request.shading.is_some_and(|view|view.options.x<=1. || view.options.w<=0.5);
        let attachments_match = !needs_prepass || prepass.is_some_and(|prepass| {
            prepass.normal.as_ref().zip(prepass.depth.as_ref()).is_some_and(|(normal,depth)| {
                let normal = &normal.texture.texture; let depth = &depth.texture.texture;
                normal.sample_count()==1 && depth.sample_count()==1
                    && normal.size()==depth.size() && normal.size()==target.main_texture().size()
            })
        });
        main.id() == request.camera && *msaa == request.msaa
            && camera.physical_viewport_size == Some(request.size)
            && shading.copied() == request.shading && grid.copied() == request.grid
            && view.world_from_view.to_matrix() == request.camera_world
            && view.clip_from_view == request.clip_from_view
            && attachments_match
    });
    observation.advance(request.clone(), inventory(&cache), view_matches);
}

pub(in crate::project::embedded) fn poll_readiness(world: &mut World, ticket: u64) -> (bool, Value) {
    let camera = world.query_filtered::<Entity, With<feathrise_scene_core::view::PrimarySceneView>>()
        .single(world).ok();
    let Some(camera) = camera else {
        world.remove_resource::<Request>();
        return (false, json!({"ready":false,"reason":"唯一正式相机尚未建立","ticket":ticket}));
    };
    let Some(render_camera) = world.get::<RenderEntity>(camera).map(RenderEntity::id) else {
        return (false, json!({"ready":false,"reason":"相机尚未登记RenderEntity","ticket":ticket}));
    };
    let Some(size) = world.get::<Camera>(camera).and_then(Camera::physical_viewport_size) else {
        return (false, json!({"ready":false,"reason":"相机物理视口尚未完成","ticket":ticket}));
    };
    let Some(camera_world) = world.get::<GlobalTransform>(camera).map(GlobalTransform::to_matrix) else {
        return (false,json!({"ready":false,"reason":"相机世界变换尚未建立","ticket":ticket}));
    };
    let request = Request { ticket, camera, render_camera, size,
        camera_world, clip_from_view: world.get::<Camera>(camera).unwrap().clip_from_view(),
        shading: world.get::<ShadingView>(camera).copied(), grid: world.get::<GridView>(camera).copied(),
        msaa: world.get::<Msaa>(camera).copied().unwrap_or_default() };
    let observation = world.resource::<Observation>();
    let ready = observation.ready_for(&request);
    let report = json!({"ready":ready,"ticket":ticket,"render_ticket":observation.request.as_ref().map(|value|value.ticket),
        "view_matches":observation.view_matches,"stable_render_submissions":observation.settled,
        "camera":camera.to_bits(),"render_camera":render_camera.to_bits(),"size":size.to_array(),
        "camera_world":request.camera_world.to_cols_array(),"clip_from_view":request.clip_from_view.to_cols_array(),
        "pipelines":observation.inventory.report(),"source":"RenderApp-after-RenderSystems-Render"});
    world.insert_resource(request);
    (ready, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request_for(ticket: u64) -> Request {
        Request { ticket, camera: Entity::PLACEHOLDER, render_camera: Entity::PLACEHOLDER,
            shading: Some(ShadingView { options: Vec4::new(0.,1.,0.,1.),wire: Vec4::X }),
            grid: None, msaa: Msaa::Off, size: UVec2::new(800,600),camera_world:Mat4::IDENTITY,clip_from_view:Mat4::IDENTITY }
    }
    fn prepared() -> Inventory { Inventory { total:3,solid:1,wire:1,..default() } }
    #[test]
    fn render_readiness_rejects_old_view_queued_pipeline_and_late_new_variants() {
        let request = request_for(3);
        let mut observation = Observation::default();
        observation.advance(request.clone(),prepared(),true);
        assert!(!observation.ready_for(&request));
        observation.advance(request.clone(),prepared(),true);
        assert!(observation.ready_for(&request));
        assert!(!observation.ready_for(&request_for(4)), "历史同名视图不得授权新取证");
        let mut changed_view = request.clone();
        changed_view.shading.as_mut().unwrap().wire.x = 0.;
        assert!(!observation.ready_for(&changed_view), "同票据也不能借用旧四球参数回执");
        let mut moved = request.clone(); moved.camera_world = Mat4::from_translation(Vec3::X);
        assert!(!observation.ready_for(&moved),"尚未提取的新相机位置不能借用旧渲染回执");
        for pending in [Inventory{waiting:1,..prepared()},Inventory{queued:1,..prepared()},
            Inventory{creating:1,..prepared()},Inventory{errors:vec!["shader failure".into()],..prepared()},
            Inventory{overflow:true,..prepared()}, Inventory{wire:0,..prepared()}] {
            observation.advance(request.clone(),pending,true);
            assert!(!observation.ready_for(&request));
        }
        observation.advance(request.clone(),prepared(),true);
        observation.advance(request.clone(),prepared(),true);
        let changed = Inventory{total:4,wire:2,..prepared()};
        observation.advance(request.clone(),changed.clone(),true);
        assert!(!observation.ready_for(&request), "新变体首次入缓存仍需实际Render确认");
        observation.advance(request.clone(),changed,true);
        assert!(observation.ready_for(&request));
        observation.advance(request.clone(),prepared(),false);
        assert!(!observation.ready_for(&request), "RenderWorld实际视图不匹配时不得仅信CPU参数");
    }
}
