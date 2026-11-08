//! 地形目标观察与固定预算材质覆盖；稳定帧不遍历地形或复制材质。

use super::components::{TerrainBackfacesRenderMaterial, TerrainBackfacesTemporaryTag};
use super::{
    materials::BackfaceMaterial,
    resources::{MaterialTargets, Source, Target, TerrainBackfacesPreview},
};
use crate::{
    TerrainGridTarget,
    maps::dynamic_map::{ClipmapHeightMaterial, SparseClipmapSurfaceMaterial},
};
use bevy::mesh::MeshTag;
use bevy::pbr::{MaterialExtractionSystems, RenderMaterialInstances};
use bevy::prelude::*;
use bevy::render::{Extract, ExtractSchedule, RenderApp};
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;
use std::ops::Bound::{Excluded, Unbounded};

// 每种材质每帧最多64个小句柄切换和64份小材质更新；不复制Mesh、纹理或高度页。
const TARGETS_PER_FRAME: usize = 64;
const EVENTS_PER_FRAME: usize = 64;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<TerrainBackfacesPreview>();
    install_material::<StandardMaterial>(app);
    install_material::<TerrainSurfaceMaterial>(app);
    install_material::<ClipmapHeightMaterial>(app);
    install_material::<SparseClipmapSurfaceMaterial>(app);
    app.add_systems(
        PostUpdate,
        (
            sync::<StandardMaterial>,
            sync::<TerrainSurfaceMaterial>,
            sync::<ClipmapHeightMaterial>,
            sync::<SparseClipmapSurfaceMaterial>,
            publish_restoration,
        )
            .chain()
            .after(bevy::asset::AssetEventSystems), // 读取本帧源坐标/高度页更新，避免子地图拖动时多滞后一帧。
    );
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            ExtractSchedule,
            extract_backfaces
                .after(MaterialExtractionSystems)
                .before(bevy::pbr::late_sweep_material_instances),
        );
    }
}

fn install_material<M: BackfaceMaterial>(app: &mut App) {
    app.init_resource::<MaterialTargets<M>>()
        .add_observer(material_inserted::<M>)
        .add_observer(target_inserted::<M>)
        .add_observer(material_removed::<M>)
        .add_observer(target_removed::<M>);
}

fn register<M: BackfaceMaterial>(
    entity: Entity,
    material: &Handle<M>,
    state: &mut MaterialTargets<M>,
) {
    if let Some(target) = state.targets.get(&entity) {
        if target.source == *material
            || state
                .sources
                .get(&target.source.id())
                .and_then(|source| source.preview.as_ref())
                .is_some_and(|preview| preview == material)
        {
            return; // 重复同源登记不重新创建缓存；正式地表租约始终保留作者源身份。
        }
        state.remove(entity);
    }
    state
        .sources
        .entry(material.id())
        .or_insert(Source {
            users: 0,
            active: 0,
            preview: None,
        })
        .users += 1;
    state.targets.insert(
        entity,
        Target {
            source: material.clone(),
            overridden: false,
        },
    );
    state.enqueue(entity);
}

fn material_inserted<M: BackfaceMaterial>(
    event: On<Insert<MeshMaterial3d<M>>>,
    targets: Query<&MeshMaterial3d<M>, With<TerrainGridTarget>>,
    mut state: ResMut<MaterialTargets<M>>,
) {
    if let Ok(material) = targets.get(event.entity) {
        register(event.entity, &material.0, &mut state);
    }
}

fn target_inserted<M: BackfaceMaterial>(
    event: On<Insert<TerrainGridTarget>>,
    materials: Query<&MeshMaterial3d<M>>,
    mut state: ResMut<MaterialTargets<M>>,
) {
    if let Ok(material) = materials.get(event.entity) {
        register(event.entity, &material.0, &mut state);
    }
}

fn material_removed<M: BackfaceMaterial>(
    event: On<Remove<MeshMaterial3d<M>>>,
    mut state: ResMut<MaterialTargets<M>>,
) {
    state.remove(event.entity);
}

