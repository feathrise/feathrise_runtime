//! 动态地表首载和热更新共用的失败白模事务。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
use feathrise_environment::terrain_surface::{
    TerrainSurfaceCarrier, TerrainSurfaceFailure, TerrainSurfaceMaterial,
    TerrainSurfaceMaterialOverride, TerrainSurfacePhase, TerrainSurfaceRenderStats,
    TerrainSurfaceTextureProbe,
};

use super::components::DynamicSurfaceSourceWarmup;
use super::helpers::retained_active_surface;
use super::resources::DynamicSurfaceState;
use super::systems::SourceData;
// 失败恢复与安装系统共享唯一正式地形源身份。
use crate::DynamicSceneTerrainPrimitive;
use crate::scenes::dynamic_scene::clipmap::{
    ClipmapHeightMaterial, SparseClipmapSurfaceLease, SparseClipmapSurfaceMaterial,
}; // 新图基础高度材质与正式地表外层共同参与唯一源事务。
use crate::scenes::dynamic_scene::resources::{
    DynamicSceneRuntimePhase, DynamicSceneRuntimeState, DynamicSceneRuntimeStatus,
};

type CurrentSurfaceSource<'a> = Option<(
    Entity,
    Option<&'a MeshMaterial3d<StandardMaterial>>,
    Option<&'a MeshMaterial3d<ClipmapHeightMaterial>>,
    Option<&'a MeshMaterial3d<TerrainSurfaceMaterial>>,
    Option<&'a MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
    bool,
    Option<&'a TerrainSurfaceMaterialOverride>,
    Option<&'a SparseClipmapSurfaceLease>,
)>; // 实体、白模、扩展材质、Carrier和运行时替代租约组成常数大小的正式源快照。

/// 进入动态场景时同时开始主世界统计代次和唯一地表会话。
pub(crate) fn begin_dynamic_surface(
    mut state: InstanceResMut<DynamicSurfaceState>,
    mut stats: InstanceResMut<TerrainSurfaceRenderStats>,
) {
    stats.begin_generation(); // RenderApp回执从新代次重新计数。
    state.begin(stats.generation); // 场景状态与统计代次原子绑定。
}

/// 只读判断稳定源是否真的需要写修复；普通Active/白模帧不得反复触发Changed。
pub(super) fn surface_source_needs_reconcile(
    materials: &Assets<TerrainSurfaceMaterial>,
    clipmap_materials: &Assets<SparseClipmapSurfaceMaterial>,
    current: CurrentSurfaceSource<'_>,
    state: &DynamicSurfaceState,
    runtime: &DynamicSceneRuntimeState,
    stats: &TerrainSurfaceRenderStats,
) -> bool {
    let Some((
        entity,
        white,
        height_fallback,
        installed_surface,
        installed_clipmap,
        has_carrier,
        material_override,
        clipmap_lease,
    )) = current
    else {
        return state.source_entity.is_some(); // 只有旧源身份仍被状态持有时才进入换代空窗事务。
    };
    let same_source = state.source_entity == Some(entity); // 稳定实体身份比较不扫描世界。
    let active = state
        .effective_material()
        .filter(|handle| materials.get(handle.id()).is_some()); // 已被资产表回收的句柄不算稳定活动材质。
    let valid_installed_surface =
        installed_surface.filter(|handle| materials.get(handle.0.id()).is_some()); // 租约替代材质同样必须仍在资产表中。
    let active_stable = same_source
        && white.is_none()
        && height_fallback.is_none()
        && installed_clipmap.is_none()
        && has_carrier
        && retained_active_surface(active, valid_installed_surface, material_override); // 唯一扩展材质或精确租约路径完整且没有白模重复draw。
    let clipmap_stable = same_source
        && white.is_none()
        && height_fallback.is_none()
        && installed_surface.is_none()
        && !has_carrier
        && material_override.is_none()
        && installed_clipmap
            .filter(|handle| clipmap_materials.get(handle.0.id()).is_some())
            .zip(clipmap_lease)
            .zip(active)
            .is_some_and(|((installed, lease), active)| {
                lease.matches(active.id(), installed.0.id())
            }); // 外层材质必须精确包装当前内层作者资产，并从普通地表探针退出。
    if active_stable || clipmap_stable {
        return !state.reload_requested
            && state.warmup_entity.is_none()
            && (!runtime.surface_ready || stats.phase != TerrainSurfacePhase::Active); // 只在稳定证据自身丢失时补写一次。
    }
    let standard_stable = white
        .zip(state.white_material.as_ref())
        .is_some_and(|(installed, retained)| installed.0.id() == retained.id());
    let fallback_stable = height_fallback.is_some(); // 该组件只由Clipmap驻留系统安装并拥有，句柄身份即可证明兜底路径。
    let white_stable = same_source
        && state.effective_material().is_none()
        && installed_surface.is_none()
        && installed_clipmap.is_none()
        && !has_carrier
        && ((standard_stable && height_fallback.is_none()) || (white.is_none() && fallback_stable)); // Clipmap基础材质等价于白模外观，但必须保留GPU高度扩展。
    !white_stable // 其余源、材质或Carrier差异必须进入统一修复事务。
}

