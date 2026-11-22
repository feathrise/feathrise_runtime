//! 给隐藏摆放候选冻结同一场景实例的地表，供后台解析源 GLB 子节点落点。

use super::{
    ClipmapGroundSnapshot, TerrainObjectQueries, TerrainObjectRaySnapshot, TerrainSurfaceVersion,
};
use bevy::prelude::*;
use feathrise_scene_content::{SceneId, SceneInstancePath, SceneTerrainObject, TerrainId};
use feathrise_scene_core::residency::SceneSourceFrame;
use std::sync::Arc;

pub(crate) enum PlacementGroundSources {
    Objects(Arc<[SceneTerrainObject]>),
    Legacy,
    Empty,
}

pub(crate) enum PlacementGroundSurface {
    Pending,
    Empty,
    Ready(TerrainObjectRaySnapshot, TerrainSurfaceVersion),
}

impl TerrainObjectQueries<'_, '_> {
    pub(crate) fn placement_ground_sources(
        &self,
        owner: &SceneId,
        scope: Option<Entity>,
    ) -> Option<PlacementGroundSources> {
        let registry = match scope {
            Some(scope) => self.sources.as_ref()?.values.get(&scope)?,
            None => self.root_content.as_deref()?,
        };
        let manifest = self
            .manifests
            .as_ref()?
            .get(&registry.handles(owner)?.manifest)?;
        Some(match &manifest.terrains {
            Some(objects) if !objects.is_empty() => {
                PlacementGroundSources::Objects(objects.clone())
            }
            Some(_) => PlacementGroundSources::Empty,
            None if !manifest.terrain.is_empty() => PlacementGroundSources::Legacy,
            None => PlacementGroundSources::Empty,
        })
    }

    pub(crate) fn placement_ground_surface(
        &self,
        owner: &SceneId,
        scope: Option<Entity>,
        terrain: Option<&TerrainId>,
    ) -> PlacementGroundSurface {
        let Some(terrain) = terrain else {
            return self.legacy_placement_surface(owner, scope);
        };
        let (path, entity) = if let Some(scope) = scope {
            let Ok(context) = self.contexts.get(scope) else {
                return PlacementGroundSurface::Empty;
            };
            if &context.source_scene_id != owner {
                return PlacementGroundSurface::Empty;
            }
            let path = context.path.clone().terrain(terrain.clone());
            let entity = self
                .lookup
                .as_ref()
                .and_then(|lookup| lookup.instance(context.batch, &path));
            let Some(entity) = entity else {
                return PlacementGroundSurface::Pending;
            };
            if !self.contexts.get(entity).is_ok_and(|instance| {
                instance.batch == context.batch && instance.generation == context.generation
            }) {
                return PlacementGroundSurface::Pending;
            }
            (path, entity)
        } else {
            let path = SceneInstancePath::root(owner.clone()).terrain(terrain.clone());
            let Some(entity) = self
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.preparing_terrain(&path))
            else {
                return PlacementGroundSurface::Pending;
            };
            (path, entity)
        };
        let Ok((instance, object, frame)) = self.definitions.get(entity) else {
            return PlacementGroundSurface::Pending;
        };
        if &instance.source_scene_id != owner
            || instance.path != path
            || &object.definition.id != terrain
        {
            return PlacementGroundSurface::Pending;
        }
        if !object.definition.enabled {
            return PlacementGroundSurface::Empty;
        }
        if let Some((snapshot, version)) = self.object_surface(entity, owner, *frame) {
            return PlacementGroundSurface::Ready(snapshot, version);
        }
        let complete = self
            .scenes
            .as_ref()
            .and_then(|table| table.values.get(&entity))
            .is_some_and(|map| {
                map.active_scene_id.as_ref() == Some(owner)
                    && map.collision_ready
                    && map.sparse_height_path.is_none()
            });
        if complete {
            PlacementGroundSurface::Empty
        } else {
            PlacementGroundSurface::Pending
        }
    }

    fn legacy_placement_surface(
        &self,
        owner: &SceneId,
        scope: Option<Entity>,
    ) -> PlacementGroundSurface {
        let (map, surface, frame) = if let Some(scope) = scope {
            let map = self
                .scenes
                .as_ref()
                .and_then(|table| table.values.get(&scope));
            let surface = self
                .ground
                .as_ref()
                .and_then(|table| table.values.get(&scope));
            let Ok(frame) = self.context_frames.get(scope) else {
                return PlacementGroundSurface::Pending;
            };
            (map, surface, *frame)
        } else {
            (
                self.root_scene.as_deref(),
                self.root_ground.as_deref(),
                SceneSourceFrame::default(),
            )
        };
        let Some(map) = map.filter(|map| map.active_scene_id.as_ref() == Some(owner)) else {
            return PlacementGroundSurface::Pending;
        };
        let Some(root) = map.root else {
            return PlacementGroundSurface::Pending;
        };
        if let Some(snapshot) = surface
            .and_then(|surface| surface.snapshot.as_ref())
            .filter(|snapshot| &snapshot.scene_id == owner)
        {
            let (snapshot, version) = clipmap_surface(root, snapshot, frame);
            return PlacementGroundSurface::Ready(snapshot, version);
        }
        if let Some((snapshot, version)) =
            self.collider_surface(root, map.terrain_display.unwrap_or(root), frame)
        {
            return PlacementGroundSurface::Ready(snapshot, version);
        }
        if map.collision_ready && map.sparse_height_path.is_none() {
            PlacementGroundSurface::Empty
        } else {
            PlacementGroundSurface::Pending
        }
    }
}

pub(super) fn clipmap_surface(
    entity: Entity,
    snapshot: &ClipmapGroundSnapshot,
    frame: SceneSourceFrame,
) -> (TerrainObjectRaySnapshot, TerrainSurfaceVersion) {
    let version = TerrainSurfaceVersion {
        entity,
        collider: None,
        content: Some((snapshot.content_revision_words(), snapshot.ready)),
        query: Some((
            snapshot.revision,
            snapshot.missing_pages,
            snapshot.authoritative_page_revision(),
        )),
        frame: frame.source_to_world,
    };
    (
        TerrainObjectRaySnapshot::Clipmap(snapshot.clone(), frame),
        version,
    )
}
