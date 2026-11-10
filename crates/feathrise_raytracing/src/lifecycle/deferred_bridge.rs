//! Solari活动期的Deferred光栅兼容桥。
//!
//! 【功能设计】角色、Dither玩家、风动植物、通用地表和公告板继续由光栅管线绘制且不进入TLAS，
//! 但它们在写入深度/运动矢量时也必须写入有效GBuffer；否则Solari会把零GBuffer当成表面并产生黑色轮廓与历史残影。

use bevy::{
    asset::{Asset, AssetEvent, AssetId, UntypedAssetId}, // 按具体材质资产身份保存作者渲染方式并消费热更新。
    ecs::system::SystemParam,            // 把六类资产表与消息读取器收拢为有界系统参数。
    material::OpaqueRendererMethod, // Solari期强制Deferred，回退时恢复Forward/Deferred/Auto原值。
    pbr::{DefaultOpaqueRendererMethod, StandardMaterial}, // 新增Auto材质同帧默认按Deferred准备。
    platform::collections::{HashMap, HashSet}, // 固定256项备份表与P1源材质跳过集合。
    prelude::*,                     // Assets、MessageReader、ResMut与Solari历史查询。
};
use feathrise_environment::{
    // 动态顶点与通用地表仍光栅，但必须写入有效GBuffer。
    mesh_wind::TreeWindMaterial,
    terrain_surface::TerrainSurfaceMaterial,
};
use feathrise_scene::SparseClipmapSurfaceMaterial; // 真正显示的地表是两层 ExtendedMaterial，不能只预热隐藏内层。
use feathrise_vegetation::VegetationMeshBillboardMaterial; // AlphaMask远景公告板继续使用自有顶点Shader。
use bevy_solari::prelude::SolariLighting; // 运行期新增或重载材质后只重置一次Solari时间历史。

use crate::{
    custom_materials::CustomDeferredMaterials,
    constants::MAX_DEFERRED_RASTER_MATERIAL_COUNT, // 初始捕获与活动成员共享同一硬上限。
    resources::{
        DeferredRasterBridgeState, DeferredRasterBridgeTargets, RaytracingRuntime,
        RollbackDisposition,
    },
    types::{
        RaytracingPrototypeBlocker, RaytracingPrototypeStage, SolariHistoryResetReason,
        SourceMaterialBinding,
    },
};

/// 已知PBR资产的统一只读/可写基础材质入口；透明资产不会进入兼容目标。
trait StandardMaterialBacked: Asset {
    fn standard_material(&self) -> &StandardMaterial; // 读取AlphaMode与作者渲染方式。
    fn standard_material_mut(&mut self) -> &mut StandardMaterial; // 只改写opaque_render_method字段。
}

impl StandardMaterialBacked for StandardMaterial {
    fn standard_material(&self) -> &StandardMaterial {
        self // 普通PBR资产本身就是基础材质。
    }

    fn standard_material_mut(&mut self) -> &mut StandardMaterial {
        self // 只允许调用方修改统一基础字段。
    }
}

macro_rules! impl_extended_standard_material {
    ($material:ty) => {
        impl StandardMaterialBacked for $material {
            fn standard_material(&self) -> &StandardMaterial {
                &self.base // ExtendedMaterial统一把StandardMaterial保存在base字段。
            }

            fn standard_material_mut(&mut self) -> &mut StandardMaterial {
                &mut self.base // 扩展Uniform、纹理和Shader身份保持不变。
            }
        }
    };
}

impl_extended_standard_material!(TreeWindMaterial);
impl_extended_standard_material!(TerrainSurfaceMaterial);
impl_extended_standard_material!(VegetationMeshBillboardMaterial);

impl StandardMaterialBacked for SparseClipmapSurfaceMaterial {
    fn standard_material(&self) -> &StandardMaterial {
        &self.base.base
    }
    fn standard_material_mut(&mut self) -> &mut StandardMaterial {
        &mut self.base.base
    }
}

