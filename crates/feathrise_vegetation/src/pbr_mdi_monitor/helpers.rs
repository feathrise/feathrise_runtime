//! 公开普通分箱与项目自有MDI镜像的纯统计函数。

use std::collections::HashSet;

use bevy::render::{
    render_phase::{BinnedPhaseItem, ViewBinnedRenderPhases},
    sync_world::MainEntity,
    view::RetainedViewEntity,
};

use super::types::{PbrMdiPhaseEntry, PbrPhaseStats};

/// 合并一个阶段的公开普通分箱与镜像MDI条目。
pub(super) fn summarize_phase<BPI>(
    phases: &ViewBinnedRenderPhases<BPI>,
    mdi_entries: &[PbrMdiPhaseEntry<BPI>],
    matches_bin: impl Fn(&BPI::BinKey) -> bool,
    matches_entity: impl Fn(MainEntity) -> bool,
) -> PbrPhaseStats
where
    BPI: BinnedPhaseItem,
{
    let mut stats = PbrPhaseStats::default(); // 从零累计当前完整官方排队帧。
    let mut matched_views = HashSet::<RetainedViewEntity>::new(); // 普通与MDI共用视图去重。
    for (view, phase) in phases.iter() {
        for ((_, bin_key), bin) in &phase.batchable_meshes {
            if !matches_bin(bin_key) {
                continue; // 资产过滤未命中的普通分箱不参与统计。
            }
            let entity_count = count_matching_entities(bin.entities().keys(), &matches_entity);
            if entity_count == 0 {
                continue; // 精确身份过滤后没有目标实体。
            }
            matched_views.insert(*view); // 同一视图跨三种分箱只计一次。
            stats.draw_groups = stats.draw_groups.saturating_add(1); // 普通可合批Bin形成一个提交组。
            stats.bins = stats.bins.saturating_add(1); // 当前官方普通分箱计一次。
            stats.instances = stats.instances.saturating_add(entity_count); // 只累计匹配实体。
        }
        for ((_, bin_key), entities) in &phase.unbatchable_meshes {
            if !matches_bin(bin_key) {
                continue; // 非目标资产独立分箱不参与统计。
            }
            let entity_count = count_matching_entities(entities.entities.keys(), &matches_entity);
            if entity_count == 0 {
                continue; // 当前独立分箱没有目标实体。
            }
            matched_views.insert(*view); // 当前视图确实提交了目标实体。
            stats.draw_groups = stats.draw_groups.saturating_add(entity_count); // 独立实体各自形成提交组。
            stats.bins = stats.bins.saturating_add(1); // 多个独立实体仍共享一个官方Bin。
            stats.instances = stats.instances.saturating_add(entity_count); // 实体数量即实例数量。
        }
    }

    let mut mdi_groups = HashSet::<(RetainedViewEntity, BPI::BatchSetKey)>::new();
    let mut mdi_bins = HashSet::<(RetainedViewEntity, BPI::BatchSetKey, BPI::BinKey)>::new();
    for entry in mdi_entries {
        if !matches_bin(&entry.bin_key) || !matches_entity(entry.entity) {
            continue; // 同时保持资产与精确实体两层过滤语义。
        }
        matched_views.insert(entry.view); // MDI与公开普通分箱共享视图去重。
        mdi_groups.insert((entry.view, entry.batch_set_key.clone())); // 同一视图每个批集只提交一次。
        mdi_bins.insert((
            entry.view,
            entry.batch_set_key.clone(),
            entry.bin_key.clone(),
        )); // 同一批集内每个Mesh只计一个分箱。
        stats.instances = stats.instances.saturating_add(1); // 镜像每条记录严格对应一个可见实体。
    }
    stats.views = usize_to_u32(matched_views.len()); // 普通与MDI联合视图数量。
    stats.draw_groups = stats
        .draw_groups
        .saturating_add(usize_to_u32(mdi_groups.len())); // MDI按批集而非Mesh数量计提交组。
    stats.bins = stats.bins.saturating_add(usize_to_u32(mdi_bins.len())); // MDI按完整官方键去重分箱。
    stats
}

/// 统计公开分箱中满足调用方精确实体条件的数量。
fn count_matching_entities<'a>(
    entities: impl IntoIterator<Item = &'a MainEntity>,
    matches_entity: &impl Fn(MainEntity) -> bool,
) -> u32 {
    entities.into_iter().fold(0u32, |count, entity| {
        count.saturating_add(u32::from(matches_entity(*entity)))
    })
}

