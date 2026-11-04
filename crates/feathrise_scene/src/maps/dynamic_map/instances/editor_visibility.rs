//! 编辑器显隐只改变独立地形根的显示；源、LOD和碰撞继续存在，退出编辑后恢复显示。
use bevy::prelude::*;
use feathrise_map_content::{MapId, MapInstancePath, TerrainId};
use std::collections::BTreeSet;
use std::ops::Bound::{Excluded, Unbounded};

#[derive(Resource, Default)]
pub struct TerrainObjectEditorVisibility {
    pub map: Option<MapId>,
    pub hidden: BTreeSet<TerrainId>,
}

pub(super) fn apply(
    state: Res<TerrainObjectEditorVisibility>,
    runtime: Res<super::resources::SubMapRuntime>,
    mut cursor: Local<Option<MapInstancePath>>,
    mut roots: Query<&mut Visibility, With<super::TerrainObjectInstance>>,
) {
    let Some(batch) = &runtime.active else {
        *cursor = None;
        return;
    };
    // 所有对象共享128次小组件写预算；有序游标避免大量对象时反复遍历前缀。
    for _ in 0..128 {
        let next = if let Some(after) = cursor.as_ref() {
            batch.index.range((Excluded(after), Unbounded)).next()
        } else {
            batch.index.iter().next()
        };
        let Some((path, entity)) = next else {
            *cursor = None;
            break;
        };
        if let Some(id) = &path.terrain_id
            && let Ok(mut visibility) = roots.get_mut(*entity)
        {
            let hidden = path.instances.is_empty()
                && state.map.as_ref() == Some(&path.root_map_id)
                && state.hidden.contains(id);
            let next = if hidden {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if *visibility != next {
                *visibility = next;
            }
        }
        *cursor = Some(path.clone());
    }
}