/// 所有兼容材质资产表；每类只保存一份全局Assets资源且稳定帧不做全表扫描。
#[derive(SystemParam)]
pub(crate) struct DeferredRasterMaterialAssets<'w> {
    standard: ResMut<'w, Assets<StandardMaterial>>, // 角色、岩石与普通场景PBR材质。
    custom: ResMut<'w, CustomDeferredMaterials>, // 应用适配器仅发布基础字段，真实资产仍由应用注册的泛型系统访问。
    tree_wind: ResMut<'w, Assets<TreeWindMaterial>>, // 树木与灌木GPU风材质。
    terrain_surface: ResMut<'w, Assets<TerrainSurfaceMaterial>>, // 通用权重地表扩展材质。
    billboard: ResMut<'w, Assets<VegetationMeshBillboardMaterial>>, // 远景动态公告板材质。
    sparse_clipmap: ResMut<'w, Assets<SparseClipmapSurfaceMaterial>>, // 正式 GPU 高度地表的嵌套基础材质。
}

/// 六类资产事件读取器；稳定帧只消费真实新增/重载/移除，不重新遍历资产表。
#[derive(SystemParam)]
pub(crate) struct DeferredRasterMaterialEvents<'w, 's> {
    standard: MessageReader<'w, 's, AssetEvent<StandardMaterial>>, // 普通PBR资产事件。
    tree_wind: MessageReader<'w, 's, AssetEvent<TreeWindMaterial>>, // 风Uniform修改也会到达但不会重复写方法。
    terrain_surface: MessageReader<'w, 's, AssetEvent<TerrainSurfaceMaterial>>, // 地表扩展热更新事件。
    billboard: MessageReader<'w, 's, AssetEvent<VegetationMeshBillboardMaterial>>, // 公告板资产事件。
    sparse_clipmap: MessageReader<'w, 's, AssetEvent<SparseClipmapSurfaceMaterial>>,
}

