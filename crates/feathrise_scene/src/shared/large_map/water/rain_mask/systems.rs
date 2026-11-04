//! 源水变化定向维护子代理，最多每帧安装64份轻量实体，不读Mesh内容。
use super::super::{components::{DynamicWaterSurface, WaterRenderChunk}, material::DynamicWaterMaterial};
use super::{
    components::RainMaskProxy,
    material::RainHeightMaterial,
    resources::{ClipmapProxyRecord, MaterialRecord, ProxyRecord, RainMaskCache},
}; // 轻量缓存与捕获材质。
use crate::maps::{ClipmapRainHeightMaterial, ClipmapWaterMaterial, ClipmapWaterSurface};
use bevy::camera::{
    primitives::Aabb,
    visibility::{NoFrustumCulling, RenderLayers},
}; // GLB复用源边界；Clipmap固定八层用片元精确裁切共享环带。
use bevy::light::{NotShadowCaster, NotShadowReceiver}; // 数值代理不参与灯光和阴影。
use bevy::prelude::*; // 资产事件、层级和变更过滤。
use feathrise_scene_core::constants::render_layers::RAIN_WATER_MASK_LAYER; // 只进入独立高度图。 // 只为真实动态水体建立代理。

pub(in super::super) fn sync_source_uniform(
    id: AssetId<DynamicWaterMaterial>,
    source: &DynamicWaterMaterial,
    cache: &RainMaskCache,
    captures: &mut Assets<RainHeightMaterial>,
) {
    // 既有水参数循环直接调用，不再扫描全部水体。
    let Some(record) = cache.materials.get(&id) else {
        return;
    }; // 尚未安装代理的材质无额外成本。
    let coverage = Vec4::new(source.base.base_color.alpha(), 0.0, 0.0, 0.0); // 完全透明原水不能生成遮雨覆盖。
    let Some(current) = captures.get(&record.handle) else {
        return;
    }; // 生命周期空窗保守跳过。
    if current.uniform == source.extension.uniform && current.coverage == coverage {
        return;
    } // 稳定材质不重复触发GPU上传。
    if let Some(mut capture) = captures.get_mut(&record.handle) {
        capture.uniform = source.extension.uniform;
        capture.coverage = coverage;
    } // 固定参数同帧原子更新。
} // 完成同源波形同步。

fn release_material(
    id: AssetId<DynamicWaterMaterial>,
    cache: &mut RainMaskCache,
    captures: &mut Assets<RainHeightMaterial>,
) {
    // 缓存只回收自己创建的高度材质。
    let Some(record) = cache.materials.get_mut(&id) else {
        return;
    }; // 重复移除事件幂等。
    record.users = record.users.saturating_sub(1); // 每个代理归还一个引用。
    if record.users == 0 {
        let handle = record.handle.clone();
        cache.materials.remove(&id);
        captures.remove(handle.id());
    } // 最后一个引用释放完整捕获材质。
} // 完成精确材质回收。

fn remove_source(
    entity: Entity,
    commands: &mut Commands,
    cache: &mut RainMaskCache,
    captures: &mut Assets<RainHeightMaterial>,
) {
    // 源水恢复普通材质或despawn时定向清理。
    cache.queued.remove(&entity); // 队列中旧占位稍后按固定预算跳过。
    if let Some(record) = cache.proxies.remove(&entity) {
        commands.entity(record.entity).try_despawn();
        release_material(record.source_material, cache, captures);
    } // 子实体可能已经随父despawn，重复清理安全。
} // 完成源生命周期移除。

fn remove_clipmap_source(
    entity: Entity,
    commands: &mut Commands,
    cache: &mut RainMaskCache,
    captures: &mut Assets<ClipmapRainHeightMaterial>,
) {
    // 每层捕获材质包含独立水位与R8，源消失时必须一起释放。
    cache.clipmap_queued.remove(&entity);
    if let Some(record) = cache.clipmap_proxies.remove(&entity) {
        commands.entity(record.entity).try_despawn();
        captures.remove(record.material.id());
    }
} // 完成Clipmap源生命周期移除。