/// 每帧以唯一正式primitive核对源身份；GLB换代时优先转交仍有效的活动材质。
pub(super) fn reconcile_current_surface_source(
    commands: &mut Commands,
    materials: &Assets<TerrainSurfaceMaterial>,
    clipmap_materials: &Assets<SparseClipmapSurfaceMaterial>,
    current: CurrentSurfaceSource<'_>,
    state: &mut DynamicSurfaceState,
    runtime: &mut DynamicSceneRuntimeState,
    stats: &mut TerrainSurfaceRenderStats,
) {
    let Some((
        entity,
        white,
        height_fallback,
        installed_surface,
        installed_clipmap,
        _,
        material_override,
        clipmap_lease,
    )) = current
    else {
        if state.source_entity.take().is_some() && state.definition.is_some() {
            state.request_reload(); // Scene换代空窗使旧预热修订失效，新源出现后只重试一次。
            runtime.surface_ready = false; // 当前没有正式实体可证明活动材质已经承接。
            stats.set_phase(TerrainSurfacePhase::Loading);
        }
        return; // 稳定查询下一帧会自动发现新primitive，无需依赖一次性Changed。
    };
    let source_changed = state.source_entity != Some(entity);
    let previous_source = state.source_entity; // 材质转交成功后再清理旧源的渲染组件。
    if let Some(white) = white {
        state.white_material = Some(white.0.clone()); // 新GLB白模句柄用于真实失败时恢复。
    }
    if state
        .author_material
        .as_ref()
        .is_some_and(|handle| materials.get(handle.id()).is_none())
    {
        state.author_material = None; // 作者资产回收只放弃作者源，不能误清仍有效的文件源。
    }
    if state
        .active_material
        .as_ref()
        .is_some_and(|handle| materials.get(handle.id()).is_none())
    {
        state.active_material = None; // 文件资产独立核对。
    }
    let active = state
        .effective_material()
        .filter(|handle| materials.get(handle.id()).is_some())
        .cloned(); // 只有资产表仍持有的活动材质才允许跨实体复用。
    if state.active_material.is_some() && active.is_none() {
        state.active_material = None; // 已被回收的句柄不能掩盖真实材质失效。
    }
    let valid_installed_surface =
        installed_surface.filter(|handle| materials.get(handle.0.id()).is_some()); // 被回收的Deferred替代不能继续证明正式地表有效。
    let retained_surface =
        retained_active_surface(active.as_ref(), valid_installed_surface, material_override);
    let retained_height_fallback = height_fallback.is_some(); // 驻留系统会在高度页换代时原位刷新该资产。
    let retained_clipmap = installed_clipmap
        .filter(|handle| clipmap_materials.get(handle.0.id()).is_some())
        .zip(clipmap_lease)
        .zip(active.as_ref())
        .is_some_and(|((installed, lease), active)| lease.matches(active.id(), installed.0.id())); // 新图包装与当前活动作者材质一一对应才允许跨稳定帧保留。
    state.source_entity = Some(entity);
    if source_changed && let Some(previous_source) = previous_source {
        commands.entity(previous_source).try_remove::<(
            MeshMaterial3d<ClipmapHeightMaterial>,
            MeshMaterial3d<TerrainSurfaceMaterial>,
            MeshMaterial3d<SparseClipmapSurfaceMaterial>,
            TerrainSurfaceCarrier,
            TerrainSurfaceMaterialOverride,
            SparseClipmapSurfaceLease,
            DynamicSurfaceSourceWarmup,
        )>(); // 新实体接管前同批清除旧Scene上的正式draw路径和预热所有权。
    }
    if retained_clipmap {
        let mut source = commands.entity(entity);
        source
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
            .remove::<TerrainSurfaceMaterialOverride>()
            .remove::<TerrainSurfaceCarrier>(); // Clipmap 外层拥有唯一 draw，普通地表探针只验证采用前的内层候选。
        runtime.surface_ready = true;
        if !state.reload_requested && state.warmup_entity.is_none() {
            stats.set_phase(TerrainSurfacePhase::Active);
        }
    } else if retained_surface {
        let mut source = commands.entity(entity);
        source
            .remove::<MeshMaterial3d<StandardMaterial>>() // Scene热更重新插入的白模不能与正式材质形成第二个draw。
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .insert(TerrainSurfaceCarrier); // 防御性恢复RenderApp承接身份，继续使用已预热活动材质。
        runtime.surface_ready = true; // 同网格布局不变，已有管线、五纹理与局部参数表仍是完整正式结果。
        if !state.reload_requested && state.warmup_entity.is_none() {
            stats.set_phase(TerrainSurfacePhase::Active); // 无在途真实地表修订时恢复稳定活动证据。
        }
    } else if let Some(active) = active {
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>() // 新GLB白模在同一命令批次被正式活动材质替换。
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>() // 清除Scene可能插回的任意陈旧扩展材质。
            .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
            .remove::<TerrainSurfaceMaterialOverride>() // 场景重新接管时同步撤销失配或过期的运行时租约。
            .remove::<SparseClipmapSurfaceLease>()
            .insert((MeshMaterial3d(active), TerrainSurfaceCarrier)); // 稳定顶点布局允许直接把已预热材质转交给新地形实体。
        runtime.surface_ready = true; // 网格换代不需要重新验证未变化的纹理和Shader管线。
        if !state.reload_requested && state.warmup_entity.is_none() {
            stats.set_phase(TerrainSurfacePhase::Active); // 仅几何换代继续保持活动地表阶段。
        }
    } else if retained_height_fallback {
        if source_changed && state.definition.is_some() {
            state.request_reload(); // 新正式源继续预热地表，但等待期间保留已经可见的高度位移。
            runtime.surface_ready = false;
            stats.set_phase(TerrainSurfacePhase::Loading);
        }
    } else if state.definition.is_some() {
        if installed_surface.is_some()
            || installed_clipmap.is_some()
            || material_override.is_some()
            || clipmap_lease.is_some()
        {
            commands
                .entity(entity)
                .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
                .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
                .remove::<TerrainSurfaceMaterialOverride>()
                .remove::<SparseClipmapSurfaceLease>()
                .remove::<TerrainSurfaceCarrier>(); // 不匹配的旧扩展材质不得与白模并存等待新候选。
        }
        state.request_reload();
        runtime.surface_ready = false;
        stats.set_phase(TerrainSurfacePhase::Loading); // 活动材质确实失效时才重新走完整预热门。
    }
}