/// 在P1进入预热时一次捕获全部已加载材质，之后只按资产事件维护兼容成员。
pub(crate) fn maintain_deferred_raster_bridge(
    mut assets: DeferredRasterMaterialAssets, // 初始有界捕获与事件局部修改共享同一资产借用。
    mut events: DeferredRasterMaterialEvents, // 每类消息均拥有独立读取游标，不干扰原有热更系统。
    mut default_renderer: ResMut<DefaultOpaqueRendererMethod>, // 保障活动期新建Auto材质首次提取即为Deferred。
    mut bridge: ResMut<DeferredRasterBridgeState>, // 保存每份资产的作者原值和当前目标代次。
    mut targets: ResMut<DeferredRasterBridgeTargets>, // 成员变化时才重建固定小型RenderApp身份表。
    mut solari_cameras: Query<&mut SolariLighting>, // 资产真实换代后清空一次旧时间历史。
    mut runtime: ResMut<RaytracingRuntime>,        // 读取P1源跳过集合并提交失败/等待状态。
) {
    let bridge_required = matches!(
        runtime.stage,
        RaytracingPrototypeStage::WarmingSolari | RaytracingPrototypeStage::Active
    ); // 只有已经取得玩法相机租约的会话才允许接管材质。
    if runtime.pending_rollback.is_some() {
        drain_events(&mut events, &mut assets.custom); // 同帧已有回退事务时停止接纳新资产，后续系统立即统一恢复。
        return;
    }
    if !bridge_required && !bridge.installed {
        drain_events(&mut events, &mut assets.custom); // 关闭态推进读取游标，重新开启时由有界全量捕获读取当前权威资产。
        return;
    }

    if !bridge.installed
        && let Err(blocker) = install_bridge(
            &mut assets,
            &mut default_renderer,
            &mut bridge,
            &mut targets,
            &runtime,
        )
    {
        runtime.request_rollback(Some(blocker), RollbackDisposition::Failed); // 初始超限前未产生任何部分写入。
        drain_events(&mut events, &mut assets.custom); // 失败帧仍消费旧消息，防止下一次重开误把历史事件当新增。
        return;
    }

    let (skipped_standard, skipped_surface) = p1_source_materials(&runtime); // 原始P1材质由实体句柄副本事务单独接管。
    let mut tracked_total = tracked_material_count(&bridge); // 后续六类新增共同消耗同一256项硬上限。
    let mut membership_changed = false; // 只有成员增删才重建提取目标并推进代次。
    let mut material_reprepared = false; // 方法被热更覆盖时需要重置一次Solari历史。
    let maintenance = maintain_asset_events(
        events.standard.read(),
        &mut assets.standard,
        &mut bridge.standard,
        &skipped_standard,
        &mut tracked_total,
        &mut membership_changed,
        &mut material_reprepared,
    )
    .and_then(|_| {
        maintain_asset_events(
            events.tree_wind.read(),
            &mut assets.tree_wind,
            &mut bridge.tree_wind,
            &HashSet::default(),
            &mut tracked_total,
            &mut membership_changed,
            &mut material_reprepared,
        )
    })
    .and_then(|_| {
        maintain_asset_events(
            events.terrain_surface.read(),
            &mut assets.terrain_surface,
            &mut bridge.terrain_surface,
            &skipped_surface,
            &mut tracked_total,
            &mut membership_changed,
            &mut material_reprepared,
        )
    })
    .and_then(|_| {
        maintain_asset_events(
            events.billboard.read(),
            &mut assets.billboard,
            &mut bridge.billboard,
            &HashSet::default(),
            &mut tracked_total,
            &mut membership_changed,
            &mut material_reprepared,
        )
    })
    .and_then(|_| {
        maintain_asset_events(
            events.sparse_clipmap.read(),
            &mut assets.sparse_clipmap,
            &mut bridge.sparse_clipmap,
            &HashSet::default(),
            &mut tracked_total,
            &mut membership_changed,
            &mut material_reprepared,
        )
    })
    .and_then(|_| maintain_custom_materials(
        &mut assets.custom, &mut bridge.custom, &mut tracked_total,
        &mut membership_changed, &mut material_reprepared,
    ));
    if let Err(actual) = maintenance {
        runtime.request_rollback(
            Some(
                RaytracingPrototypeBlocker::DeferredRasterMaterialBudgetExceeded {
                    actual,                                      // 最多只访问或接纳到第257项。
                    maximum: MAX_DEFERRED_RASTER_MATERIAL_COUNT, // 固定硬上限不能运行时扩大。
                },
            ),
            RollbackDisposition::Failed,
        ); // 动态超限也必须完整回退，不能只保留先到的材质子集。
        return;
    }

    if membership_changed {
        publish_bridge_targets(&mut bridge, &mut targets); // 新集合先进入RenderApp，再由预热证据决定首次挂接Solari。
    }
    if (membership_changed || material_reprepared)
        && runtime.solari_inserted
        && let Some(camera) = runtime.camera
        && let Ok(mut lighting) = solari_cameras.get_mut(camera)
    {
        let reason = if membership_changed {
            SolariHistoryResetReason::MaterialMembershipChanged // 新增/移除资产会改变哪些深度像素拥有有效GBuffer。
        } else {
            SolariHistoryResetReason::MaterialReprepared // 同一成员热更后不能复用旧GBuffer语义。
        };
        runtime.note_solari_history_reset(reason); // PostUpdate把同帧材质与相机来源合并后只累计一次。
        if !lighting.reset {
            lighting.reset = true; // 新材质或热更不能复用包含旧GBuffer语义的时间历史。
        }
    }

    if runtime.stage == RaytracingPrototypeStage::WarmingSolari && !runtime.solari_inserted {
        runtime.blocker = Some(RaytracingPrototypeBlocker::DeferredRasterBridgePreparing {
            ready: runtime.render_telemetry.deferred_bridge_ready_count, // F1显示上一完整RenderApp帧的真实值。
            total: tracked_total.min(u32::MAX as usize) as u32, // 异常平台也使用饱和诊断而非回绕。
        });
    }
}

