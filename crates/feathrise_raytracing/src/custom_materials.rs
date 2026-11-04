//! 应用材质的显式 Deferred 适配：仅交换基础渲染字段，具体材质与 shader 始终由应用拥有。

use std::{any::TypeId, marker::PhantomData};

use bevy::{
    asset::{AssetEvent, UntypedAssetId},
    material::OpaqueRendererMethod,
    pbr::{ExtendedMaterial, MaterialExtension},
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use crate::{
    constants::MAX_DEFERRED_RASTER_MATERIAL_COUNT,
    lifecycle::{maintain_deferred_raster_bridge, restore_deferred_raster_bridge},
};

/// 应用材质提供自己的基础 PBR 层；共享桥不读取扩展参数或接管材质的业务行为。
pub trait DeferredRasterMaterial: Asset {
    fn base_material(&self) -> &StandardMaterial;
    fn base_material_mut(&mut self) -> &mut StandardMaterial;
}

impl<E: MaterialExtension> DeferredRasterMaterial for ExtendedMaterial<StandardMaterial, E> {
    fn base_material(&self) -> &StandardMaterial {
        &self.base
    }
    fn base_material_mut(&mut self) -> &mut StandardMaterial {
        &mut self.base
    }
}

/// 应用的材质生产系统可显式排在 Collect 前，确保本帧新增资产在同帧提取前完成桥接。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeferredRasterMaterialSystems {
    Collect,
    Apply,
}

/// 在应用安装该材质的 MaterialPlugin 后登记适配；重复登记同一类型不重复消费事件。
/// 此函数不安装材质、创建资产或改变透明算法，普通编辑器无需登记原游戏的材质。
pub fn register_deferred_raster_material<M: DeferredRasterMaterial>(app: &mut App) {
    if app.world().contains_resource::<RegisteredMaterial<M>>() {
        return;
    }
    app.init_resource::<CustomDeferredMaterials>()
        .insert_resource(RegisteredMaterial::<M>(PhantomData))
        .add_systems(
            Update,
            collect_custom_material_changes::<M>
                .in_set(DeferredRasterMaterialSystems::Collect)
                .before(maintain_deferred_raster_bridge),
        )
        .add_systems(
            Update,
            apply_custom_material_changes::<M>
                .in_set(DeferredRasterMaterialSystems::Apply)
                .after(restore_deferred_raster_bridge),
        );
}

#[derive(Resource)]
struct RegisteredMaterial<M: Asset>(PhantomData<M>);

#[derive(Clone, Copy)]
pub(crate) struct CustomMaterialRecord {
    pub(crate) alpha_mode: AlphaMode,
    pub(crate) method: OpaqueRendererMethod,
}

impl CustomMaterialRecord {
    pub(crate) fn writes_gbuffer(self) -> bool {
        matches!(
            self.alpha_mode,
            AlphaMode::Opaque | AlphaMode::Mask(_) | AlphaMode::AlphaToCoverage
        )
    }
}

/// 每类最多保留257项字段快照；真实总数只取Assets::len，超预算时仍由原桥完整拒绝。
#[derive(Resource, Default)]
pub(crate) struct CustomDeferredMaterials {
    pub(crate) records: HashMap<UntypedAssetId, CustomMaterialRecord>,
    pub(crate) loaded_counts: HashMap<TypeId, usize>,
    pub(crate) changed: HashSet<UntypedAssetId>,
    pub(crate) writes: HashMap<UntypedAssetId, OpaqueRendererMethod>,
    pub(crate) overflowed: HashSet<TypeId>,
}

