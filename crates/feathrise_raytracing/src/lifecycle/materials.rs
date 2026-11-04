//! 全部候选材质副本、无 `Mesh3d` 子代理、GPU绑定预热与原子可见切换。

use bevy::{
    asset::AssetId,                            // 普通StandardMaterial按源AssetId去重克隆。
    gltf::{GltfMaterialName, GltfMeshName},    // 每阶段重新核对源双身份。
    material::OpaqueRendererMethod,            // StandardMaterial显式Forward/Deferred选择。
    pbr::{MeshMaterial3d, StandardMaterial},   // 类型化材质切换。
    platform::collections::{HashMap, HashSet}, // 全部候选内的普通源材质缓存与真实AssetId审计。
    prelude::*,                                // Commands、Assets、Mesh3d与层级组件。
};
use bevy_solari::prelude::{RaytracingMesh3d, SolariLighting};
use feathrise_environment::terrain_surface::{
    TerrainSurfaceCarrier, TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride,
    TerrainVolumeSurface,
}; // 动态地形必须等待通用权重地表完成正式承接，并显式仲裁Deferred临时替代。
use feathrise_settings::{GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements}; // 相机要求只能通过通用租约声明。 // 代理只进TLAS，目标相机启用实时光照。

use crate::{
    components::RaytracingProxy,
    constants::{REQUIRED_DEFERRED_RASTER_READY_FRAMES, REQUIRED_WARMUP_RENDER_FRAMES},
    resources::{DeferredRasterBridgeState, RaytracingRuntime, RollbackDisposition},
    types::{
        PreparedSourceMaterial, RaytracingPrototypeBlocker, RaytracingPrototypeStage,
        RaytracingPrototypeTarget, SourceMaterialBinding,
    },
};

type SourceIdentityData<'a> = (
    Option<&'a GltfMeshName>,
    Option<&'a GltfMaterialName>,
    &'a Mesh3d,
);
type SurfaceSourceData<'a> = (
    &'a MeshMaterial3d<TerrainSurfaceMaterial>,
    Option<&'a TerrainSurfaceMaterialOverride>,
);
type SurfaceSourceFilter = Or<(
    With<TerrainSurfaceCarrier>,
    With<feathrise_scene::ClipmapRaytracingAnchor>,
    With<TerrainVolumeSurface>,
)>; // CPU 锚点不具备可见地表身份。

mod validation; // 源身份、事务完整性和活动期核对独立维护，避免材质装配文件超过职责上限。
pub(crate) use validation::validate_active_sources;
#[cfg(test)]
use validation::{expected_material_id, prepared_transaction_has_expected_count};
use validation::{
    handle_gpu_preparation_blocker, prepared_sources_still_match, validate_candidate_identity,
    validate_opaque_material, validate_source_material_identity_count,
};

