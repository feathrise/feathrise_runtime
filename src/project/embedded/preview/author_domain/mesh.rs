//! 有界点边面幽灵沿用原构网格与材质；正式根只临时遮蔽，碰撞及玩法状态不改。
use bevy::prelude::*;
use feathrise_map_content::{
    MapId, MapPlacementAssetTarget, embedded_author_preview::MeshPreview,
};
use feathrise_scene::MapPlacementInstance;
pub(super) struct Prepared {
    map: MapId,
    revision: u64,
    value: Option<(MeshPreview, Mesh, StandardMaterial, Transform)>,
    committed: bool,
}
pub(super) fn prepare(
    map: MapId,
    revision: u64,
    value: Option<MeshPreview>,
    committed: bool,
) -> Result<Prepared, String> {
    let value = value
        .map(|value| -> Result<_, String> {
            if value.placement_id.is_empty() || value.placement_id.len() > 256 {
                return Err("网格预览对象身份无效".into());
            }
            let geometry = value
                .target
                .editable_geometry()
                .ok_or("网格预览不是可编辑内置几何")?;
            geometry.validate()?;
            let matrix = Mat4::from_cols_array(&value.local);
            let transform = Transform::from_matrix(matrix);
            if !matrix.is_finite()
                || matrix.determinant().abs() < 1e-12
                || !transform.to_matrix().abs_diff_eq(matrix, 0.0005)
            {
                return Err("网格预览局部姿态不是有效TRS".into());
            }
            let mut mesh = feathrise_scene::editable_mesh_mesh(geometry);
            if let Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)) =
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
            {
                let colors = normals
                    .iter()
                    .map(|normal| {
                        let shade =
                            (0.70 + 0.20 * normal[1] - 0.10 * normal[0].abs()).clamp(0.40, 1.0);
                        [shade, shade, shade, 1.0]
                    })
                    .collect::<Vec<_>>();
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            }
            let color = match value.target {
                MapPlacementAssetTarget::Cuboid { color_srgb, .. }
                | MapPlacementAssetTarget::EditableCuboid { color_srgb, .. }
                | MapPlacementAssetTarget::EditableMesh { color_srgb, .. } => color_srgb,
                _ => return Err("网格预览颜色来源不兼容".into()),
            };
            Ok((
                value,
                mesh,
                StandardMaterial {
                    base_color: Color::srgb_u8(color[0], color[1], color[2]),
                    unlit: true,
                    ..default()
                },
                transform,
            ))
        })
        .transpose()?;
    Ok(Prepared {
        map,
        revision,
        value,
        committed,
    })
}
#[derive(Component)]
struct OwnedVisibility(u64);
fn external_visibility(event: On<Insert, Visibility>, mut commands: Commands) {
    // 临时显隐直接改组件值；外部显式插入哪怕同值也撤销旧手势归还权。
    commands.entity(event.entity).remove::<OwnedVisibility>();
}
pub(super) fn install(app: &mut App) {
    app.add_observer(external_visibility);
}
struct Active {
    map: MapId,
    id: String,
    root: Entity,
    parent: Option<Entity>,
    ghost: Entity,
    original: Visibility,
    ticket: u64,
    target: MapPlacementAssetTarget,
    committed: bool,
    handoff: bool,
}
#[derive(Default)]
pub(super) struct State {
    active: Option<Active>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    ticket: u64,
    #[cfg(feature = "validation")]
    revision: u64,
}
impl State {
    fn release(&mut self, world: &mut World) {
        let Some(active) = self.active.take() else {
            return;
        };
        let owned = world
            .get::<OwnedVisibility>(active.root)
            .is_some_and(|owner| owner.0 == active.ticket)
            && world
                .get::<MapPlacementInstance>(active.root)
                .is_some_and(|root| {
                    root.is_active() && root.map_id() == &active.map && root.id() == active.id
                })
            && world.get::<ChildOf>(active.root).map(ChildOf::parent) == active.parent;
        if owned {
            if world.get::<Visibility>(active.root) == Some(&Visibility::Hidden) {
                *world.get_mut::<Visibility>(active.root).unwrap() = active.original;
            }
            world.entity_mut(active.root).remove::<OwnedVisibility>();
        }
        if let Ok(entity) = world.get_entity_mut(active.ghost) {
            entity.despawn();
        }
    }
    pub(super) fn receive(&mut self, world: &mut World, value: Prepared) -> Result<(), String> {
        #[cfg(feature = "validation")]
        {
            self.revision = value.revision;
        }
        let Some((source, mesh, material, transform)) = value.value else {
            if value.committed {
                if let Some(active) = self.active.as_mut() {
                    active.committed = true;
                }
            } else {
                self.release(world);
            }
            return Ok(());
        };
        let root = super::super::placement::active_root(world, &value.map, &source.placement_id)?;
        let parent = world.get::<ChildOf>(root).map(ChildOf::parent);
        if self.active.as_ref().is_some_and(|active| {
            active.root != root || active.map != value.map || active.id != source.placement_id
        }) {
            self.release(world);
        }
        let visibility = world
            .get::<Visibility>(root)
            .copied()
            .ok_or("网格运行根缺少显隐组件")?;
        let mesh_handle = if let Some(handle) = &self.mesh {
            world
                .resource_mut::<Assets<Mesh>>()
                .insert(handle.id(), mesh)
                .map_err(|error| error.to_string())?;
            handle.clone()
        } else {
            let handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
            self.mesh = Some(handle.clone());
            handle
        };
        let material_handle = if let Some(handle) = &self.material {
            world
                .resource_mut::<Assets<StandardMaterial>>()
                .insert(handle.id(), material)
                .map_err(|error| error.to_string())?;
            handle.clone()
        } else {
            let handle = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(material);
            self.material = Some(handle.clone());
            handle
        };
        if let Some(active) = self.active.as_mut() {
            world.entity_mut(active.ghost).insert((
                Mesh3d(mesh_handle),
                MeshMaterial3d(material_handle),
                transform,
            ));
            world
                .entity_mut(active.ghost)
                .remove::<bevy::camera::primitives::Aabb>();
            active.target = source.target;
            active.committed = false;
            active.handoff = false;
        } else {
            self.ticket = self.ticket.wrapping_add(1).max(1);
            let mut ghost = world.spawn((
                Name::new("作者点边面实时预览"),
                Mesh3d(mesh_handle),
                MeshMaterial3d(material_handle),
                transform,
                Visibility::Inherited,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ));
            if let Some(parent) = parent {
                ghost.insert(ChildOf(parent));
            }
            let ghost = ghost.id();
            world.entity_mut(root).insert(OwnedVisibility(self.ticket));
            *world.get_mut::<Visibility>(root).unwrap() = Visibility::Hidden;
            self.active = Some(Active {
                map: value.map,
                id: source.placement_id,
                root,
                parent,
                ghost,
                original: visibility,
                ticket: self.ticket,
                target: source.target,
                committed: false,
                handoff: false,
            });
        }
        Ok(())
    }
    pub(super) fn handoff(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.handoff = true;
        }
    }
    pub(super) fn reconcile(&mut self, world: &mut World) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let owner_current = world
            .get::<OwnedVisibility>(active.root)
            .is_some_and(|owner| owner.0 == active.ticket);
        let map_current = world
            .get_resource::<feathrise_map_content::ActiveDynamicMap>()
            .and_then(|map| map.map_id.as_ref())
            == Some(&active.map);
        let adopted = active.committed
            && active.handoff
            && super::super::placement::active_root(world, &active.map, &active.id)
                .ok()
                .and_then(|entity| world.get::<MapPlacementInstance>(entity))
                .is_some_and(|root| root.asset_target() == active.target);
        if !owner_current || !map_current || adopted || world.get_entity(active.ghost).is_err() {
            self.release(world);
        }
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        serde_json::json!({"revision":self.revision,"ghost":self.active.as_ref().map(|active|active.ghost.to_bits()),
            "visible":self.active.as_ref().is_some_and(|active|world.get::<Mesh3d>(active.ghost).is_some()),
            "hidden_root":self.active.as_ref().is_some_and(|active|world.get::<Visibility>(active.root)==Some(&Visibility::Hidden))})
    }
}
