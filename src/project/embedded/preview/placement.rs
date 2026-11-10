//! 原对象手柄的稳定根投影；索引随实体生命周期维护，多选使用原作者的分层pivot保留剪切。

use bevy::prelude::*;
use feathrise_scene_content::{SceneId, embedded_placement_preview::{
    MAX_PLACEMENT_PREVIEW_MEMBERS, PLACEMENT_PREVIEW_FORMAT, PlacementPreviewCommand, PlacementPreviewKind, PlacementPreviewFinalMember}};
use feathrise_scene::{ScenePlacementInstance, SubsceneInstance};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Resource, Default)]
struct RootIndex {
    roots: BTreeMap<(SceneId, String), BTreeSet<Entity>>,
    reverse: BTreeMap<Entity, (SceneId, String)>,
}
fn inserted(event: On<Insert<ScenePlacementInstance>>, roots: Query<&ScenePlacementInstance>, mut index: ResMut<RootIndex>) {
    if let Ok(root) = roots.get(event.entity) {
        let key = (root.scene_id().clone(), root.id().to_owned());
        if let Some(previous) = index.reverse.get(&event.entity).cloned()
            && previous != key {
            if let Some(roots) = index.roots.get_mut(&previous) { roots.remove(&event.entity); }
            if index.roots.get(&previous).is_some_and(BTreeSet::is_empty) { index.roots.remove(&previous); }
        }
        index.roots.entry(key.clone()).or_default().insert(event.entity);
        index.reverse.insert(event.entity, key);
    }
}
fn removed(event: On<Remove<ScenePlacementInstance>>, mut index: ResMut<RootIndex>) {
    if let Some(key) = index.reverse.remove(&event.entity) {
        if let Some(roots) = index.roots.get_mut(&key) { roots.remove(&event.entity); }
        if index.roots.get(&key).is_some_and(BTreeSet::is_empty) { index.roots.remove(&key); }
    }
}
pub(super) fn install(app: &mut App) {
    app.init_resource::<RootIndex>().add_observer(inserted).add_observer(removed);
}

pub(super) fn decode(format: &str, bytes: &[u8], map: &SceneId) -> Result<PlacementPreviewCommand, String> {
    if format != PLACEMENT_PREVIEW_FORMAT || bytes.len() > 256 * 1024 { return Err("对象预览格式或大小无效".into()); }
    let value: PlacementPreviewCommand = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    match &value {
        PlacementPreviewCommand::Begin { gesture, scene_id, ids, kind, .. } => {
            if *gesture == 0 || scene_id != map || ids.is_empty() || ids.len() > MAX_PLACEMENT_PREVIEW_MEMBERS
                || (*kind == PlacementPreviewKind::Absolute && ids.len() != 1)
                || ids.iter().any(|id| id.is_empty() || id.len() > 256)
                || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
                return Err("对象预览成员身份无效或重复".into());
            }
        }
        PlacementPreviewCommand::Update { transform, .. } => { matrix(*transform)?; }
        PlacementPreviewCommand::End { transform, final_members, .. } => {
            if let Some(transform) = transform { matrix(*transform)?; }
            if final_members.len() > MAX_PLACEMENT_PREVIEW_MEMBERS
                || final_members.iter().map(|member|&member.id).collect::<BTreeSet<_>>().len() != final_members.len() {
                return Err("对象结束成员过多或重复".into());
            }
            for member in final_members { exact_transform(matrix(member.local)?)?; }
        }
    }
    Ok(value)
}

fn matrix(value: [f32; 16]) -> Result<Mat4, String> {
    let value = Mat4::from_cols_array(&value);
    if !value.is_finite() || value.determinant().abs() < 1e-12
        || value.x_axis.w != 0.0 || value.y_axis.w != 0.0 || value.z_axis.w != 0.0 || value.w_axis.w != 1.0 {
        return Err("对象预览需要有限且可逆的仿射变换".into());
    }
    Ok(value)
}
fn exact_transform(value: Mat4) -> Result<Transform, String> {
    let result = Transform::from_matrix(value);
    if !result.to_matrix().abs_diff_eq(value, 0.0005) { return Err("对象局部变换含不可表示的剪切".into()); }
    Ok(result)
}

