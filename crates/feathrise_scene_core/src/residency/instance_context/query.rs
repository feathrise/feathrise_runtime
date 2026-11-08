//! 子地图只遍历自身实体索引；父图仍使用原查询过滤，不随实例数量重复扫描整个世界。
use bevy::{ecs::{entity::{EntityHashMap, EntityHashSet, hash_set}, query::{IterQueryData,
    QueryFilter, QueryIter, QueryManyUniqueIter, ROQueryItem}, system::SystemParam}, prelude::*};
use std::collections::BTreeMap;
use super::{MapInstanceOwner, MapRuntimeScope};

#[derive(Resource, Default)]
pub(super) struct OwnershipIndex {
    instances: BTreeMap<Entity, EntityHashSet>,
    entities: EntityHashMap<Entity>,
    empty: EntityHashSet,
}
impl OwnershipIndex {
    pub(super) fn insert(&mut self, entity: Entity, instance: Entity) {
        self.remove(entity);
        self.entities.insert(entity, instance);
        self.instances.entry(instance).or_default().insert(entity);
    }
    pub(super) fn remove(&mut self, entity: Entity) {
        if let Some(instance) = self.entities.remove(&entity) {
            if let Some(entities) = self.instances.get_mut(&instance) {
                entities.remove(&entity);
                if entities.is_empty() { self.instances.remove(&instance); }
            }
        }
    }
    fn entities(&self, instance: Entity) -> &EntityHashSet {
        self.instances.get(&instance).unwrap_or(&self.empty)
    }
}
#[derive(SystemParam)]
pub struct InstanceQuery<'w, 's, D: IterQueryData + 'static, F: QueryFilter + 'static = ()> {
    query: Query<'w, 's, (D, Option<&'static MapInstanceOwner>), F>,
    scope: Option<Res<'w, MapRuntimeScope>>,
    index: Option<Res<'w, OwnershipIndex>>,
}
type OwnedIter<'w, 's, D, F> = QueryManyUniqueIter<'w, 's,
    (D, Option<&'static MapInstanceOwner>), F, std::iter::Copied<hash_set::Iter<'w, Entity>>>;
enum Inner<'w, 's, D: IterQueryData, F: QueryFilter> {
    All(QueryIter<'w, 's, (D, Option<&'static MapInstanceOwner>), F>),
    Owned(OwnedIter<'w, 's, D, F>),
}
pub struct InstanceQueryIter<'w, 's, D: IterQueryData, F: QueryFilter> {
    inner: Inner<'w, 's, D, F>,
    instance: Option<Entity>,
}
impl<'w, 's, D: IterQueryData, F: QueryFilter> Iterator for InstanceQueryIter<'w, 's, D, F> {
    type Item = D::Item<'w, 's>;
    fn next(&mut self) -> Option<Self::Item> {
        let belongs = |(value, owner): (D::Item<'w, 's>, Option<&MapInstanceOwner>)|
            (owner.map(|owner| owner.0) == self.instance).then_some(value);
        match &mut self.inner { Inner::All(iter) => iter.find_map(belongs),
            // 索引包含实例内的全部实体；和原查询一样跳过不匹配或已失效的实体。
            Inner::Owned(iter) => iter.find_map(|item| item.ok().and_then(belongs)) }
    }
}
impl<'w, 's, D: IterQueryData + 'static, F: QueryFilter + 'static> InstanceQuery<'w, 's, D, F> {
    fn instance(&self) -> Option<Entity> { self.scope.as_deref().and_then(|scope| scope.instance) }
    pub fn iter(&self) -> InstanceQueryIter<'_, 's, D::ReadOnly, F> {
        let instance = self.instance();
        let inner = if let (Some(instance), Some(index)) = (instance, self.index.as_deref()) {
            Inner::Owned(self.query.iter_many_unique(index.entities(instance).iter().copied()))
        } else { Inner::All(self.query.iter()) };
        InstanceQueryIter { inner, instance }
    }
    pub fn iter_mut(&mut self) -> InstanceQueryIter<'_, 's, D, F> {
        let instance = self.instance();
        let inner = if let (Some(instance), Some(index)) = (instance, self.index.as_deref()) {
            Inner::Owned(self.query.iter_many_unique_mut(index.entities(instance).iter().copied()))
        } else { Inner::All(self.query.iter_mut()) };
        InstanceQueryIter { inner, instance }
    }
    pub fn get(&self, entity: Entity) -> Result<ROQueryItem<'_, 's, D>, ()> {
        let (value, owner) = self.query.get(entity).map_err(|_| ())?;
        if owner.map(|owner| owner.0) == self.instance() { Ok(value) } else { Err(()) }
    }
    pub fn get_mut(&mut self, entity: Entity) -> Result<D::Item<'_, 's>, ()> {
        let instance = self.instance();
        let (value, owner) = self.query.get_mut(entity).map_err(|_| ())?;
        if owner.map(|owner| owner.0) == instance { Ok(value) } else { Err(()) }
    }
    pub fn single(&self) -> Result<ROQueryItem<'_, 's, D>, ()> {
        let mut values = self.iter();
        let value = values.next().ok_or(())?;
        if values.next().is_none() { Ok(value) } else { Err(()) }
    }
    pub fn single_mut(&mut self) -> Result<D::Item<'_, 's>, ()> {
        let mut values = self.iter_mut();
        let value = values.next().ok_or(())?;
        if values.next().is_none() { Ok(value) } else { Err(()) }
    }
    pub fn contains(&self, entity: Entity) -> bool { self.get(entity).is_ok() }
    pub fn is_empty(&self) -> bool { self.iter().next().is_none() }
}
impl<'a, 'w, 's, D: IterQueryData + 'static, F: QueryFilter + 'static> IntoIterator for &'a InstanceQuery<'w, 's, D, F> {
    type Item = ROQueryItem<'a, 's, D>;
    type IntoIter = InstanceQueryIter<'a, 's, D::ReadOnly, F>;
    fn into_iter(self) -> Self::IntoIter { self.iter() }
}
impl<'a, 'w, 's, D: IterQueryData + 'static, F: QueryFilter + 'static> IntoIterator for &'a mut InstanceQuery<'w, 's, D, F> {
    type Item = D::Item<'a, 's>;
    type IntoIter = InstanceQueryIter<'a, 's, D, F>;
    fn into_iter(self) -> Self::IntoIter { self.iter_mut() }
}