/// 回退事务前恢复全局Auto默认与每个具体材质作者值；核心回退随后恢复实体句柄和相机租约。
pub(crate) fn restore_deferred_raster_bridge(
    mut assets: DeferredRasterMaterialAssets, // 只触碰本会话记录过的固定上限资产ID。
    mut default_renderer: ResMut<DefaultOpaqueRendererMethod>, // 恢复插件明确建立的Forward默认。
    mut bridge: ResMut<DeferredRasterBridgeState>, // 一次drain全部作者备份，重复回退保持幂等。
    mut targets: ResMut<DeferredRasterBridgeTargets>, // 清空RenderApp目标并推进代次拒绝旧就绪帧。
    mut runtime: ResMut<RaytracingRuntime>, // 只在核心生命周期已提交回退事务时执行并清空旧Render证据。
) {
    if runtime.pending_rollback.is_none() || !bridge.installed {
        return; // 稳定帧不改写DefaultOpaqueRendererMethod或任何材质资产。
    }

    default_renderer.set_to_forward(); // Auto材质恢复后将在同一提取帧重新按光栅Forward准备。
    restore_assets(&mut assets.standard, &mut bridge.standard); // 普通角色、岩石和场景材质。
    for (id, original) in bridge.custom.drain() {
        if assets.custom.records.contains_key(&id) { assets.custom.writes.insert(id, original); }
    } // 游戏材质也在同帧恢复作者原值，后续显式适配器负责写回自身资产类型。
    restore_assets(&mut assets.tree_wind, &mut bridge.tree_wind); // 风动树木/灌木作者值。
    restore_assets(&mut assets.terrain_surface, &mut bridge.terrain_surface); // 通用地表作者值。
    restore_assets(&mut assets.billboard, &mut bridge.billboard); // 远景公告板作者值。
    restore_assets(&mut assets.sparse_clipmap, &mut bridge.sparse_clipmap); // 高度、纹理与作者 opaque 方法一起恢复。
    bridge.installed = false; // 从此行起关闭态不再拥有材质渲染方式。
    publish_bridge_targets(&mut bridge, &mut targets); // 发布空集合并使RenderApp连续就绪帧归零。
    runtime.render_telemetry.deferred_bridge_target_count = 0; // F1回退完成帧不继续显示旧会话目标。
    runtime.render_telemetry.deferred_bridge_ready_count = 0; // 旧PreparedMaterial证据立即失效。
    runtime.render_telemetry.deferred_bridge_ready_frames = 0; // 下一次会话必须重新累计完整帧。
}

