//! 在绘制调用处按相机选择线框变体；原位移/裁切与内置作者面的真实边各走对应片元。
//!
//! 不给世界Mesh挂Wireframe，不复制场景实体，不改全局材质。每次正式绘制只做常数次查询；
//! 首次遇到某条实际绘制管线才异步派生其线模式，并以有界缓存复用。
use super::components::ShadingView;
use bevy::{
    core_pipeline::{
        core_3d::{AlphaMask3d, Opaque3d, Transparent3d},
        deferred::{AlphaMask3dDeferred, Opaque3dDeferred},
        prepass::{AlphaMask3dPrepass, Opaque3dPrepass},
    },
    pbr::{RenderMeshInstances, Transmissive3d},
    prelude::*,
    render::{
        RenderApp,
        mesh::RenderMesh,
        render_asset::RenderAssets,
        render_phase::{
            BinnedPhaseItem, CachedRenderPipelinePhaseItem, Draw, DrawError, DrawFunctions,
            TrackedRenderPass,
        },
        render_resource::{
            CachedRenderPipelineId, CompareFunction, FragmentState, PipelineCache, PolygonMode, PrimitiveTopology,
            WgpuFeatures,
        },
        renderer::RenderDevice,
    },
    shader::ShaderDefVal,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

const MAX_PIPELINE_VARIANTS: usize = 512; // 仅持有管线ID；GPU对象本身继续由Bevy管线缓存负责。
#[derive(Resource, Default)]
struct WirePipelines(
    Mutex<(
        HashMap<(CachedRenderPipelineId, bool, bool), CachedRenderPipelineId>,
        VecDeque<(CachedRenderPipelineId, bool, bool)>,
    )>,
);

/// 与原材质共用顶点布局和绑定；只在作者视口消费内置网格的UV1边界编码。
#[derive(Resource)]
pub(super) struct FaceWireShader(pub(super) Handle<Shader>);

pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    if !render
        .world()
        .resource::<RenderDevice>()
        .features()
        .contains(WgpuFeatures::POLYGON_MODE_LINE)
    {
        return;
    }
    render.init_resource::<WirePipelines>();
    wrap::<Opaque3d>(render.world_mut());
    wrap::<AlphaMask3d>(render.world_mut());
    // 只替换颜色通道会留下完整地形/物体的实心深度：地下边被挡住，天空也填不进面内。
    // 预通道同步采用真实线段，仍保留原位移、裁切和Deferred附件，玩法相机走原绘制。
    wrap::<Opaque3dPrepass>(render.world_mut());
    wrap::<AlphaMask3dPrepass>(render.world_mut());
    wrap::<Opaque3dDeferred>(render.world_mut());
    wrap::<AlphaMask3dDeferred>(render.world_mut());
    wrap::<Transparent3d>(render.world_mut());
    wrap::<Transmissive3d>(render.world_mut());
}

trait ShadingItem: CachedRenderPipelinePhaseItem + Sized {
    fn with_pipeline(&self, pipeline: CachedRenderPipelineId) -> Self;
}
macro_rules! binned_item {
    ($($ty:ty),*)=>{$(impl ShadingItem for $ty {
        fn with_pipeline(&self,pipeline:CachedRenderPipelineId)->Self {
            let mut key=self.batch_set_key.clone();key.pipeline=pipeline;
            Self::new(key,self.bin_key.clone(),self.representative_entity,self.batch_range.clone(),self.extra_index.clone())
        }
    })*};
}
binned_item!(
    Opaque3d,
    AlphaMask3d,
    Opaque3dPrepass,
    AlphaMask3dPrepass,
    Opaque3dDeferred,
    AlphaMask3dDeferred
);
macro_rules! sorted_item {
    ($($ty:ty),*)=>{$(impl ShadingItem for $ty {
        fn with_pipeline(&self,pipeline:CachedRenderPipelineId)->Self {
            Self {sorting_info:self.sorting_info,distance:self.distance,pipeline,entity:self.entity,draw_function:self.draw_function,
                batch_range:self.batch_range.clone(),extra_index:self.extra_index.clone(),indexed:self.indexed}
        }
    })*};
}
sorted_item!(Transparent3d, Transmissive3d);