fn target_removed<M: BackfaceMaterial>(
    event: On<Remove<TerrainGridTarget>>,
    mut commands: Commands,
    mut state: ResMut<MaterialTargets<M>>,
    tags: Query<(&MeshTag, Option<&TerrainBackfacesTemporaryTag>)>,
) {
    if let Some(target) = state.targets.get(&event.entity) {
        commands
            .entity(event.entity)
            .try_insert(MeshMaterial3d(target.source.clone()))
            .try_remove::<TerrainBackfacesRenderMaterial>();
        if let Ok((tag, Some(_))) = tags.get(event.entity) {
            if tag.value == 0 {
                commands.entity(event.entity).try_remove::<MeshTag>();
            }
            commands
                .entity(event.entity)
                .try_remove::<TerrainBackfacesTemporaryTag>();
        }
    } // 单独撤销地形身份时归还原句柄；despawn的实体使用try_insert避免重复销毁警告。
    state.remove(event.entity);
}

fn sync<M: BackfaceMaterial>(
    preview: Res<TerrainBackfacesPreview>,
    mut commands: Commands,
    mut state: ResMut<MaterialTargets<M>>,
    mut assets: Option<ResMut<Assets<M>>>,
    mut events: Option<ResMut<Messages<AssetEvent<M>>>>,
    mut cursor: Local<bevy::ecs::message::MessageCursor<AssetEvent<M>>>,
    mut targets: Query<
        (
            &MeshMaterial3d<M>,
            Option<&mut MeshTag>,
            Option<&TerrainBackfacesRenderMaterial>,
            Option<&TerrainBackfacesTemporaryTag>,
        ),
        With<TerrainGridTarget>,
    >,
) {
    let Some(assets) = assets.as_mut() else {
        return;
    };
    if state.enabled != preview.enabled {
        state.enabled = preview.enabled;
        state.sweep = Some(None);
    }
    if let Some(events) = events.as_ref() {
        // 先按已登记源ID合并通知，再有界处理。不能延迟读取Bevy仅保留两帧的消息而漏掉热更。
        for event in cursor.read(&events) {
            let id = match event {
                AssetEvent::Added { id }
                | AssetEvent::Modified { id }
                | AssetEvent::LoadedWithDependencies { id } => *id,
                _ => continue,
            };
            if state.sources.contains_key(&id) && state.queued_sources.insert(id) {
                state.changed_sources.push_back(id);
            }
        }
    }
    for _ in 0..EVENTS_PER_FRAME {
        let Some(id) = state.changed_sources.pop_front() else {
            break;
        };
        state.queued_sources.remove(&id);
        let Some(source) = state.sources.get(&id) else {
            continue;
        };
        if source.active == 0 {
            if state.enabled {
                state.sweep = Some(None);
            }
            continue; // 关闭编辑器后停止刷新临时材质，避免玩法继续承担复制和GPU上传。
        }
        if let Some(handle) = source.preview.as_ref() {
            let Some(mut updated) = assets.get(id).cloned() else {
                continue;
            };
            updated.standard_mut().cull_mode = None;
            updated.standard_mut().double_sided = true;
            if let Some(current) = assets.get_mut_untracked(handle) {
                *current = updated;
                if let Some(events) = events.as_mut() {
                    events.write(AssetEvent::Modified { id: handle.id() });
                }
            }
        }
    }
    for _ in 0..TARGETS_PER_FRAME {
        let entity = if let Some(entity) = state.pending.pop_front() {
            state.queued.remove(&entity);
            entity
        } else if let Some(cursor) = state.sweep {
            let next = match cursor {
                Some(last) => state.targets.range((Excluded(last), Unbounded)).next(),
                None => state.targets.iter().next(),
            }
            .map(|(entity, _)| *entity);
            let Some(next) = next else {
                state.sweep = None;
                break;
            };
            state.sweep = Some(Some(next));
            next
        } else {
            break;
        };
        let Some(target) = state.targets.get(&entity) else {
            continue;
        };
        if target.overridden == state.enabled {
            continue;
        }
        let original = target.source.clone();
        let Ok((material, tag, current, temporary_tag)) = targets.get_mut(entity) else {
            state.remove(entity);
            continue;
        };
        if material.0 != original {
            register(entity, &material.0, &mut state);
            continue; // 下一次登记采用真正的新源；旧渲染覆盖不能跨材质换代沿用。
        }
        let displayed;
        if state.enabled {
            let handle = if let Some(handle) = state.sources[&original.id()].preview.clone() {
                if state.sources[&original.id()].active == 0 {
                    let Some(mut derived) = assets.get(&original).cloned() else {
                        continue;
                    };
                    derived.standard_mut().cull_mode = None;
                    derived.standard_mut().double_sided = true;
                    if let Some(current) = assets.get_mut_untracked(&handle) {
                        *current = derived;
                        if let Some(events) = events.as_mut() {
                            events.write(AssetEvent::Modified { id: handle.id() });
                        }
                    }
                } // 再次打开时从最新原材质刷新一次，不沿用关闭期间陈旧的参数。
                handle
            } else {
                let Some(mut derived) = assets.get(&original).cloned() else {
                    continue;
                };
                derived.standard_mut().cull_mode = None;
                derived.standard_mut().double_sided = true;
                let handle = assets.add(derived);
                if let Some(events) = events.as_mut() {
                    events.write(AssetEvent::Added { id: handle.id() });
                }
                // AssetEventSystems已收口，显式通知同帧渲染提取；原生Added下帧补发不改变资产身份。
                state.sources.get_mut(&original.id()).unwrap().preview = Some(handle.clone());
                handle
            };
            displayed = handle.id().untyped();
            state.targets.get_mut(&entity).unwrap().overridden = true;
            state.sources.get_mut(&original.id()).unwrap().active += 1;
            state.active += 1;
        } else {
            displayed = original.id().untyped();
            state.targets.get_mut(&entity).unwrap().overridden = false;
            state.sources.get_mut(&original.id()).unwrap().active -= 1;
            state.active -= 1;
        }
        commands
            .entity(entity)
            .insert(TerrainBackfacesRenderMaterial {
                original: original.id().untyped(),
                displayed,
                previous: current.map_or(original.id().untyped(), |current| current.displayed),
            });
        // Mesh3d变更会让Bevy扫描全部顶点重算AABB；只通知等价的MeshTag以更新GPU材质绑定。
        // 已有非零Tag保持原值；临时零Tag在恢复时移除，其他系统后来写入的Tag保留。
        if !state.enabled && temporary_tag.is_some() {
            if tag.as_ref().is_none_or(|tag| tag.value == 0) {
                commands.entity(entity).remove::<MeshTag>();
            } else if let Some(mut tag) = tag {
                tag.set_changed();
            }
            commands
                .entity(entity)
                .remove::<TerrainBackfacesTemporaryTag>();
        } else if let Some(mut tag) = tag {
            tag.set_changed();
        } else if state.enabled {
            commands
                .entity(entity)
                .insert((MeshTag::new(0), TerrainBackfacesTemporaryTag));
        }
    }
}