/// 初始捕获先完整验证数量，再统一修改资产，确保第257项不会留下部分Deferred状态。
fn install_bridge(
    assets: &mut DeferredRasterMaterialAssets,
    default_renderer: &mut DefaultOpaqueRendererMethod,
    bridge: &mut DeferredRasterBridgeState,
    targets: &mut DeferredRasterBridgeTargets,
    runtime: &RaytracingRuntime,
) -> Result<(), RaytracingPrototypeBlocker> {
    let (skipped_standard, skipped_surface) = p1_source_materials(runtime); // P1源实体即将统一切到既有Deferred副本。
    let mut visited = 0usize; // 五类资产合计最多读取硬上限加一项。
    let standard = capture_assets(&assets.standard, &skipped_standard, &mut visited)?;
    visited = visited.saturating_add(assets.custom.loaded_counts.values().sum::<usize>());
    if visited > MAX_DEFERRED_RASTER_MATERIAL_COUNT {
        return Err(RaytracingPrototypeBlocker::DeferredRasterMaterialBudgetExceeded {
            actual: MAX_DEFERRED_RASTER_MATERIAL_COUNT + 1, maximum: MAX_DEFERRED_RASTER_MATERIAL_COUNT,
        });
    }
    let tree_wind = capture_assets(&assets.tree_wind, &HashSet::default(), &mut visited)?;
    let terrain_surface = capture_assets(&assets.terrain_surface, &skipped_surface, &mut visited)?;
    let billboard = capture_assets(&assets.billboard, &HashSet::default(), &mut visited)?;
    let sparse_clipmap = capture_assets(&assets.sparse_clipmap, &HashSet::default(), &mut visited)?;

    default_renderer.set_to_deferred(); // 从本帧起新增Auto材质无需等待事件即可首次按Deferred准备。
    force_deferred(&mut assets.standard, &mut bridge.standard, standard); // 普通PBR兼容目标。
    let custom: Vec<_> = assets.custom.records.iter().filter(|(_, record)| record.writes_gbuffer())
        .map(|(id, record)| (*id, record.method)).collect();
    for (id, original) in custom {
        bridge.custom.insert(id, original);
        if original != OpaqueRendererMethod::Deferred {
            let method = if original == OpaqueRendererMethod::Auto { original } else { OpaqueRendererMethod::Deferred };
            assets.custom.writes.insert(id, method);
        }
    }
    force_deferred(&mut assets.tree_wind, &mut bridge.tree_wind, tree_wind); // 动态风姿态写入GBuffer。
    force_deferred(
        &mut assets.terrain_surface,
        &mut bridge.terrain_surface,
        terrain_surface,
    ); // 地形扩展继续保留自有WGSL。
    force_deferred(&mut assets.billboard, &mut bridge.billboard, billboard); // AlphaMask公告板保留裁切语义。
    force_deferred(
        &mut assets.sparse_clipmap,
        &mut bridge.sparse_clipmap,
        sparse_clipmap,
    );
    bridge.installed = true; // 全部六类成功后才公开会话所有权。
    publish_bridge_targets(bridge, targets); // RenderApp至少确认两帧后才允许挂接Solari。
    Ok(())
}

/// 有界读取一类资产；透明/透射材质计入访问预算但不进入GBuffer兼容成员。
fn capture_assets<M: StandardMaterialBacked>(
    assets: &Assets<M>,
    skipped: &HashSet<AssetId<M>>,
    visited: &mut usize,
) -> Result<Vec<(AssetId<M>, OpaqueRendererMethod)>, RaytracingPrototypeBlocker> {
    let mut captured = Vec::new(); // 只有全部类型验证成功后才采用该临时快照。
    for (id, asset) in assets.iter() {
        *visited = visited.saturating_add(1); // 初始成本随已加载材质增长但严格受257项停止线限制。
        if *visited > MAX_DEFERRED_RASTER_MATERIAL_COUNT {
            return Err(
                RaytracingPrototypeBlocker::DeferredRasterMaterialBudgetExceeded {
                    actual: *visited, // 只需证明至少257项，不继续扫描得出无界精确总数。
                    maximum: MAX_DEFERRED_RASTER_MATERIAL_COUNT,
                },
            );
        }
        let base = asset.standard_material(); // 所有扩展类型统一读取StandardMaterial基础层。
        if skipped.contains(&id) || !writes_deferred_gbuffer(base) {
            continue; // P1原始句柄由副本事务接管；透明路径不读取Deferred GBuffer。
        }
        captured.push((id, base.opaque_render_method)); // 保存作者Forward/Deferred/Auto精确值。
    }
    Ok(captured)
}