/// 完整取消一个预热事务：世界标记、离屏实体、状态句柄和候选资产必须同进同退。
pub(super) fn cancel_surface_warmup(
    commands: &mut Commands,
    materials: &mut Assets<TerrainSurfaceMaterial>,
    layer_tables: &mut Assets<ShaderBuffer>,
    state: &mut DynamicSurfaceState,
    warmup_entity: Entity,
    source_entity: Entity,
    retry: bool,
) {
    commands
        .entity(source_entity)
        .try_remove::<DynamicSurfaceSourceWarmup>(); // 旧源仍存在时释放single-flight所有权标记。
    commands.entity(warmup_entity).try_despawn(); // 离屏副本无论是否已被Scene级联销毁都幂等回收。
    if state.warmup_entity != Some(warmup_entity) {
        return; // 非当前事务不能误删当前候选材质或推进修订。
    }
    state.warmup_entity = None; // 先释放事务槽，随后重试才不会被“已有预热”永久挡住。
    state.warmup_source_entity = None; // 状态不再引用已取消的旧正式源。
    if let Some(candidate) = state.candidate_material.take() {
        materials.remove(candidate.id()); // 候选从未正式采用，不允许泄漏为孤立扩展材质。
    }
    if let Some(candidate) = state.candidate_layer_table.take() {
        layer_tables.remove(candidate.id()); // 层表与候选材质共享同一事务生命周期。
    }
    if retry && state.definition.is_some() {
        state.request_reload(); // 已有新修订时保持同一请求，否则只新增一次重试修订。
    }
}

