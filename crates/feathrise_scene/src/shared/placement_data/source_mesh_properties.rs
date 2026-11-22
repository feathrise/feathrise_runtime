//! 源 GLB 的节点策略在资产加载时解码，实例只读取相同资产代次的小型策略。

use bevy::{
    asset::LoadContext,
    ecs::world::EntityWorldMut,
    gltf::{
        extensions::{ErasedGltfExtensionHandler, GltfExtensionHandler, GltfExtensionHandlers},
        gltf,
    },
    prelude::*,
    world_serialization::WorldAsset,
};
use feathrise_scene_content::{MeshCollisionLodSettings, MeshDisplayLodSettings, MeshLodMode};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum SourceMeshHeight {
    FollowTerrain { offset: f32 },
    FixedHeight,
    SnapToGround,
}

/// 每个缺省项在当前摆放上独立继承。显式 Inherit 表示该通道直接读取全局 LOD。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Deserialize)]
#[reflect(opaque)]
#[reflect(Component, Clone)]
#[serde(default)]
pub(crate) struct SourceMeshProperties {
    pub height: Option<SourceMeshHeight>,
    pub collision: Option<bool>,
    pub display_lod: Option<MeshLodMode<MeshDisplayLodSettings>>,
    pub collision_lod: Option<MeshLodMode<MeshCollisionLodSettings>>,
}

impl SourceMeshProperties {
    fn from_extras(raw: &str) -> Result<Self, serde_json::Error> {
        #[derive(Deserialize)]
        struct NodeExtras {
            #[serde(default)]
            feathrise_mesh_properties: SourceMeshProperties,
        }

        // glTF 允许其他工具将任意 JSON 放入 extras，非对象无本项目属性。
        if !raw.trim_start().starts_with('{') {
            return Ok(Self::default());
        }
        let mut properties = serde_json::from_str::<NodeExtras>(raw)?.feathrise_mesh_properties;
        if matches!(properties.height, Some(SourceMeshHeight::FollowTerrain { offset }) if !offset.is_finite())
        {
            properties.height = None;
        }
        if matches!(properties.display_lod, Some(MeshLodMode::Override(settings)) if settings.validate().is_err())
        {
            properties.display_lod = None;
        }
        if matches!(properties.collision_lod, Some(MeshLodMode::Override(settings)) if settings.validate().is_err())
        {
            properties.collision_lod = None;
        }
        Ok(properties)
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SourceMeshCollisionSummary {
    pub has_overrides: bool,
    pub any_enabled: bool,
}

/// 每个载入场景只有一个摘要。后续实例只克隆节点策略，避免拷贝或遍历目录。
#[derive(Component, Default)]
#[component(clone_behavior = Ignore)]
struct SceneSummary {
    ground: bool,
    collision: SourceMeshCollisionSummary,
}

fn summary(asset: &WorldAsset) -> Option<&SceneSummary> {
    let world = &asset.world;
    let component = world.component_id::<SceneSummary>()?;
    let archetypes = world.archetypes();
    let entity = archetypes
        .component_index()
        .get(&component)?
        .keys()
        .find_map(|id| {
            // 后续载入钩子也可能修改根，跳过已空的旧 archetype。
            archetypes
                .get(*id)?
                .entities()
                .first()
                .map(|entry| entry.id())
        })?;
    world.get::<SceneSummary>(entity)
}

pub(crate) fn requires_source_grounding(asset: &WorldAsset) -> bool {
    summary(asset).is_some_and(|summary| summary.ground)
}

pub(crate) fn collision_summary(asset: &WorldAsset) -> SourceMeshCollisionSummary {
    summary(asset).map_or_else(SourceMeshCollisionSummary::default, |summary| {
        summary.collision
    })
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<GltfExtensionHandlers>()
        .register_type::<SourceMeshProperties>();
    app.world()
        .resource::<GltfExtensionHandlers>()
        .0
        .try_write()
        .expect("GLB 载入器启动前登记源网格属性")
        .push(Box::new(ReadSourceMeshProperties::default()));
}

#[derive(Default)]
struct ReadSourceMeshProperties(SceneSummary);

impl GltfExtensionHandler for ReadSourceMeshProperties {
    fn dyn_clone(&self) -> Box<dyn ErasedGltfExtensionHandler> {
        Box::new(Self::default())
    }

    fn on_gltf_node(
        &mut self,
        context: &mut LoadContext<'_>,
        node: &gltf::Node,
        entity: &mut EntityWorldMut,
    ) {
        let Some(raw) = node.extras().as_ref().filter(|_| node.mesh().is_some()) else {
            return;
        };
        let properties = match SourceMeshProperties::from_extras(raw.get()) {
            Ok(properties) => properties,
            Err(error) => {
                warn!(
                    "GLB {} 节点 {} 网格属性无法读取：{error}",
                    context.path(),
                    node.index()
                );
                return;
            }
        };
        if properties == SourceMeshProperties::default() {
            return;
        }
        self.0.ground |= matches!(
            properties.height,
            Some(SourceMeshHeight::FollowTerrain { .. } | SourceMeshHeight::SnapToGround)
        );
        self.0.collision.has_overrides |=
            properties.collision.is_some() || properties.collision_lod.is_some();
        self.0.collision.any_enabled |= properties.collision == Some(true);
        entity.insert(properties);
    }

    fn on_scene_completed(
        &mut self,
        _: &mut LoadContext<'_>,
        _: &gltf::Scene,
        root: Entity,
        world: &mut World,
    ) {
        let summary = std::mem::take(&mut self.0);
        if summary.ground || summary.collision.has_overrides {
            world.entity_mut(root).insert(summary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::reflect::AppTypeRegistry;

    #[test]
    fn source_node_policy_survives_world_instantiation_and_asset_summary_stays_local() {
        // 值与作者端保存的标准节点 extras 一致。保留未启用碰撞与继承碰撞 LOD 的区别。
        let properties = SourceMeshProperties::from_extras(
            r#"{"other":42,"feathrise_mesh_properties":{
            "height":{"mode":"follow_terrain","offset":2.5},"collision":false,
            "display_lod":{"mode":"disabled"},"collision_lod":{"mode":"inherit"}
        }}"#,
        )
        .unwrap();
        assert_eq!(
            properties.height,
            Some(SourceMeshHeight::FollowTerrain { offset: 2.5 })
        );
        assert_eq!(properties.collision, Some(false));
        assert_eq!(properties.display_lod, Some(MeshLodMode::Disabled));
        assert_eq!(properties.collision_lod, Some(MeshLodMode::Inherit));
        assert_eq!(
            SourceMeshProperties::from_extras(r#"{"other":42}"#).unwrap(),
            SourceMeshProperties::default()
        );

        let mut world = World::new();
        world.spawn(SceneSummary {
            ground: true,
            collision: SourceMeshCollisionSummary {
                has_overrides: true,
                any_enabled: false,
            },
        });
        world.spawn(properties);
        let asset = WorldAsset::new(world);
        assert!(requires_source_grounding(&asset));
        let collision = collision_summary(&asset);
        assert!(collision.has_overrides && !collision.any_enabled);

        let mut app = App::new();
        app.register_type::<SourceMeshProperties>();
        let instance = asset
            .clone_with(app.world().resource::<AppTypeRegistry>())
            .unwrap();
        assert!(!requires_source_grounding(&instance));
        let mut query = instance.world.try_query::<&SourceMeshProperties>().unwrap();
        assert_eq!(query.single(&instance.world).unwrap(), &properties);
    }
}