/// 把完整捕获结果应用到一类资产；Auto保持作者字段不变但触发一次重新准备以读取当前全局Deferred默认。
fn force_deferred<M: StandardMaterialBacked>(
    assets: &mut Assets<M>,
    backups: &mut HashMap<AssetId<M>, OpaqueRendererMethod>,
    captured: Vec<(AssetId<M>, OpaqueRendererMethod)>,
) {
    for (id, original) in captured {
        backups.insert(id, original); // 先建立回退所有权，再允许修改资产字段。
        if original == OpaqueRendererMethod::Deferred {
            continue; // 已经显式Deferred的P1会话副本无需重写，避免制造会被源热更监视器看到的自发Modified事件。
        }
        let Some(mut asset) = assets.get_mut(id) else {
            continue; // 同帧外部移除由后续资产事件收敛，不能制造伪句柄。
        };
        let base = asset.standard_material_mut(); // 取得可写资产即让RenderApp重新准备本材质。
        if original == OpaqueRendererMethod::Auto {
            base.opaque_render_method = OpaqueRendererMethod::Auto; // 保持可继承语义，当前全局默认会把准备结果解析为Deferred。
        } else {
            base.opaque_render_method = OpaqueRendererMethod::Deferred; // 显式Forward不能依赖全局默认，临时覆盖并保存原值。
        }
    }
}

/// 增量维护一类资产事件；返回第257项证据并由外层触发完整失败回退。
#[allow(clippy::too_many_arguments)]
fn maintain_asset_events<'a, M: StandardMaterialBacked + 'a>(
    events: impl Iterator<Item = &'a AssetEvent<M>>,
    assets: &mut Assets<M>,
    backups: &mut HashMap<AssetId<M>, OpaqueRendererMethod>,
    skipped: &HashSet<AssetId<M>>,
    tracked_total: &mut usize,
    membership_changed: &mut bool,
    material_reprepared: &mut bool,
) -> Result<(), usize> {
    for event in events {
        let id = asset_event_id(event); // 所有事件变体都保留稳定类型化AssetId。
        match event {
            AssetEvent::Removed { .. } => {
                if backups.remove(&id).is_some() {
                    *tracked_total = tracked_total.saturating_sub(1); // 移除项立即释放固定成员预算。
                    *membership_changed = true; // RenderApp目标必须删除已不存在的资产。
                }
                continue;
            }
            AssetEvent::Unused { .. } => continue, // 弱句柄归零不等于资产已经从Assets移除。
            AssetEvent::Added { .. }
            | AssetEvent::Modified { .. }
            | AssetEvent::LoadedWithDependencies { .. } => {}
        }
        if skipped.contains(&id) {
            continue; // P1源资产消息仍由全部候选热更系统决定是否整体重建。
        }
        let Some(asset) = assets.get(id) else {
            continue; // Added消息与资产删除交错时等待后续权威事件。
        };
        if !writes_deferred_gbuffer(asset.standard_material()) {
            if let Some(original) = backups.remove(&id) {
                if let Some(mut asset) = assets.get_mut(id) {
                    asset.standard_material_mut().opaque_render_method = original; // 离开Opaque/Mask时立即归还作者方式。
                }
                *tracked_total = tracked_total.saturating_sub(1); // 透明路径不占兼容成员预算。
                *membership_changed = true;
                *material_reprepared = true;
            }
            continue;
        }

        let current = asset.standard_material().opaque_render_method; // 先在不可变借用中冻结热更后的作者值。
        if let Some(original) = backups.get_mut(&id) {
            if current == OpaqueRendererMethod::Forward {
                *original = OpaqueRendererMethod::Forward; // 记录热更后的最新显式作者方式。
                if let Some(mut asset) = assets.get_mut(id) {
                    asset.standard_material_mut().opaque_render_method =
                        OpaqueRendererMethod::Deferred; // 显式Forward必须由本桥临时覆盖。
                }
                *material_reprepared = true;
            } else if current == OpaqueRendererMethod::Auto
                && *original != OpaqueRendererMethod::Auto
            {
                *original = OpaqueRendererMethod::Auto; // Auto字段保持不变，资产事件已按当前全局Deferred重新准备。
                *material_reprepared = true;
            }
            continue; // 风Uniform或Dither alpha普通变化不会重复写opaque_render_method。
        }

        if *tracked_total >= MAX_DEFERRED_RASTER_MATERIAL_COUNT {
            return Err(tracked_total.saturating_add(1)); // 不采用第257项，外层立即完整回退已有集合。
        }
        backups.insert(id, current); // 新资产保存首次作者方式供关闭/切换场景精确恢复。
        if current == OpaqueRendererMethod::Forward
            && let Some(mut asset) = assets.get_mut(id)
        {
            asset.standard_material_mut().opaque_render_method = OpaqueRendererMethod::Deferred; // 新增Auto由全局默认正确准备，只有显式Forward需要字段覆盖。
            *material_reprepared = true;
        }
        *tracked_total = tracked_total.saturating_add(1); // 成功采用后才消耗成员预算。
        *membership_changed = true;
    }
    Ok(())
}