/// 移除候选和正式扩展材质，并在源primitive仍存在时恢复GLB白色材质。
pub(super) fn restore_white_model(
    commands: &mut Commands,
    materials: &mut Assets<TerrainSurfaceMaterial>,
    layer_tables: &mut Assets<ShaderBuffer>,
    state: &mut DynamicSurfaceState,
    sources: &InstanceQuery<SourceData, With<DynamicSceneTerrainPrimitive>>,
) {
    if let Some(source_entity) = state.source_entity
        && sources.get(source_entity).is_ok()
    {
        let mut source = commands.entity(source_entity);
        source
            .remove::<MeshMaterial3d<ClipmapHeightMaterial>>()
            .remove::<MeshMaterial3d<TerrainSurfaceMaterial>>()
            .remove::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>()
            .remove::<TerrainSurfaceMaterialOverride>() // 真实失败回白必须撤销任何Deferred临时替代。
            .remove::<SparseClipmapSurfaceLease>()
            .remove::<TerrainSurfaceCarrier>()
            .remove::<DynamicSurfaceSourceWarmup>();
        if let Some(white) = state.white_material.clone() {
            source.insert(MeshMaterial3d(white));
        }
    }
    if let Some(source_entity) = state.warmup_source_entity.take() {
        commands
            .entity(source_entity)
            .try_remove::<DynamicSurfaceSourceWarmup>(); // GLB换代后也按冻结身份清理旧源标记。
    }
    if let Some(source_entity) = state.source_entity {
        commands.entity(source_entity).try_remove::<(
            TerrainSurfaceMaterialOverride,
            SparseClipmapSurfaceLease,
            MeshMaterial3d<ClipmapHeightMaterial>,
            MeshMaterial3d<SparseClipmapSurfaceMaterial>,
        )>(); // 查询换代空窗中也撤销可能仍存活的运行时租约。
    }
    if let Some(entity) = state.warmup_entity.take() {
        commands.entity(entity).try_despawn();
    }
    if let Some(candidate) = state.candidate_material.take() {
        materials.remove(candidate.id());
    }
    if let Some(active) = state.active_material.take() {
        materials.remove(active.id());
    }
    if let Some(candidate) = state.candidate_layer_table.take() {
        layer_tables.remove(candidate.id());
    }
    if let Some(active) = state.active_layer_table.take() {
        layer_tables.remove(active.id());
    }
    state.reload_requested = false;
    state.author_material = None; // 放弃作者覆盖但不删除编辑器持有的资产。
    state.white_requested = false;
}