struct ShadingDraw<P: ShadingItem> {
    inner: Box<dyn Draw<P>>,
}
impl<P: ShadingItem> Draw<P> for ShadingDraw<P> {
    fn prepare(&mut self, world: &World) {
        self.inner.prepare(world);
    }
    fn draw<'w>(
        &mut self,
        world: &'w World,
        pass: &mut TrackedRenderPass<'w>,
        view: Entity,
        item: &P,
    ) -> Result<(), DrawError> {
        let Some(settings) = world
            .get::<ShadingView>(view)
            .filter(|settings| settings.options.x == 0.0)
        else {
            return self.inner.draw(world, pass, view, item);
        };
        // 同一阶段可能含Gizmo、粒子等自定义draw；只有正式Mesh队列有几何线框变体。
        let Some(mesh) = world
            .resource::<RenderMeshInstances>()
            .render_mesh_queue_data(item.main_entity())
        else {
            return self.inner.draw(world, pass, view, item);
        };
        let cache = world.resource::<PipelineCache>();
        let original = item.cached_pipeline();
        if cache.get_render_pipeline(original).is_none() {
            return self.inner.draw(world, pass, view, item);
        } // 新入队源管线尚无可读descriptor，等待Bevy完成异步准备。
        let xray = settings.wire.x > 0.5;
        // binned绘制的render entity可能为PLACEHOLDER，身份必须从主世界Mesh队列恢复。
        // 私有属性随原资产上传；额外stride同时隔离普通UV1网格的管线和多重间接批次。
        // 编辑器多边形模式保留作者面边；游戏预览显式检查真实三角拓扑，沿原顶点管线显示每条边。
        let face_wire = settings.wire.z < 0.5 && world.resource::<RenderAssets<RenderMesh>>()
            .get(mesh.mesh_asset_id())
            .is_some_and(|mesh| mesh.layout.0.contains(crate::ATTRIBUTE_AUTHORING_WIRE));
        let key = (original, xray, face_wire);
        let pipelines = world.resource::<WirePipelines>();
        let mut variants = pipelines
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = if let Some(&id) = variants.0.get(&key) {
            id
        } else {
            let mut descriptor = cache.get_render_pipeline_descriptor(original).clone();
            if !matches!(
                descriptor.primitive.topology,
                PrimitiveTopology::TriangleList | PrimitiveTopology::TriangleStrip
            ) {
                drop(variants);
                return self.inner.draw(world, pass, view, item);
            }
            descriptor.label = Some("editor_view_wireframe".into());
            descriptor.primitive.polygon_mode = PolygonMode::Line;
            descriptor.primitive.cull_mode = None;
            if face_wire {
                let Some(shader) = world.get_resource::<FaceWireShader>() else {
                    drop(variants);
                    return self.inner.draw(world, pass, view, item);
                };
                if let Some(fragment) = descriptor.fragment.as_mut() {
                    fragment.shader = shader.0.clone();
                    fragment.entry_point = Some("fragment".into());
                } else {
                    // 纯深度预通道也需丢弃作者面内对角线，否则后处理仍会从残留深度画出它。
                    let mut defs = descriptor.vertex.shader_defs.clone();
                    defs.push(ShaderDefVal::Bool("EDITOR_WIRE_DEPTH_ONLY".into(), true));
                    descriptor.fragment = Some(FragmentState {
                        shader: shader.0.clone(),
                        shader_defs: defs,
                        entry_point: Some("fragment".into()),
                        targets: Vec::new(),
                        constants: Vec::new(),
                    });
                }
            }
            if xray {
                if let Some(depth) = descriptor.depth_stencil.as_mut() {
                    depth.depth_compare = Some(CompareFunction::Always);
                    // 穿透只放宽线段深度比较，保留源通道的写深度约定。
                    // Deferred要靠线段深度重建位置；天空也靠它避开已绘制的线，禁写会使边消失。
                }
            }
            let id = cache.queue_render_pipeline(descriptor);
            if variants.0.len() == MAX_PIPELINE_VARIANTS {
                if let Some(old) = variants.1.pop_front() {
                    variants.0.remove(&old);
                }
            }
            variants.0.insert(key, id);
            variants.1.push_back(key);
            id
        };
        drop(variants);
        // GPU异步编译期间保留源管线；不能阻塞Render线程等待新管线。
        if cache.get_render_pipeline(id).is_none() {
            return self.inner.draw(world, pass, view, item);
        }
        self.inner.draw(world, pass, view, &item.with_pipeline(id))
    }
}

struct SkipDraw;
impl<P: bevy::render::render_phase::PhaseItem> Draw<P> for SkipDraw {
    fn draw<'w>(
        &mut self,
        _: &'w World,
        _: &mut TrackedRenderPass<'w>,
        _: Entity,
        _: &P,
    ) -> Result<(), DrawError> {
        Ok(())
    }
}
fn wrap<P: ShadingItem>(world: &mut World) {
    let Some(functions) = world.get_resource::<DrawFunctions<P>>() else {
        return;
    };
    let mut functions = functions.write();
    for function in &mut functions.draw_functions {
        let inner = std::mem::replace(function, Box::new(SkipDraw));
        *function = Box::new(ShadingDraw { inner });
    }
}