/// 在 `AwaitingGpuAssets` 创建完整暂存事务，先稳定Deferred光栅桥，再挂接Solari观察两帧并保留光栅内容标准阴影。
#[allow(clippy::too_many_arguments)] // 参数逐项对应普通材质、扩展材质、源身份和租约所有权。
pub(crate) fn drive_gpu_preparation_and_warmup(
    mut commands: Commands, // 生成无Mesh3d代理、挂接Solari并在成功帧替换全部可见材质。
    mut standard_materials: ResMut<Assets<StandardMaterial>>, // 普通延迟副本与地形静态代理材质。
    mut surface_materials: ResMut<Assets<TerrainSurfaceMaterial>>, // 通用地表延迟扩展副本。
    identities: Query<SourceIdentityData<'_>>, // 预热与激活前核对源实体身份未换代。
    standard_sources: Query<&MeshMaterial3d<StandardMaterial>>, // 九项村落/村路普通材质。
    surface_sources: Query<SurfaceSourceData<'_>, SurfaceSourceFilter>, // 两类源各自以明确标记取得资格。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 只通过设置crate声明相机与阴影覆盖。
    bridge: Res<DeferredRasterBridgeState>, // Solari挂接前必须确认全部光栅材质已在RenderApp准备为Deferred。
    mut runtime: ResMut<RaytracingRuntime>, // 保存完整可撤销事务和预热帧证据。
) {
    match runtime.stage {
        RaytracingPrototypeStage::AwaitingGpuAssets => {
            if !runtime.gpu_assets_prepared {
                if let Err(blocker) = prepare_all_materials_and_proxies(
                    &mut commands,
                    &mut standard_materials,
                    &mut surface_materials,
                    &identities,
                    &standard_sources,
                    &surface_sources,
                    &mut runtime,
                ) {
                    handle_gpu_preparation_blocker(&mut runtime, blocker); // 仅真实异步资源等待可留在本阶段。
                    return;
                }
                runtime.blocker = Some(RaytracingPrototypeBlocker::GpuBindingsUnavailable); // 代理下一帧才会提取并建立BLAS/TLAS。
                return;
            }
            if runtime.render_telemetry.proxy_count != runtime.expected_instance_count()
                || !runtime.render_telemetry.bindings_ready
                || !crate::telemetry::geometry_ready(&runtime)
            {
                runtime.blocker = Some(RaytracingPrototypeBlocker::GpuBindingsUnavailable);
                return; // bind group缺失时可见源仍Forward，标准阴影仍开启。
            }
            let Some(camera) = runtime.camera else {
                runtime.request_rollback(
                    Some(RaytracingPrototypeBlocker::WaitingForGameplayCamera),
                    RollbackDisposition::RasterReady,
                );
                return;
            };
            if !prepared_sources_still_match(
                &runtime,
                &identities,
                &standard_sources,
                &surface_sources,
            ) {
                runtime.request_rollback(
                    Some(RaytracingPrototypeBlocker::SourceChanged),
                    RollbackDisposition::RasterReady,
                );
                return; // 切换可见材质前再次核对全部候选实体与原句柄仍属于同一地图代次。
            }
            let lease = leases.acquire(GraphicsRuntimeRequirements::solari_warmup(camera)); // 预热先保留标准阴影。
            runtime.lease = Some(lease); // 回退路径持有精确票据。
            switch_visible_sources_to_deferred(&mut commands, &runtime); // Solari读取任何深度像素前，P1可见源先写入有效GBuffer。
            runtime.visible_materials_activated = true; // 从本行起回退必须恢复全部候选原始材质句柄。
            runtime.lease_settle_pending = true; // 本帧PostUpdate捕获作者相机状态，随后等待兼容材质RenderApp就绪。
            runtime.blocker = Some(RaytracingPrototypeBlocker::DeferredRasterBridgePreparing {
                ready: 0,
                total: 0,
            }); // 下一系统安装固定上限兼容桥并发布真实总数。
            runtime.transition(RaytracingPrototypeStage::WarmingSolari);
            return;
        }
        RaytracingPrototypeStage::WarmingSolari => {}
        _ => return, // 光栅、适配、活动和回退阶段不执行材质冷启动逻辑。
    }

    let Some(camera) = runtime.camera else {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::WaitingForGameplayCamera),
            RollbackDisposition::RasterReady,
        );
        return;
    };
    if runtime.lease_settle_pending {
        let bridge_target_count = bridge
            .standard
            .len()
            .saturating_add(bridge.custom.len())
            .saturating_add(bridge.tree_wind.len())
            .saturating_add(bridge.terrain_surface.len())
            .saturating_add(bridge.billboard.len())
            .saturating_add(bridge.sparse_clipmap.len())
            .min(u32::MAX as usize) as u32; // F1和RenderApp都使用饱和值，禁止usize截断。
        let render = runtime.render_telemetry;
        let bridge_ready = bridge.installed
            && render.deferred_bridge_revision == bridge.revision
            && render.deferred_bridge_target_count == bridge_target_count
            && render.deferred_bridge_ready_count == bridge_target_count
            && render.deferred_bridge_ready_frames >= REQUIRED_DEFERRED_RASTER_READY_FRAMES;
        if !bridge_ready {
            runtime.blocker = Some(RaytracingPrototypeBlocker::DeferredRasterBridgePreparing {
                ready: render.deferred_bridge_ready_count, // 只显示上一完整Render帧实际准备数量。
                total: bridge_target_count,                // 初始安装完成后固定不超过256。
            });
            return; // 至少两个完整Deferred光栅帧后才允许Solari读取GBuffer。
        }
        commands.entity(camera).insert(SolariLighting::default()); // 作者快照已在上一帧PostUpdate完成，required组件不污染基线。
        runtime.lease_settle_pending = false;
        runtime.solari_inserted = true;
        runtime.warmup_frames = 0;
        runtime.last_warmup_render_frame = runtime.render_telemetry.render_frame; // 只接受插入后产生的新完整帧。
        runtime.last_warmup_gpu_diagnostic_frame = runtime.gpu_diagnostic_frame; // 旧会话残留诊断即使仍新鲜也不能计入本次预热。
        runtime.blocker = None; // 兼容材质和Solari入口均已提交，随后只等待四段GPU诊断。
        return;
    }
    if !runtime.solari_inserted {
        return; // 防御外部移除与命令尚未落地的极短窗口，由活动验证或超时回退。
    }
    let render = runtime.render_telemetry;
    let complete_frame = render.render_frame > runtime.last_warmup_render_frame
        && runtime.gpu_diagnostic_frame > runtime.last_warmup_gpu_diagnostic_frame
        && render.proxy_count == runtime.expected_instance_count()
        && render.bindings_ready
        && crate::telemetry::geometry_ready(&runtime)
        && render.solari_camera_count == 1
        && runtime.gpu_timings.all_present()
        && runtime.gpu_diagnostics_fresh; // 四段诊断必须来自当前半秒内的真实管线执行。
    if !complete_frame {
        return; // 不重复消费旧快照，也不提前切换可见材质。
    }
    runtime.last_warmup_render_frame = render.render_frame;
    runtime.last_warmup_gpu_diagnostic_frame = runtime.gpu_diagnostic_frame; // 下一次必须等待四段诊断全部再次更新。
    runtime.warmup_frames = runtime.warmup_frames.saturating_add(1);
    if runtime.warmup_frames < REQUIRED_WARMUP_RENDER_FRAMES {
        return; // 第一完整Solari诊断帧后仍保留标准阴影，兼容材质已经稳定Deferred。
    }
    if !prepared_sources_still_match(&runtime, &identities, &standard_sources, &surface_sources) {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::SourceChanged),
            RollbackDisposition::RasterReady,
        );
        return;
    }

    let Some(lease) = runtime.lease else {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::WaitingForGameplayCamera),
            RollbackDisposition::Failed,
        );
        return;
    };
    // 【原子激活】P3角色和GPU叶片不进入TLAS，活动态必须继续生成标准shadow map，
    // 由项目的Solari后兼容层消费；这里只更新同一相机租约并提交活动阶段。
    if !leases.update(lease, GraphicsRuntimeRequirements::solari_warmup(camera)) {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::WaitingForGameplayCamera),
            RollbackDisposition::Failed,
        );
        return;
    }
    runtime.blocker = None;
    runtime.transition(RaytracingPrototypeStage::Active); // Solari诊断与保留标准投影的租约在同帧提交为活动态。
}