/// 通过当前索引定向复核活动根；同来源子场景实例不能冒充父图作者对象。
pub(crate) fn active_root(world: &World, map: &SceneId, id: &str) -> Result<Entity, String> {
    let roots = world.get_resource::<RootIndex>().and_then(|index| index.roots.get(&(map.clone(), id.to_owned())))
        .ok_or_else(|| format!("对象{id}的运行根尚未就绪"))?;
    let mut found = None;
    for &entity in roots {
        if !world.get::<ScenePlacementInstance>(entity).is_some_and(|root|root.is_active() && root.scene_id() == map && root.id() == id) { continue; }
        let mut parent = Some(entity);
        let mut admitted = true;
        for _ in 0..32 {
            let Some(current) = parent else { break; };
            if world.get::<SubsceneInstance>(current).is_some_and(|node| !node.path.instances.is_empty()) { admitted = false; break; }
            parent = world.get::<ChildOf>(current).map(ChildOf::parent);
        }
        if parent.is_some() { admitted = false; }
        if admitted && found.replace(entity).is_some() { return Err(format!("对象{id}存在重复活动根")); }
    }
    found.ok_or_else(|| format!("对象{id}没有当前场景活动根"))
}

struct Root { id: String, entity: Entity, parent: Option<Entity>, original: Transform, pivot: Option<Entity> }
struct Pivot { parent: Option<Entity>, control: Entity, attach: Entity, cleanup: Entity }
struct Gesture {
    id: u64, sequence: u64, kind: PlacementPreviewKind, scene_id: SceneId,
    pending: VecDeque<String>, roots: Vec<Root>, pivots: Vec<Pivot>,
    ended: bool, snapshot_started: bool,
    finishing: VecDeque<PlacementPreviewFinalMember>,
    abandon_cursor: Option<usize>,
}
#[derive(Default)]
pub(super) struct State {
    active: Option<Gesture>,
    #[cfg(feature = "validation")]
    last_edge: Option<(&'static str, u64, bool)>,
    #[cfg(feature = "validation")]
    evidence_roots: Vec<Entity>,
}

impl State {
    pub(super) fn release_root(&mut self, world: &mut World, entity: Entity) {
        if let Some(gesture) = self.active.as_mut()
            && let Some(root) = gesture.roots.iter_mut().find(|root| root.entity == entity)
            && let Some(pivot) = root.pivot.take()
            && world.get::<ChildOf>(entity).map(ChildOf::parent) == Some(pivot) {
            // 只归还本手势仍拥有的父级；正式采用或项目系统已接管时，不能重新拉回旧容器。
            if let Some(parent) = root.parent { world.entity_mut(entity).insert(ChildOf(parent)); }
            else { world.entity_mut(entity).remove::<ChildOf>(); }
        }
    }
    pub(super) fn receive(&mut self, world: &mut World, map: &SceneId, command: PlacementPreviewCommand) -> Result<(), String> {
        #[cfg(feature = "validation")]
        { self.last_edge = Some(match &command {
            PlacementPreviewCommand::Begin { gesture, .. } => ("begin", *gesture, false),
            PlacementPreviewCommand::Update { gesture, .. } => ("update", *gesture, false),
            PlacementPreviewCommand::End { gesture, cancelled, .. } => ("end", *gesture, *cancelled),
        }); }
        match command {
            PlacementPreviewCommand::Begin { gesture, ids, kind, .. } => {
                #[cfg(feature = "validation")]
                { self.evidence_roots.clear(); }
                if let Some(previous) = self.active.take() { cleanup(world, previous, false)?; }
                self.active = Some(Gesture { id: gesture, sequence: 0, kind, scene_id: map.clone(), pending: ids.into(),
                    roots: Vec::new(), pivots: Vec::new(), ended: false, snapshot_started: false, finishing: VecDeque::new(), abandon_cursor: None });
            }
            PlacementPreviewCommand::Update { gesture, sequence, transform } => {
                self.update(world, gesture, sequence, Some(transform))?;
            }
            PlacementPreviewCommand::End { gesture, sequence, transform, cancelled, final_members, .. } => {
                let abandon = cancelled && transform.is_none() && final_members.is_empty();
                self.update(world, gesture, sequence, transform)?;
                let active = self.active.as_mut().unwrap();
                if abandon {
                    active.ended = true;
                    active.abandon_cursor = Some(0);
                } else if active.kind == PlacementPreviewKind::GroupDelta || !final_members.is_empty() {
                    if final_members.len() != active.roots.len()
                        || final_members.iter().any(|member| !active.roots.iter().any(|root|root.id == member.id)) {
                        return Err("对象结束必须覆盖本手势全部最终权威姿态".into());
                    }
                    active.ended = true;
                    active.finishing = final_members.into();
                } else if cancelled {
                    if let Some(value) = self.active.take() { cleanup(world, value, true)?; }
                } else if let Some(value) = self.active.take() { cleanup(world, value, false)?; }
            }
        }
        Ok(())
    }
    fn update(&mut self, world: &mut World, id: u64, sequence: u64, transform: Option<[f32; 16]>) -> Result<(), String> {
        let gesture = self.active.as_mut().filter(|value| value.id == id).ok_or("对象预览不属于当前手势")?;
        if gesture.ended || !gesture.pending.is_empty() || sequence <= gesture.sequence { return Err("对象预览顺序倒退或开始尚未完成".into()); }
        gesture.sequence = sequence;
        let Some(transform) = transform else { return Ok(()); };
        let transform = matrix(transform)?;
        if gesture.kind == PlacementPreviewKind::Absolute {
            let root = gesture.roots.first().ok_or("单对象预览缺少运行根")?;
            let parent = root.parent.and_then(|parent| world.get::<GlobalTransform>(parent)).map_or(Mat4::IDENTITY, GlobalTransform::to_matrix);
            let local = exact_transform(parent.inverse() * transform)?;
            world.entity_mut(root.entity).insert(local);
        } else {
            let delta = exact_transform(transform)?;
            for pivot in &gesture.pivots { world.entity_mut(pivot.control).insert(delta); }
        }
        Ok(())
    }
    pub(super) fn snapshot_started(&mut self, _: &mut World) {
        if let Some(active) = self.active.as_mut() { active.snapshot_started = true; }
    }
    pub(super) fn advance(&mut self, world: &mut World) -> Result<bool, String> {
        let Some(gesture) = self.active.as_mut() else { return Ok(true); };
        for _ in 0..32 {
            let Some(id) = gesture.pending.pop_front() else { break; };
            let entity = active_root(world, &gesture.scene_id, &id)?;
            let parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
            let original = *world.get::<Transform>(entity).ok_or("对象运行根缺少变换")?;
            let pivot = if gesture.kind == PlacementPreviewKind::GroupDelta {
                let index = gesture.pivots.iter().position(|pivot| pivot.parent == parent);
                let index = if let Some(index) = index { index } else {
                    if gesture.pivots.len() >= 66 { return Err("对象预览跨越过多来源容器".into()); }
                    gesture.pivots.push(spawn_pivot(world, parent)?);
                    gesture.pivots.len() - 1
                };
                let attach = gesture.pivots[index].attach;
                world.entity_mut(entity).insert(ChildOf(attach));
                Some(attach)
            } else { None };
            gesture.roots.push(Root { id, entity, parent, original, pivot });
            #[cfg(feature = "validation")]
            self.evidence_roots.push(entity);
        }
        if let Some(cursor) = gesture.abandon_cursor {
            let end = (cursor + 32).min(gesture.roots.len());
            for root in &mut gesture.roots[cursor..end] {
                if let Some(pivot) = root.pivot.take()
                    && world.get::<ChildOf>(root.entity).map(ChildOf::parent) == Some(pivot) {
                    // 外部删除/切换场景时没有合法作者终值；只归还父级所有权，不写回旧initial。
                    if let Some(parent) = root.parent { world.entity_mut(root.entity).insert(ChildOf(parent)); }
                    else { world.entity_mut(root.entity).remove::<ChildOf>(); }
                }
            }
            if end < gesture.roots.len() { gesture.abandon_cursor = Some(end); return Ok(false); }
            let previous = self.active.take().unwrap(); cleanup(world, previous, false)?; return Ok(true);
        }
        if !gesture.finishing.is_empty() {
            for _ in 0..32 {
                let Some(member) = gesture.finishing.pop_front() else { break; };
                let root = gesture.roots.iter_mut().find(|root| root.id == member.id).ok_or("对象结束出现未知成员")?;
                if !world.get::<ScenePlacementInstance>(root.entity).is_some_and(|instance|
                    instance.is_active() && instance.id() == member.id) { return Err("对象结束时运行根已换代".into()); }
                let local = exact_transform(matrix(member.local)?)?;
                if let Some(pivot) = root.pivot.take() {
                    if world.get::<ChildOf>(root.entity).map(ChildOf::parent) != Some(pivot) { return Err("对象结束时临时父级所有权已改变".into()); }
                    if let Some(parent) = root.parent { world.entity_mut(root.entity).insert(ChildOf(parent)); }
                    else { world.entity_mut(root.entity).remove::<ChildOf>(); }
                }
                world.entity_mut(root.entity).insert(local);
            }
            if !gesture.finishing.is_empty() { return Ok(false); }
            let previous = self.active.take().unwrap();
            cleanup(world, previous, false)?;
            return Ok(true); // End真正写回原根之后才ACK；下一Begin无需等待整图快照。
        }
        // 正式候选采用后旧根身份退役；只清空本手势仍拥有的辅助层级，绝不动新根。
        if gesture.ended && gesture.snapshot_started && gesture.roots.iter().all(|root|
            !world.get::<ScenePlacementInstance>(root.entity).is_some_and(ScenePlacementInstance::is_active)) {
            let old = self.active.take().unwrap(); cleanup(world, old, true)?; return Ok(true);
        }
        Ok(gesture.pending.is_empty())
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        let roots = self.evidence_roots.iter().filter_map(|entity| {
            let root = world.get::<ScenePlacementInstance>(*entity)?;
            let local = world.get::<Transform>(*entity)?;
            let global = world.get::<GlobalTransform>(*entity)?;
            Some(serde_json::json!({"id":root.id(), "entity":entity.to_bits(), "active":root.is_active(),
                "local":local.to_matrix().to_cols_array(), "world":global.to_matrix().to_cols_array()}))
        }).collect::<Vec<_>>();
        serde_json::json!({"edge":self.last_edge.map(|value|value.0), "gesture":self.last_edge.map(|value|value.1),
            "cancelled":self.last_edge.map(|value|value.2), "placements":roots})
    }
}

fn spawn_pivot(world: &mut World, parent: Option<Entity>) -> Result<Pivot, String> {
    let source = parent.and_then(|entity| world.get::<GlobalTransform>(entity)).map_or(Mat4::IDENTITY, GlobalTransform::to_matrix);
    let source = exact_transform(source)?;
    // 与原作者GroupWorld相同：P*S^-1*(R^-1 T^-1)*A*P，避免把剪切硬分解成单个TRS。
    let cleanup = world.spawn((Transform::from_scale(source.scale.recip()), Visibility::Inherited)).id();
    if let Some(parent) = parent { world.entity_mut(cleanup).insert(ChildOf(parent)); }
    let rotation = source.rotation.inverse();
    let inverse = world.spawn((Transform { translation: rotation * -source.translation, rotation, scale: Vec3::ONE },
        Visibility::Inherited, ChildOf(cleanup))).id();
    let control = world.spawn((Transform::IDENTITY, Visibility::Inherited, ChildOf(inverse))).id();
    let attach = world.spawn((source, Visibility::Inherited, ChildOf(control))).id();
    Ok(Pivot { parent, control, attach, cleanup })
}
fn cleanup(world: &mut World, value: Gesture, restore_group: bool) -> Result<(), String> {
    for root in value.roots {
        if root.pivot.is_none() || world.get::<ChildOf>(root.entity).map(ChildOf::parent) != root.pivot { continue; }
        if !restore_group && world.get::<ScenePlacementInstance>(root.entity).is_some_and(ScenePlacementInstance::is_active) {
            return Err("前一多选手势尚未由正式快照接回".into());
        }
        let mut entity = world.entity_mut(root.entity);
        entity.insert(root.original);
        if let Some(parent) = root.parent { entity.insert(ChildOf(parent)); } else { entity.remove::<ChildOf>(); }
    }
    for pivot in value.pivots { if world.get_entity(pivot.cleanup).is_ok() { world.entity_mut(pivot.cleanup).despawn(); } }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gesture_fixture(count: usize) -> (App, SceneId, Entity, Vec<Entity>, Vec<String>) {
        let mut app = App::new(); install(&mut app);
        let map = SceneId::parse("MAP-preview").unwrap();
        let parent = app.world_mut().spawn((Transform::IDENTITY, GlobalTransform::IDENTITY)).id();
        let mut entities = Vec::new(); let mut ids = Vec::new();
        for index in 0..count {
            let id = format!("object-{index}");
            let record = serde_json::from_value(serde_json::json!({"id":id,"asset_path":"model.glb",
                "translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]})).unwrap();
            entities.push(app.world_mut().spawn((ScenePlacementInstance::new(&map, &record),
                Transform::IDENTITY, GlobalTransform::IDENTITY, ChildOf(parent))).id());
            ids.push(id);
        }
        (app, map, parent, entities, ids)
    }
    #[test]
    fn two_group_gestures_commit_in_place_without_a_full_snapshot_between_them() {
        let (mut app, map, parent, roots, ids) = gesture_fixture(65);
        let mut state = State::default();
        for gesture in [1, 2] {
            state.receive(app.world_mut(), &map, PlacementPreviewCommand::Begin { gesture, scene_id:map.clone(),
                source_revision:gesture, ids:ids.clone(), kind:PlacementPreviewKind::GroupDelta }).unwrap();
            assert!(!state.advance(app.world_mut()).unwrap());
            assert!(!state.advance(app.world_mut()).unwrap());
            assert!(state.advance(app.world_mut()).unwrap());
            state.receive(app.world_mut(), &map, PlacementPreviewCommand::Update { gesture, sequence:1,
                transform:Transform::from_translation(Vec3::X).to_matrix().to_cols_array() }).unwrap();
            let local = Transform::from_translation(Vec3::X * gesture as f32).to_matrix().to_cols_array();
            state.receive(app.world_mut(), &map, PlacementPreviewCommand::End { gesture, sequence:2,
                source_revision:gesture+1, transform:None, cancelled:false,
                final_members:ids.iter().map(|id|PlacementPreviewFinalMember { id:id.clone(), local }).collect() }).unwrap();
            assert!(!state.advance(app.world_mut()).unwrap(), "End不能提前ACK尚未解除的65个根");
            assert!(!state.advance(app.world_mut()).unwrap());
            assert!(state.advance(app.world_mut()).unwrap());
            for entity in &roots {
                assert_eq!(app.world().get::<ChildOf>(*entity).unwrap().parent(), parent);
                assert_eq!(app.world().get::<Transform>(*entity).unwrap().translation.x, gesture as f32);
                assert!(app.world().get::<ScenePlacementInstance>(*entity).unwrap().is_active());
            }
        }
    }
    #[test]
    fn explicit_authority_abandon_only_releases_owned_pivot_without_restoring_old_pose() {
        let (mut app, map, parent, roots, ids) = gesture_fixture(1);
        let mut state = State::default();
        state.receive(app.world_mut(), &map, PlacementPreviewCommand::Begin { gesture:1, scene_id:map.clone(),
            source_revision:1, ids, kind:PlacementPreviewKind::GroupDelta }).unwrap();
        assert!(state.advance(app.world_mut()).unwrap());
        app.world_mut().entity_mut(roots[0]).insert(Transform::from_translation(Vec3::X * 17.0));
        state.receive(app.world_mut(), &map, PlacementPreviewCommand::End { gesture:1, sequence:1,
            source_revision:2, transform:None, cancelled:true, final_members:vec![] }).unwrap();
        assert!(state.advance(app.world_mut()).unwrap());
        assert_eq!(app.world().get::<ChildOf>(roots[0]).unwrap().parent(), parent);
        assert_eq!(app.world().get::<Transform>(roots[0]).unwrap().translation.x, 17.0);
    }
    #[test]
    fn formal_adoption_does_not_reparent_a_root_already_taken_by_project_systems() {
        let (mut app, map, _, roots, ids) = gesture_fixture(1);
        let mut state = State::default();
        state.receive(app.world_mut(), &map, PlacementPreviewCommand::Begin { gesture:1, scene_id:map.clone(),
            source_revision:1, ids, kind:PlacementPreviewKind::GroupDelta }).unwrap();
        assert!(state.advance(app.world_mut()).unwrap());
        let owner = app.world_mut().spawn((Transform::IDENTITY, GlobalTransform::IDENTITY)).id();
        app.world_mut().entity_mut(roots[0]).insert(ChildOf(owner));
        state.release_root(app.world_mut(), roots[0]);
        assert_eq!(app.world().get::<ChildOf>(roots[0]).unwrap().parent(), owner);
        state.receive(app.world_mut(), &map, PlacementPreviewCommand::End { gesture:1, sequence:1,
            source_revision:2, transform:None, cancelled:true, final_members:vec![] }).unwrap();
        assert!(state.advance(app.world_mut()).unwrap());
        assert_eq!(app.world().get::<ChildOf>(roots[0]).unwrap().parent(), owner);
    }
    #[test]
    fn local_conversion_retains_nonuniform_parent_and_rejects_shear() {
        let parent = Transform::from_rotation(Quat::from_rotation_y(0.6)).with_scale(Vec3::new(2.0, 1.0, 3.0));
        let local = Transform::from_translation(Vec3::new(3.0, 1.0, 2.0)).with_rotation(Quat::from_rotation_y(0.3));
        let global = parent.to_matrix() * local.to_matrix();
        assert!(exact_transform(parent.to_matrix().inverse() * global).unwrap().to_matrix().abs_diff_eq(local.to_matrix(), 0.0005));
        assert!(exact_transform(global).is_err());
    }
    #[test]
    fn world_delta_pivot_preserves_exact_affine_for_nonuniform_source() {
        let mut world = World::new();
        let source = Transform::from_translation(Vec3::new(10.0, 2.0, 4.0)).with_rotation(Quat::from_rotation_y(0.4)).with_scale(Vec3::new(2.0, 1.0, 3.0));
        let parent = world.spawn((source, GlobalTransform::from(source))).id();
        let pivot = spawn_pivot(&mut world, Some(parent)).unwrap();
        let delta = Transform::from_rotation(Quat::from_rotation_y(0.8)).with_translation(Vec3::X);
        world.entity_mut(pivot.control).insert(delta);
        let mut product = Mat4::IDENTITY;
        let mut node = Some(pivot.attach);
        while let Some(entity) = node {
            product = world.get::<Transform>(entity).unwrap().to_matrix() * product;
            node = world.get::<ChildOf>(entity).map(ChildOf::parent);
        }
        assert!(product.abs_diff_eq(delta.to_matrix() * source.to_matrix(), 0.0005));
    }
}