/// 与Bevy正式提取共用同一实体索引；只处理显示覆盖或可见性变化，不遍历所有地图实体。
fn extract_backfaces(
    mut instances: ResMut<RenderMaterialInstances>,
    mut dirty: ResMut<bevy::render::camera::DirtySpecializations>,
    targets: Extract<
        Query<
            (Entity, Ref<TerrainBackfacesRenderMaterial>, &ViewVisibility),
            Or<(
                Changed<TerrainBackfacesRenderMaterial>,
                Changed<ViewVisibility>,
            )>,
        >,
    >,
) {
    let tick = instances.current_change_tick;
    for (entity, material, visible) in &targets {
        if !visible.get() {
            continue;
        }
        let Some(instance) = instances
            .instances
            .get_mut(&bevy::render::sync_world::MainEntity::from(entity))
        else {
            continue;
        };
        if instance.asset_id != material.original
            && instance.asset_id != material.displayed
            && instance.asset_id != material.previous
        {
            continue;
        }
        // 防止刚换代的其他材质被旧覆盖污染；所有常规/prepass/shadow使用相同临时材质。
        instance.asset_id = material.displayed;
        instance.last_change_tick = tick;
        if material.is_changed() {
            dirty.changed_renderables.insert(bevy::render::sync_world::MainEntity::from(entity));
        } // 材质剔除状态属于专用管线缓存；仅刷新GPU实例仍会沿用旧单面管线。
    }
}

fn publish_restoration(
    mut preview: ResMut<TerrainBackfacesPreview>,
    standard: Res<MaterialTargets<StandardMaterial>>,
    surface: Res<MaterialTargets<TerrainSurfaceMaterial>>,
    height: Res<MaterialTargets<ClipmapHeightMaterial>>,
    sparse: Res<MaterialTargets<SparseClipmapSurfaceMaterial>>,
) {
    let active = standard.active + surface.active + height.active + sparse.active;
    if preview.active_targets != active {
        preview.active_targets = active;
    }
}