/// 把容器长度安全压缩到诊断字段，极端数量使用饱和值。
fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)] // 纯值回归不创建窗口、RenderApp或GPU资源。
mod tests {
    use bevy::asset::AssetId;
    use bevy::core_pipeline::core_3d::{Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey};
    use bevy::prelude::{Entity, Mesh};
    use bevy::render::{
        batching::gpu_preprocessing::GpuPreprocessingMode,
        mesh::allocator::MeshSlabs,
        render_phase::{
            BinnedRenderPhaseType, DrawFunctionId, InputUniformIndex, ViewBinnedRenderPhases,
        },
        render_resource::CachedRenderPipelineId,
        sync_world::MainEntity,
        view::RetainedViewEntity,
    };

    use super::summarize_phase;
    use crate::pbr_mdi_monitor::types::PbrMdiPhaseEntry;

    /// 构造可辨识且无需真实相机实体的稳定视图身份。
    fn retained_view(entity: u32) -> RetainedViewEntity {
        RetainedViewEntity {
            main_entity: MainEntity::from(Entity::from_raw_u32(entity).unwrap()),
            auxiliary_entity: MainEntity::from(Entity::PLACEHOLDER),
            subview_index: 0,
        }
    }

    /// 构造只用于分组语义的Opaque批集键。
    fn batch_set(pipeline: usize) -> Opaque3dBatchSetKey {
        Opaque3dBatchSetKey {
            pipeline: CachedRenderPipelineId::new(pipeline),
            draw_function: DrawFunctionId(7),
            material_bind_group_index: Some(3),
            slabs: MeshSlabs::default(),
            lightmap_slab: None,
        }
    }

    /// 构造不依赖Assets存储的Mesh分箱键。
    fn bin(asset_id: AssetId<Mesh>) -> Opaque3dBinKey {
        Opaque3dBinKey {
            asset_id: asset_id.untyped(),
        }
    }

    /// 构造可见实体主世界身份。
    fn main_entity(entity: u32) -> MainEntity {
        MainEntity::from(Entity::from_raw_u32(entity).unwrap())
    }

    #[test] // MDI应按视图、批集和Mesh去重，并与普通分箱共享视图计数。
    fn summarize_phase_preserves_exact_mdi_group_bin_and_instance_semantics() {
        let view_a = retained_view(1);
        let view_b = retained_view(2);
        let normal_batch = batch_set(10);
        let mdi_batch = batch_set(20);
        let mesh_a = bin(AssetId::<Mesh>::default());
        let mesh_b = bin(AssetId::<Mesh>::invalid());
        let mut phases = ViewBinnedRenderPhases::<Opaque3d>::default();
        phases.prepare_for_new_frame(view_a, GpuPreprocessingMode::None);
        phases
            .get_mut(&view_a)
            .expect("test view was prepared")
            .add(
                normal_batch,
                mesh_a.clone(),
                (Entity::PLACEHOLDER, main_entity(11)),
                InputUniformIndex::default(),
                BinnedRenderPhaseType::BatchableMesh,
            ); // 同一视图先放入一个公开普通批次。
        let mdi_entries = vec![
            PbrMdiPhaseEntry {
                view: view_a,
                entity: main_entity(12),
                batch_set_key: mdi_batch.clone(),
                bin_key: mesh_b.clone(),
            },
            PbrMdiPhaseEntry {
                view: view_a,
                entity: main_entity(13),
                batch_set_key: mdi_batch.clone(),
                bin_key: mesh_b.clone(),
            }, // 同一批集同一Mesh的第二实体只能增加实例数。
            PbrMdiPhaseEntry {
                view: view_a,
                entity: main_entity(14),
                batch_set_key: mdi_batch.clone(),
                bin_key: mesh_a.clone(),
            }, // 同一批集的另一Mesh增加一个bin但不增加提交组。
            PbrMdiPhaseEntry {
                view: view_b,
                entity: main_entity(15),
                batch_set_key: mdi_batch.clone(),
                bin_key: mesh_b.clone(),
            }, // 相同批集键进入另一视图时形成独立提交组和bin。
            PbrMdiPhaseEntry {
                view: view_b,
                entity: main_entity(99),
                batch_set_key: mdi_batch,
                bin_key: mesh_b,
            }, // 实体过滤拒绝的条目不能影响任何统计维度。
        ];

        let stats = summarize_phase(
            &phases,
            &mdi_entries,
            |_| true,
            |entity| entity != main_entity(99),
        );

        assert_eq!(stats.views, 2); // view_a的普通与MDI命中必须合并，view_b另计一次。
        assert_eq!(stats.draw_groups, 3); // 一个普通批次，加两个按视图区分的MDI批集。
        assert_eq!(stats.bins, 4); // 一个普通bin，加三个按视图/批集/Mesh去重的MDI bin。
        assert_eq!(stats.instances, 5); // 一个普通实体和四个通过过滤的MDI实体。
    }
}