fn collect_custom_material_changes<M: DeferredRasterMaterial>(
    assets: Option<Res<Assets<M>>>,
    mut events: MessageReader<AssetEvent<M>>,
    mut state: ResMut<CustomDeferredMaterials>,
) {
    let kind = TypeId::of::<M>();
    let Some(assets) = assets else {
        let removed: Vec<_> = state
            .records
            .keys()
            .filter(|id| id.type_id() == kind)
            .copied()
            .collect();
        for id in removed {
            state.records.remove(&id);
            state.changed.insert(id); // 资源整体被卸载也必须撤销已发布的真实材质目标。
            state.writes.remove(&id);
        }
        state.loaded_counts.remove(&kind); // 重建资源后重新执行一次有界初始快照。
        state.overflowed.remove(&kind);
        events.read().for_each(drop);
        return;
    };
    let previous = state.loaded_counts.insert(kind, assets.len());
    let recovering = previous.is_some_and(|count| count > MAX_DEFERRED_RASTER_MATERIAL_COUNT)
        && assets.len() <= MAX_DEFERRED_RASTER_MATERIAL_COUNT;
    if previous.is_none() || recovering {
        // 初次登记或从超预算恢复时只做一次有硬上限的快照；稳定帧只读资产事件。
        let previous_ids: Vec<_> = state
            .records
            .keys()
            .filter(|id| id.type_id() == kind)
            .copied()
            .collect();
        state.changed.extend(previous_ids); // 完整快照也撤销已不在新资产表里的旧成员。
        state.records.retain(|id, _| id.type_id() != kind);
        state.overflowed.remove(&kind);
        for (id, material) in assets.iter().take(MAX_DEFERRED_RASTER_MATERIAL_COUNT + 1) {
            let base = material.base_material();
            let record = CustomMaterialRecord {
                alpha_mode: base.alpha_mode,
                method: base.opaque_render_method,
            };
            if !record.writes_gbuffer() {
                continue;
            } // 初始访问预算仍按Assets总数计，透明字段无需长期缓存。
            state.records.insert(
                id.untyped(),
                CustomMaterialRecord {
                    alpha_mode: base.alpha_mode,
                    method: base.opaque_render_method,
                },
            );
            state.changed.insert(id.untyped());
        }
    }
    for event in events.read() {
        let id = match *event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => id,
            AssetEvent::Removed { id } => {
                if state.records.remove(&id.untyped()).is_some() {
                    state.changed.insert(id.untyped());
                }
                continue;
            }
            AssetEvent::Unused { .. } => continue,
        };
        let Some(material) = assets.get(id) else {
            continue;
        };
        let base = material.base_material();
        let record = CustomMaterialRecord {
            alpha_mode: base.alpha_mode,
            method: base.opaque_render_method,
        };
        if !record.writes_gbuffer() && !state.records.contains_key(&id.untyped()) {
            continue; // 活动期新增透明资产不占成员预算，保持既有桥的规则。
        }
        // 活动会话超限时后续桥立即完整回退；不为证明超限复制无界资产表。
        if !state.records.contains_key(&id.untyped())
            && state.records.len() > MAX_DEFERRED_RASTER_MATERIAL_COUNT
        {
            state.overflowed.insert(kind);
            continue;
        }
        state.records.insert(
            id.untyped(),
            CustomMaterialRecord {
                alpha_mode: base.alpha_mode,
                method: base.opaque_render_method,
            },
        );
        state.changed.insert(id.untyped());
    }
}

fn apply_custom_material_changes<M: DeferredRasterMaterial>(
    assets: Option<ResMut<Assets<M>>>,
    mut state: ResMut<CustomDeferredMaterials>,
) {
    let Some(mut assets) = assets else {
        return;
    };
    let kind = TypeId::of::<M>();
    // 只遍历受256项桥预算限制的待写集合；同帧在RenderApp提取之前应用最终采用或回退值。
    let pending: Vec<_> = state
        .writes
        .iter()
        .filter(|(id, _)| id.type_id() == kind)
        .map(|(id, method)| (*id, *method))
        .collect();
    for (id, method) in pending {
        state.writes.remove(&id);
        if let Some(mut material) = assets.get_mut(id.typed::<M>()) {
            material.base_material_mut().opaque_render_method = method;
            if let Some(record) = state.records.get_mut(&id) {
                record.method = method;
            }
        }
        if state
            .records
            .get(&id)
            .is_some_and(|record| !record.writes_gbuffer())
        {
            state.records.remove(&id); // 透明化回退已经应用，不继续缓存无关资产字段。
        }
    }
}