/// 任一当前修订硬门失败时移除新旧正式材质并恢复原GLB白模。
#[allow(clippy::too_many_arguments)]
pub(super) fn fail_current_revision(
    commands: &mut Commands,
    materials: &mut Assets<TerrainSurfaceMaterial>,
    layer_tables: &mut Assets<ShaderBuffer>,
    state: &mut DynamicSurfaceState,
    runtime: &mut DynamicSceneRuntimeState,
    status: &mut DynamicSceneRuntimeStatus,
    stats: &mut TerrainSurfaceRenderStats,
    sources: &InstanceQuery<SourceData, With<DynamicSceneTerrainPrimitive>>,
    failure: TerrainSurfaceFailure,
) {
    if state.author_material.is_some() {
        if let (Some(entity), Some(source)) = (state.warmup_entity, state.warmup_source_entity) {
            cancel_surface_warmup(
                commands,
                materials,
                layer_tables,
                state,
                entity,
                source,
                false,
            ); // 保存文件失败只取消文件候选，不能销毁完整作者画面。
        }
        state.reload_requested = false; // 等真实文件修复事件再重试，稳定帧不反复构建失败候选。
        runtime.surface_ready = true; // 当前作者源仍完整可用。
        status.detail = format!("动态场景保存地表{failure:?}失败，保留未保存预览"); // 对外明确文件未完成交接。
        stats.set_failure(failure); // 失败证据保留，禁止误报文件Ready。
        return; // 编辑器拥有的材质与表不能由文件失败事务删除。
    }
    restore_white_model(commands, materials, layer_tables, state, sources);
    if let (Some(textures), Some(probe)) = (state.textures.as_ref(), state.texture_probe) {
        commands
            .entity(probe)
            .insert(TerrainSurfaceTextureProbe::failed(textures.images()));
    }
    runtime.surface_ready = false;
    status.phase = DynamicSceneRuntimePhase::Failed;
    status.detail = format!("动态场景地表{failure:?}失败，已恢复白模");
    stats.set_failure(failure);
    if state.failure_logged_revision != Some(state.revision) {
        error!(
            "动态场景地表失败并恢复白模：generation={} revision={} item={failure:?}",
            state.generation, state.revision
        );
        state.failure_logged_revision = Some(state.revision);
    }
}

/// 离开动态场景时释放探针、预热事务、候选、活动材质和全部强图片句柄。
pub(crate) fn clear_dynamic_surface(
    mut commands: Commands,
    mut state: InstanceResMut<DynamicSurfaceState>,
    mut materials: ResMut<Assets<TerrainSurfaceMaterial>>,
    mut layer_tables: ResMut<Assets<ShaderBuffer>>,
    mut stats: InstanceResMut<TerrainSurfaceRenderStats>,
) {
    if let Some(entity) = state.texture_probe.take() {
        commands.entity(entity).try_despawn(); // RenderApp纹理探针只属于当前动态场景会话。
    }
    if let Some(entity) = state.warmup_entity.take() {
        commands.entity(entity).try_despawn(); // 未采用离屏候选不得跨场景存活。
    }
    if let Some(source_entity) = state.warmup_source_entity.take() {
        commands
            .entity(source_entity)
            .try_remove::<DynamicSurfaceSourceWarmup>(); // OnExit同样释放可能仍存活旧源的事务标记。
    }
    if let Some(source_entity) = state.source_entity {
        commands.entity(source_entity).try_remove::<(
            TerrainSurfaceMaterialOverride,
            SparseClipmapSurfaceLease,
            MeshMaterial3d<ClipmapHeightMaterial>,
            MeshMaterial3d<SparseClipmapSurfaceMaterial>,
        )>(); // 离场先撤销全部材质租约，不能覆盖下一场景内容。
    }
    if let Some(handle) = state.candidate_material.take() {
        materials.remove(handle.id()); // 未采用候选资产随事务共同回收。
    }
    if let Some(handle) = state.active_material.take() {
        materials.remove(handle.id()); // 活动材质不能泄漏到下一场景代次。
    }
    if let Some(handle) = state.candidate_layer_table.take() {
        layer_tables.remove(handle.id());
    }
    if let Some(handle) = state.active_layer_table.take() {
        layer_tables.remove(handle.id());
    }
    *state = DynamicSurfaceState::default(); // 一次清空全部场景、纹理和修订身份。
    stats.clear(); // 主世界与RenderApp地表统计恢复未启用状态。
}

#[cfg(test)]
mod tests;