/// 把全部候选可见源统一切到准备好的Deferred副本；标准阴影仍保留到Solari两帧诊断完成。
fn switch_visible_sources_to_deferred(commands: &mut Commands, runtime: &RaytracingRuntime) {
    for prepared in &runtime.prepared_materials {
        match &prepared.binding {
            SourceMaterialBinding::Standard { deferred, .. } => {
                commands
                    .entity(prepared.source_entity)
                    .insert(MeshMaterial3d(deferred.clone())); // 九项普通源共享三份Deferred副本。
            }
            SourceMaterialBinding::TerrainSurface {
                source, deferred, ..
            } => {
                commands.entity(prepared.source_entity).insert((
                    MeshMaterial3d(deferred.clone()),
                    TerrainSurfaceMaterialOverride::new(source, deferred),
                )); // 句柄与精确租约同批提交，动态地表稳定核对不会逐帧抢回作者材质。
            }
        }
    }
}

/// 先验证全部全部候选资源，再创建任何新资产或代理，避免部分等待状态。
#[allow(clippy::too_many_arguments)]
fn prepare_all_materials_and_proxies(
    commands: &mut Commands,
    standard_materials: &mut Assets<StandardMaterial>,
    surface_materials: &mut Assets<TerrainSurfaceMaterial>,
    identities: &Query<SourceIdentityData<'_>>,
    standard_sources: &Query<&MeshMaterial3d<StandardMaterial>>,
    surface_sources: &Query<SurfaceSourceData<'_>, SurfaceSourceFilter>,
    runtime: &mut RaytracingRuntime,
) -> Result<(), RaytracingPrototypeBlocker> {
    if runtime.candidates.len() != runtime.expected_instance_count() as usize
        || runtime
            .candidates
            .iter()
            .any(|candidate| candidate.derived_mesh.is_none())
    {
        return Err(RaytracingPrototypeBlocker::MeshUnavailable {
            detail: format!(
                "{}项派生Mesh尚未全部登记",
                runtime.expected_instance_count()
            ),
        });
    }
    let mut unique_standard_materials = HashSet::new(); // 九项村落/村路应共享为三个真实StandardMaterial资产。
    let mut unique_surface_materials = HashSet::new(); // 白模地形接管后应额外贡献一个扩展材质资产。
    for candidate in &runtime.candidates {
        validate_candidate_identity(candidate, identities)?; // 任何源换代都在创建资产前整体停止。
        if candidate.terrain_surface {
            let Ok((handle, material_override)) = surface_sources.get(candidate.entity) else {
                return Err(RaytracingPrototypeBlocker::MaterialUnavailable {
                    detail: "等待明确地表源标记与通用扩展材质".to_string(),
                });
            };
            let Some(source_asset) = surface_materials.get(handle.id()) else {
                return Err(RaytracingPrototypeBlocker::MaterialUnavailable {
                    detail: "通用地表扩展材质或纹理资产尚未就绪".to_string(),
                });
            };
            if material_override.is_some() {
                return Err(RaytracingPrototypeBlocker::MaterialUnavailable {
                    detail: "等待上一地表Deferred替代租约释放".to_string(),
                }); // 新事务不能叠加到仍由其他会话持有的临时材质上。
            }
            validate_opaque_material(&source_asset.base, candidate.contract_index)?; // AlphaMask/Blend不得进入P1不透明契约。
            unique_surface_materials.insert(handle.id()); // 跨材质类型分组后相加，避免混淆类型化AssetId。
        } else {
            let Ok(handle) = standard_sources.get(candidate.entity) else {
                return Err(RaytracingPrototypeBlocker::MaterialUnavailable {
                    detail: format!("固定候选{}缺少StandardMaterial", candidate.contract_index),
                });
            };
            let Some(source_asset) = standard_materials.get(handle.id()) else {
                return Err(RaytracingPrototypeBlocker::MaterialUnavailable {
                    detail: format!("固定候选{}材质资产尚未加载", candidate.contract_index),
                });
            };
            validate_opaque_material(source_asset, candidate.contract_index)?; // 只允许固定静态不透明primitive进入代理集合。
            unique_standard_materials.insert(handle.id()); // 村落共享材质必须保持相同真实AssetId。
        }
    }
    runtime.budget.unique_materials =
        (unique_standard_materials.len() + unique_surface_materials.len()) as u32; // 先发布实际AssetId数量，失败帧F1也不得继续显示静态名称数。
    validate_source_material_identity_count(
        unique_standard_materials.len(), // 普通材质的真实去重数量。
        unique_surface_materials.len(),  // 通用地表扩展材质的真实去重数量。
        runtime.expected_unique_material_count() as usize, // 动态唯一地形使用一份源材质。
    )?; // 名称虽符合但资产被意外拆分时，也必须在创建副本前整体阻断。

    let mut deferred_cache: HashMap<AssetId<StandardMaterial>, Handle<StandardMaterial>> =
        HashMap::default(); // 普通源按AssetId去重克隆。
    let mut prepared = Vec::with_capacity(runtime.candidates.len());
    for candidate in &runtime.candidates {
        let binding = if candidate.terrain_surface {
            let (source_handle, _) = surface_sources
                .get(candidate.entity)
                .expect("地形地表资源已在上方预检");
            let source_asset = surface_materials
                .get(source_handle.id())
                .expect("地形地表材质已在上方预检");
            let mut deferred_asset = source_asset.clone();
            deferred_asset.base.opaque_render_method = OpaqueRendererMethod::Deferred; // 既有扩展Shader原生支持deferred。
            deferred_asset
                .extension
                .set_dynamic_terrain_shadow_terminator_data(
                    dynamic_terrain_publishes_shadow_terminator_data(
                        runtime.target,
                        candidate.contract_index,
                    ),
                ); // 仅唯一高度代理发布三角身份；体积面没有高度路由，不能把自身编号解释成高度三角。
            let mut proxy_asset = deferred_asset.base.clone();
            proxy_asset.base_color = Color::srgb(0.43, 0.31, 0.18); // Solari静态代理用高粗糙度田野杂土近似整块地形。
            proxy_asset.base_color_texture = None;
            proxy_asset.metallic = 0.0;
            proxy_asset.perceptual_roughness = 0.96;
            proxy_asset.metallic_roughness_texture = None;
            proxy_asset.opaque_render_method = OpaqueRendererMethod::Deferred; // Solari绑定器只接受StandardMaterial。
            let deferred = surface_materials.add(deferred_asset); // 本会话唯一通用地表扩展副本。
            let proxy = standard_materials.add(proxy_asset); // 只供无Mesh3d代理使用。
            runtime.created_surface_materials.push(deferred.clone());
            runtime.created_standard_materials.push(proxy.clone());
            SourceMaterialBinding::TerrainSurface {
                source: source_handle.0.clone(), // 保存回退原句柄。
                deferred,                        // 可见草地激活句柄。
                proxy,                           // TLAS代理标准材质。
            }
        } else {
            let source_handle = standard_sources
                .get(candidate.entity)
                .expect("全部普通材质已在上方预检");
            let deferred = if let Some(cached) = deferred_cache.get(&source_handle.id()) {
                cached.clone() // 同一源AssetId只创建一份延迟副本。
            } else {
                let mut asset = standard_materials
                    .get(source_handle.id())
                    .expect("全部普通材质已在上方预检")
                    .clone();
                asset.opaque_render_method = OpaqueRendererMethod::Deferred;
                let handle = standard_materials.add(asset);
                deferred_cache.insert(source_handle.id(), handle.clone());
                runtime.created_standard_materials.push(handle.clone());
                handle
            };
            SourceMaterialBinding::Standard {
                source: source_handle.0.clone(), // 保存精确原句柄。
                deferred,                        // 可见源和代理共享副本。
            }
        };
        prepared.push(PreparedSourceMaterial {
            source_entity: candidate.entity,
            binding,
        });
    }

    for (candidate, prepared_material) in runtime.candidates.iter().zip(&prepared) {
        let derived_mesh = candidate
            .derived_mesh
            .as_ref()
            .expect("全部候选派生Mesh已在上方预检")
            .clone();
        let proxy = commands
            .spawn((
                Name::new(format!(
                    "{}光追代理_{:02}",
                    runtime.target.display_name(),
                    candidate.contract_index
                )), // 世界检查器可同时辨认地图级目标与稳定槽。
                RaytracingProxy {
                    generation: runtime.generation, // 代理只属于当前地图代次。
                    source: candidate.entity,       // 保留可见源身份供诊断。
                },
                RaytracingMesh3d(derived_mesh), // 只进入Solari提取与TLAS，不插入Mesh3d。
                MeshMaterial3d(prepared_material.binding.proxy_material()), // Solari只读StandardMaterial。
                Transform::IDENTITY,       // 作为源子节点继承完整局部/全局变换。
                ChildOf(candidate.entity), // 热重载销毁源时代理随层级清理，显式回退仍主动despawn。
            ))
            .id();
        runtime.proxy_entities.push(proxy); // 回退精确撤销本crate创建的代理。
    }
    runtime.prepared_materials = prepared; // 全部候选完整创建后才发布材质事务。
    runtime.gpu_assets_prepared = true; // 后续帧只等待RenderApp绑定，不重复克隆。
    Ok(())
}

/// 动态低模高度场才需要向Deferred GBuffer发布与唯一TLAS代理同序的三角身份。
fn dynamic_terrain_publishes_shadow_terminator_data(
    target: RaytracingPrototypeTarget,
    contract_index: usize,
) -> bool {
    // 仅高度场代理携带对应三角身份，体积网格继续走普通材质路径。
    target == RaytracingPrototypeTarget::DynamicMapTerrain && contract_index == 0 // 三维体积面不能借用唯一高度路由。
} // 完成目标级光追元数据模式判定。

#[cfg(test)]
mod tests; // 材质契约与原子事务夹具独立放入同域子模块，保持运行时文件低于500行。