#[allow(clippy::too_many_arguments, clippy::type_complexity)] // 变化、完整源查询和三种移除事件职责独立。
pub(super) fn sync_proxies(
    mut commands: Commands, // 只安装轻量代理，不生成几何。
    changed: Query<
        Entity,
        (
            Or<(With<DynamicWaterSurface>, With<WaterRenderChunk>)>,
            Or<(
                Added<DynamicWaterSurface>,
                Added<WaterRenderChunk>,
                Changed<Mesh3d>,
                Changed<Aabb>,
                Changed<MeshMaterial3d<DynamicWaterMaterial>>,
            )>,
        ),
    >, // 只收集真实身份、已完成边界或句柄变化。
    sources: Query<
        (
            &Mesh3d,
            &MeshMaterial3d<DynamicWaterMaterial>,
            &GlobalTransform,
            &Aabb,
        ),
        Or<(With<DynamicWaterSurface>, With<WaterRenderChunk>)>,
    >, // 无已有边界不扫描Mesh或伪造范围，等源完整边界出现后再接管。
    mut removed_render_chunk: RemovedComponents<WaterRenderChunk>,
    mut removed_water: RemovedComponents<DynamicWaterSurface>, // 整网格取消赋水会移除水组件。
    mut removed_material: RemovedComponents<MeshMaterial3d<DynamicWaterMaterial>>, // 源改回普通材质时不能保留捕获。
    mut removed_mesh: RemovedComponents<Mesh3d>, // 源撤销几何时不能保留旧代理。
    mut removed_bounds: RemovedComponents<Aabb>, // 边界撤销也撤销旧代理，避免无限裁剪例外。
    mut events: feathrise_scene_core::residency::ResidentAssetEventReader<DynamicWaterMaterial>, // 作者材质变化定向更新缓存，不复制资产表。
    materials: Res<Assets<DynamicWaterMaterial>>, // 仅读取固定uniform和基础透明度。
    mut captures: ResMut<Assets<RainHeightMaterial>>, // 新引用时创建一次捕获材质。
    mut cache: ResMut<RainMaskCache>,             // 原实体和原材质直接定位缓存。
) {
    // 在本帧原水参数更新后执行。
    for entity in removed_water
        .read()
        .chain(removed_render_chunk.read())
        .chain(removed_material.read())
        .chain(removed_mesh.read())
        .chain(removed_bounds.read())
    {
        // 同一源重复移除安全去重。
        if sources.get(entity).is_err() {
            remove_source(entity, &mut commands, &mut cache, &mut captures);
        } // 同帧移除再插入仍由最新源接管。
    } // 完成移除事件消费。
    for entity in &changed {
        if cache.queued.insert(entity) {
            cache.pending.push_back(entity);
        }
    } // 合并同源多次变化，不重复安装。
    let budget = cache.pending.len().min(64); // 固定每帧64个轻量安装上限，不同步安装完整大地图。
    for _ in 0..budget {
        // 只处理本帧固定预算，缺资产条目排到下一帧。
        let Some(entity) = cache.pending.pop_front() else {
            break;
        }; // 队列空时立即退出。
        if !cache.queued.remove(&entity) {
            continue;
        } // 旧移除占位不会重新生成代理。
        let Ok((mesh, material, global, source_bounds)) = sources.get(entity) else {
            remove_source(entity, &mut commands, &mut cache, &mut captures);
            continue;
        }; // 迟到或无真实边界源不制造猜测代理。
        let Some(source) = materials.get(&material.0) else {
            cache.queued.insert(entity);
            cache.pending.push_back(entity);
            continue;
        }; // 等完整材质，不猜测默认波形。
        let id = material.0.id(); // 当前源材质身份。
        let mut bounds = *source_bounds; // 只复制源已完成的24字节边界，不读取Mesh顶点。
        bounds.half_extents += Vec3A::splat(164.0); // 作者4波各≤10m、陡峭度≤1、样条倍率≤3，加玩家波保守上界；改波无需每帧重排代理。
        if cache
            .proxies
            .get(&entity)
            .is_some_and(|record| record.source_material != id)
        {
            remove_source(entity, &mut commands, &mut cache, &mut captures);
        } // 换材质归还旧引用后重新授信。
        let is_new = !cache.proxies.contains_key(&entity); // 首次安装才增加共享引用。
        let height_material = if let Some(record) = cache.materials.get_mut(&id) {
            if is_new {
                record.users += 1;
            }
            record.handle.clone()
        } else {
            // 已有材质只克隆强句柄。
            let handle = captures.add(RainHeightMaterial {
                uniform: source.extension.uniform,
                coverage: Vec4::new(source.base.base_color.alpha(), 0.0, 0.0, 0.0),
            }); // 固定640字节波形加基础Alpha，与源同帧一致。
            cache.materials.insert(
                id,
                MaterialRecord {
                    handle: handle.clone(),
                    users: 1,
                },
            );
            handle // 记录单一共享材质。
        }; // 完成捕获材质接管。
        if let Some(record) = cache.proxies.get(&entity) {
            // Mesh热更新或句柄更换时保留已有代理实体。
            commands.entity(record.entity).insert((
                mesh.clone(),
                MeshMaterial3d(height_material),
                bounds,
            )); // 复用真实Mesh句柄和完整保守边界，绝不拷贝位置/索引。
        } else {
            // 首次安装一个仅捕获可见的子实体。
            let proxy = commands
                .spawn((
                    Name::new("RainWaterHeightProxy"), // 调试可定位但没有普通玩法名字语义。
                    RainMaskProxy,                     // 排除普通水体逻辑。
                    mesh.clone(),                      // 与正式水体完全同一个Mesh资产。
                    MeshMaterial3d(height_material),   // 同源波形的数值高度材质。
                    ChildOf(entity), // 原源隐藏、移动、旋转和despawn均沿真实层级传播。
                    Transform::IDENTITY, // 子代理不引入任何空间偏移。
                    *global,         // TransformPropagate已完成，首帧显式继承真实世界矩阵。
                    Visibility::Inherited, // 完整继承源水和所有祖先显隐，不用ViewVisibility的其他视图结果。
                    RenderLayers::layer(RAIN_WATER_MASK_LAYER), // 只进入高度图，不出现在主图、小地图或普通预览。
                    NotShadowCaster,                            // 不投射第二份阴影。
                    NotShadowReceiver,                          // 高度编码不接收阴影。
                    bounds, // 捕获相机用保守边界剔除远处整张水面，不禁用CPU视锥裁剪。
                ))
                .id(); // 完成唯一捕获代理。
            cache.proxies.insert(
                entity,
                ProxyRecord {
                    entity: proxy,
                    source_material: id,
                },
            ); // 直接索引后续精确清理。
        } // 完成新建或复用代理。
        sync_source_uniform(id, source, &cache, &mut captures); // 材质数值同时更新时本帧就同步。
    } // 完成固定预算安装。
    for event in events.read() {
        // 对没有CPU玩家互动循环的整网格材质也处理作者数值更新。
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = event {
            if let Some(source) = materials.get(*id) {
                sync_source_uniform(*id, source, &cache, &mut captures);
            }
        } // 稳定无资产事件时零额外材质遍历。
        if let AssetEvent::Removed { id } = event {
            if let Some(record) = cache.materials.get(id) {
                if let Some(mut capture) = captures.get_mut(&record.handle) {
                    capture.coverage = Vec4::ZERO;
                }
            }
        } // 原材质不可渲染时代理同样不写覆盖，重载Added后恢复。
    } // 完成源材质事件同步。
} // 完成实际水面捕获代理维护。