/// 透明与透射主通道不读取Deferred GBuffer；这里只接管Opaque、Mask和AlphaToCoverage。
fn writes_deferred_gbuffer(material: &StandardMaterial) -> bool {
    matches!(
        material.alpha_mode,
        AlphaMode::Opaque | AlphaMode::Mask(_) | AlphaMode::AlphaToCoverage
    ) // Dither使用Opaque基础材质，其屏幕门控由deferred_fragment_shader保持一致。
}

/// 从全部候选准备事务提取原始材质ID；这些资产本身不修改，源实体改用既有Deferred副本。
fn p1_source_materials(
    runtime: &RaytracingRuntime,
) -> (
    HashSet<AssetId<StandardMaterial>>,
    HashSet<AssetId<TerrainSurfaceMaterial>>,
) {
    let mut standard = HashSet::default(); // 九项普通源实际去重为三份资产，地形由通用扩展材质独立处理。
    let mut surface = HashSet::default(); // 当前目标地形只有一份通用扩展源资产。
    for prepared in &runtime.prepared_materials {
        match &prepared.binding {
            SourceMaterialBinding::Standard { source, .. } => {
                standard.insert(source.id()); // 跳过原始GLB资产，避免把自有修改误判成源热更。
            }
            SourceMaterialBinding::TerrainSurface { source, .. } => {
                surface.insert(source.id()); // 可见实体已切到Deferred扩展副本。
            }
        }
    }
    (standard, surface)
}

/// 只在目标成员变化时重建固定小型ID向量并推进代次。
fn publish_bridge_targets(
    bridge: &mut DeferredRasterBridgeState,
    targets: &mut DeferredRasterBridgeTargets,
) {
    bridge.revision = bridge.revision.wrapping_add(1).max(1); // 零代次保留给从未安装状态。
    targets.revision = bridge.revision; // 主世界和RenderApp使用同一不可混用代次。
    targets.ids.clear(); // Vec容量跨热更与回退复用，不在稳定帧分配。
    targets.ids.reserve(tracked_material_count(bridge)); // 最多256个UntypedAssetId。
    targets
        .ids
        .extend(bridge.standard.keys().map(|id| (*id).untyped())); // 普通PBR目标。
    targets
        .ids
        .extend(bridge.custom.keys().copied()); // 应用材质携带真实类型化身份，不生成替代资产。
    targets
        .ids
        .extend(bridge.tree_wind.keys().map(|id| (*id).untyped())); // 风材质目标。
    targets
        .ids
        .extend(bridge.terrain_surface.keys().map(|id| (*id).untyped())); // 地表目标。
    targets
        .ids
        .extend(bridge.billboard.keys().map(|id| (*id).untyped())); // 公告板目标。
    targets
        .ids
        .extend(bridge.sparse_clipmap.keys().map(|id| (*id).untyped())); // 核对真实可见嵌套地表的 PreparedMaterial。
}

/// 返回当前六类成员总数；所有调用点都保持不超过固定256项。
fn tracked_material_count(bridge: &DeferredRasterBridgeState) -> usize {
    bridge.standard.len()
        + bridge.custom.len()
        + bridge.tree_wind.len()
        + bridge.terrain_surface.len()
        + bridge.billboard.len()
        + bridge.sparse_clipmap.len()
}

/// 恢复一类资产的作者渲染方式；已随热更移除的ID只丢弃备份。
fn restore_assets<M: StandardMaterialBacked>(
    assets: &mut Assets<M>,
    backups: &mut HashMap<AssetId<M>, OpaqueRendererMethod>,
) {
    for (id, original) in backups.drain() {
        let Some(mut asset) = assets.get_mut(id) else {
            continue; // 删除资产不存在可恢复对象，强行重建会违反热更所有权。
        };
        let base = asset.standard_material_mut(); // 即使值仍为Auto也必须触发RenderApp按已恢复的全局Forward重新准备。
        base.opaque_render_method = original; // Auto/Forward/Deferred逐项精确恢复。
    }
}

/// 关闭态只推进事件游标，不做资产读取或修改。
fn drain_events(events: &mut DeferredRasterMaterialEvents, custom: &mut CustomDeferredMaterials) {
    custom.changed.clear(); // 关闭态不累积应用的历史事件，重新安装读取当前有界快照。
    events.standard.read().for_each(drop); // 普通PBR消息。
    events.tree_wind.read().for_each(drop); // 风材质消息。
    events.terrain_surface.read().for_each(drop); // 通用地表消息。
    events.billboard.read().for_each(drop); // 公告板消息。
    events.sparse_clipmap.read().for_each(drop);
}

/// 应用资产也参与同一预算、目标身份及回退事务；只消费桥前收集的变化集合。
fn maintain_custom_materials(
    custom: &mut CustomDeferredMaterials,
    backups: &mut HashMap<UntypedAssetId, OpaqueRendererMethod>,
    tracked_total: &mut usize,
    membership_changed: &mut bool,
    material_reprepared: &mut bool,
) -> Result<(), usize> {
    if !custom.overflowed.is_empty() {
        custom.changed.clear();
        return Err(MAX_DEFERRED_RASTER_MATERIAL_COUNT + 1);
    }
    for id in custom.changed.drain() {
        let record = custom.records.get(&id).copied();
        if record.is_none_or(|record| !record.writes_gbuffer()) {
            if let Some(original) = backups.remove(&id) {
                if record.is_some() { custom.writes.insert(id, original); }
                *tracked_total = tracked_total.saturating_sub(1);
                *membership_changed = true;
                *material_reprepared = true;
            }
            continue;
        }
        let current = record.unwrap().method;
        if let Some(original) = backups.get_mut(&id) {
            if current == OpaqueRendererMethod::Forward {
                *original = current;
                custom.writes.insert(id, OpaqueRendererMethod::Deferred);
                *material_reprepared = true;
            } else if current == OpaqueRendererMethod::Auto && *original != OpaqueRendererMethod::Auto {
                *original = current;
                *material_reprepared = true;
            }
            continue;
        }
        if *tracked_total >= MAX_DEFERRED_RASTER_MATERIAL_COUNT {
            return Err(MAX_DEFERRED_RASTER_MATERIAL_COUNT + 1);
        }
        backups.insert(id, current);
        if current == OpaqueRendererMethod::Forward {
            custom.writes.insert(id, OpaqueRendererMethod::Deferred);
            *material_reprepared = true;
        }
        *tracked_total += 1;
        *membership_changed = true;
    }
    Ok(())
}

/// 统一提取AssetEvent身份；Unused只用于推进游标且不会删除备份。
fn asset_event_id<M: Asset>(event: &AssetEvent<M>) -> AssetId<M> {
    match *event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id }
        | AssetEvent::LoadedWithDependencies { id } => id,
    }
}

#[cfg(test)]
mod tests; // Deferred覆盖、精确恢复、透明隔离与固定捕获上限使用独立夹具。