/// Clipmap静水捕获直接复用正式环带Mesh；专用材质在GPU按R8、真实三角水深和缺页状态裁切。
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn sync_clipmap_proxies(
    mut commands: Commands,
    changed: Query<
        Entity,
        (
            Or<(With<ClipmapWaterSurface>, With<WaterRenderChunk>)>,
            Or<(
                Added<ClipmapWaterSurface>,
                Added<WaterRenderChunk>,
                Changed<Mesh3d>,
                Changed<MeshMaterial3d<ClipmapWaterMaterial>>,
            )>,
        ),
    >,
    sources: Query<
        (
            &Mesh3d,
            &MeshMaterial3d<ClipmapWaterMaterial>,
            &GlobalTransform,
        ),
        Or<(With<ClipmapWaterSurface>, With<WaterRenderChunk>)>,
    >,
    mut removed_render_chunk: RemovedComponents<WaterRenderChunk>,
    mut removed_water: RemovedComponents<ClipmapWaterSurface>,
    mut removed_material: RemovedComponents<MeshMaterial3d<ClipmapWaterMaterial>>,
    mut removed_mesh: RemovedComponents<Mesh3d>,
    mut events: feathrise_scene_core::residency::ResidentAssetEventReader<ClipmapWaterMaterial>,
    materials: Res<Assets<ClipmapWaterMaterial>>,
    mut captures: ResMut<Assets<ClipmapRainHeightMaterial>>,
    mut cache: ResMut<RainMaskCache>,
) {
    // 先撤销失效身份，队列中的迟到占位随后会被queued集合过滤。
    for entity in removed_water
        .read()
        .chain(removed_render_chunk.read())
        .chain(removed_material.read())
        .chain(removed_mesh.read())
    {
        if sources.get(entity).is_err() {
            remove_clipmap_source(entity, &mut commands, &mut cache, &mut captures);
        }
    }
    for entity in &changed {
        if cache.clipmap_queued.insert(entity) {
            cache.clipmap_pending.push_back(entity);
        }
    }

    // 不限制水层及子块总量，代理按固定切片逐帧安装。
    let budget = cache.clipmap_pending.len().min(8);
    for _ in 0..budget {
        let Some(entity) = cache.clipmap_pending.pop_front() else {
            break;
        };
        if !cache.clipmap_queued.remove(&entity) {
            continue;
        }
        let Ok((mesh, material, global)) = sources.get(entity) else {
            remove_clipmap_source(entity, &mut commands, &mut cache, &mut captures);
            continue;
        };
        let Some(source) = materials.get(&material.0) else {
            cache.clipmap_queued.insert(entity);
            cache.clipmap_pending.push_back(entity);
            continue;
        }; // 不用默认高度或遮罩替代尚未完整采用的材质。
        let source_id = material.0.id();
        if let Some(record) = cache.clipmap_proxies.get_mut(&entity) {
            let capture = if let Some(mut capture) = captures.get_mut(&record.material) {
                capture.sync_source(source);
                record.material.clone()
            } else {
                let replacement = captures.add(ClipmapRainHeightMaterial::from_source(source));
                record.material = replacement.clone();
                replacement
            };
            record.source_material = source_id;
            commands
                .entity(record.entity)
                .insert((mesh.clone(), MeshMaterial3d(capture)));
        } else {
            let capture = captures.add(ClipmapRainHeightMaterial::from_source(source));
            let proxy = commands
                .spawn((
                    Name::new("ClipmapRainWaterHeightProxy"),
                    RainMaskProxy,
                    mesh.clone(),
                    MeshMaterial3d(capture.clone()),
                    ChildOf(entity),
                    Transform::IDENTITY,
                    *global,
                    Visibility::Inherited,
                    RenderLayers::layer(RAIN_WATER_MASK_LAYER),
                    NoFrustumCulling,
                    NotShadowCaster,
                    NotShadowReceiver,
                ))
                .id();
            cache.clipmap_proxies.insert(
                entity,
                ClipmapProxyRecord {
                    entity: proxy,
                    source_material: source_id,
                    material: capture,
                },
            );
        }
    }

    // 高度页、玩家波或作者水位变化只改小型材质绑定，不重建共享环带代理。
    for event in events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                let Some(source) = materials.get(*id) else {
                    continue;
                };
                for record in cache
                    .clipmap_proxies
                    .values()
                    .filter(|record| record.source_material == *id)
                {
                    if let Some(mut capture) = captures.get_mut(&record.material) {
                        capture.sync_source(source);
                    }
                }
            }
            AssetEvent::Removed { id } => {
                for record in cache
                    .clipmap_proxies
                    .values()
                    .filter(|record| record.source_material == *id)
                {
                    if let Some(mut capture) = captures.get_mut(&record.material) {
                        capture.disable();
                    }
                }
            }
            _ => {}
        }
    }
} // 完成Clipmap静水的局部雨水高度捕获代理。
