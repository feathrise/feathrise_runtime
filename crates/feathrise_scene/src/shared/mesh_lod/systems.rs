//! 来源变化有界登记、共享减面单飞、完整上传采用与屏幕误差选择。
use super::{
    capture::{Capture, mesh_bytes},
    components::*,
    ownership::owner,
    resources::*,
    types::*,
};
use bevy::{
    asset::AssetEvent,
    camera::primitives::Aabb,
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
};
use feathrise_scene_content::MeshDisplayLodSettings;

pub(super) fn mesh_inserted(
    event: On<Insert<Mesh3d>>,
    mut runtime: Option<ResMut<MeshLodRuntime>>,
) {
    if let Some(runtime) = runtime.as_mut() {
        runtime.enqueue(event.entity);
    }
}

pub(super) fn asset_events(
    mut events: MessageReader<AssetEvent<Mesh>>,
    mut runtime: ResMut<MeshLodRuntime>,
) {
    if events.len() > 128 {
        runtime.event_epoch = runtime.event_epoch.wrapping_add(1);
        events.clear(); // 溢出用一个epoch保守换代；不能漏热更，也不在本帧扫描所有来源。
        return;
    }
    for event in events.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } | AssetEvent::Removed { id } =
            event
        {
            if runtime.revisions.contains_key(id) {
                runtime.revision_nonce = runtime.revision_nonce.wrapping_add(1);
                let nonce = runtime.revision_nonce;
                runtime.revisions.insert(*id, nonce);
            }
        }
    }
}

fn supported_display(world: &World, entity: Entity) -> bool {
    world
        .get::<bevy::mesh::skinning::SkinnedMesh>(entity)
        .is_none()
        && world
            .get::<bevy::mesh::morph::MorphWeights>(entity)
            .is_none()
        && (world
            .get::<MeshMaterial3d<StandardMaterial>>(entity)
            .is_some()
            || world
                .get::<crate::DynamicSceneTerrainPrimitive>(entity)
                .is_some())
}

pub(super) fn register(world: &mut World) {
    world.resource_scope(|world, mut runtime: Mut<MeshLodRuntime>| {
        for _ in 0..TARGET_BUDGET {
            let Some((entity, next_child)) = runtime.discovery.pop_front() else {
                break;
            };
            if next_child == 0 {
                runtime.enqueue(entity);
            }
            if let Some(child) = world
                .get::<Children>(entity)
                .and_then(|children| children.get(next_child))
                .copied()
            {
                runtime.discovery.push_back((entity, next_child + 1));
                runtime.discovery.push_back((child, 0)); // 祖先后补来源身份时有界重访后代，不能只检查没有Mesh的祖先。
            }
        }
        for _ in 0..TARGET_BUDGET {
            let Some(entity) = runtime.pending.pop_front() else {
                break;
            };
            runtime.queued.remove(&entity);
            let Some(owner) = owner(world, entity) else {
                continue;
            };
            let Some(current) = world.get::<Mesh3d>(entity).map(|mesh| mesh.0.clone()) else {
                continue;
            };
            // 使用高度采样材质的GLB仍是静态显示；水、位移Clipmap及植被由所有权门排除。
            if !supported_display(world, entity) {
                continue;
            }
            let original = if let Some(target) = runtime.targets.get(&entity) {
                let internal = current == target.original
                    || runtime.cache.get(&target.key).is_some_and(|cache| {
                        cache.levels.iter().any(|level| level.mesh == current)
                    });
                if internal && target.owner == owner.root {
                    continue;
                } // 自身LOD换柄不能重新生成或改变来源证据。
                if internal {
                    target.original.clone()
                } else {
                    current.clone()
                }
            } else {
                current.clone()
            };
            if let Some(old) = runtime.targets.remove(&entity) {
                if old.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&old.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.coarse.remove(&entity);
                runtime.release_reference(old.key);
            } else {
                runtime.cursor.push_back(entity);
            }
            if current != original {
                world.entity_mut(entity).insert(Mesh3d(original.clone()));
            }
            runtime.watch(original.id()); // 只跟踪原始来源，派生资产事件不制造新版本或循环。
            let key = runtime.key(original.id());
            runtime.retain_reference(key);
            world.entity_mut(entity).insert(StaticMeshLodOriginal {
                mesh: original.clone(),
            });
            runtime.targets.insert(
                entity,
                VisualTarget {
                    owner: owner.root,
                    original,
                    key,
                    applied: 0,
                },
            );
        }
        world
            .resource_mut::<MeshLodRuntimeStatus>()
            .registered_meshes = runtime.targets.len();
    });
}

fn failure(
    runtime: &mut MeshLodRuntime,
    status: &mut MeshLodRuntimeStatus,
    key: SourceKey,
    error: String,
) {
    runtime.requested.remove(&key);
    runtime.cache.insert(
        key,
        VisualCache {
            levels: Vec::new(),
            bytes: 0,
            pins: 0,
            complete: false,
            failed: true,
            retry_credit: None,
        },
    );
    runtime.queue_cache(key);
    status.detail = error.clone();
    warn!(source = ?key.id, "静态网格LOD保留原始质量：{error}"); // 同来源失败缓存，稳定帧不重启或刷屏。
}

pub(super) fn drive(
    mut runtime: ResMut<MeshLodRuntime>,
    mut status: ResMut<MeshLodRuntimeStatus>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if runtime.release.as_mut().and_then(check_ready).is_some() {
        runtime.release = None;
        let credit = std::mem::take(&mut runtime.release_credit);
        runtime.cache_bytes = runtime.cache_bytes.saturating_sub(credit);
        if credit != 0 {
            runtime.credit_revision = runtime.credit_revision.wrapping_add(1);
        }
    }
    for _ in 0..TARGET_BUDGET {
        let Some(id) = runtime.revision_cursor.pop_front() else {
            break;
        };
        if !runtime.asset_references.contains_key(&id) {
            runtime.revisions.remove(&id);
        } else {
            runtime.revision_cursor.push_back(id);
        }
    } // 来源退役后有界删版本票据；再次登记分配全新nonce，旧完成值不能复活。
    for _ in 0..TARGET_BUDGET {
        let Some(key) = runtime.pop_cache() else {
            break;
        };
        let in_flight = runtime
            .upload
            .as_ref()
            .is_some_and(|upload| upload.key == key);
        if !runtime.references.contains_key(&key)
            && !in_flight
            && runtime.cache.get(&key).is_some_and(|cache| cache.pins == 0)
        {
            if let Some(cache) = runtime.cache.remove(&key) {
                runtime.retired.extend(
                    cache
                        .levels
                        .into_iter()
                        .map(|level| (level.mesh, level.bytes)),
                );
            }
        } else if runtime.cache.contains_key(&key) {
            runtime.queue_cache(key);
        }
    } // 孤儿缓存和失败票据有界清理，场景卸载不残留0字节tombstone。
    if runtime.release.is_none()
        && let Some((handle, bytes)) = runtime.retired.pop_front()
    {
        let retired = meshes.remove(handle.id());
        runtime.release_credit = bytes; // Assets移除只转移数组所有权，后台析构完成前不能重用字节信用。
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(retired);
        }));
    } // 每帧最多移除一个派生资产，完整CPU数组在后台析构。
    if runtime.release.is_none()
        && let Some((key, task)) = runtime.task.as_mut()
        && let Some(result) = check_ready(task)
    {
        let key = *key;
        runtime.task = None;
        if !runtime.current(key)
            || !meshes.contains(key.id)
            || !runtime.references.contains_key(&key)
        {
            runtime.requested.remove(&key);
            runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(result);
            }));
        } else {
            match result {
                Ok(levels) => {
                    runtime.cache.insert(
                        key,
                        VisualCache {
                            levels: Vec::new(),
                            bytes: 0,
                            pins: 0,
                            complete: false,
                            failed: false,
                            retry_credit: None,
                        },
                    );
                    runtime.queue_cache(key);
                    runtime.upload = Some(Upload {
                        key,
                        pending: levels.into(),
                    });
                    status.detail = "静态网格LOD后台完成，正在有界上传".into();
                }
                Err(error) => failure(&mut runtime, &mut status, key, error),
            }
        }
    }
    if let Some(mut upload) = runtime.upload.take() {
        if !runtime.current(upload.key)
            || !meshes.contains(upload.key.id)
            || !runtime.references.contains_key(&upload.key)
        {
            if runtime.release.is_some() {
                runtime.upload = Some(upload);
                return;
            }
            runtime.requested.remove(&upload.key);
            if let Some(cache) = runtime.cache.remove(&upload.key) {
                runtime.retired.extend(
                    cache
                        .levels
                        .into_iter()
                        .map(|level| (level.mesh, level.bytes)),
                );
            }
            runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(upload);
            }));
            return;
        }
        if let Some(level) = upload.pending.front() {
            let bytes = mesh_bytes(&level.mesh);
            if bytes > UPLOAD_BUDGET {
                if runtime.release.is_some() {
                    runtime.upload = Some(upload);
                    return;
                }
                let key = upload.key;
                if let Some(cache) = runtime.cache.remove(&key) {
                    runtime.retired.extend(
                        cache
                            .levels
                            .into_iter()
                            .map(|level| (level.mesh, level.bytes)),
                    );
                }
                runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                    drop(upload);
                }));
                failure(
                    &mut runtime,
                    &mut status,
                    key,
                    "派生单网格超过16MiB上传信用，保留完整作者源与原始显示".into(),
                );
                return;
            }
            if runtime.cache_bytes.saturating_add(bytes) > CACHE_BYTES {
                // 冷缓存按固定条目轮询；当前在显示的句柄有pin，不能回收正在使用的数组。
                for _ in 0..TARGET_BUDGET {
                    let Some(key) = runtime.pop_cache() else {
                        break;
                    };
                    if key != upload.key
                        && runtime
                            .cache
                            .get(&key)
                            .is_some_and(|cache| cache.pins == 0 && cache.complete && !cache.failed)
                    {
                        if let Some(cache) = runtime.cache.remove(&key) {
                            runtime.retired.extend(
                                cache
                                    .levels
                                    .into_iter()
                                    .map(|level| (level.mesh, level.bytes)),
                            );
                        }
                        break;
                    } else {
                        runtime.queue_cache(key);
                    }
                }
                if runtime.cache_bytes.saturating_add(bytes) <= CACHE_BYTES
                    || runtime.release.is_some()
                    || !runtime.retired.is_empty()
                {
                    runtime.upload = Some(upload);
                    status.detail = "等待静态网格LOD缓存退役信用；期间保留原始显示".into();
                    return;
                }
                // 没有可退役冷缓存时放弃本次派生，释放唯一上传槽让后续小来源继续工作。
                let key = upload.key;
                if let Some(cache) = runtime.cache.remove(&key) {
                    runtime.retired.extend(
                        cache
                            .levels
                            .into_iter()
                            .map(|level| (level.mesh, level.bytes)),
                    );
                }
                runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                    drop(upload);
                }));
                failure(
                    &mut runtime,
                    &mut status,
                    key,
                    "静态网格LOD缓存信用暂不足，本来源保留原始质量".into(),
                );
                let credit = runtime.credit_revision;
                runtime.cache.get_mut(&key).unwrap().retry_credit = Some(credit);
                return;
            }
            let level = upload.pending.pop_front().unwrap();
            let handle = meshes.add(level.mesh);
            let cache = runtime.cache.get_mut(&upload.key).unwrap();
            cache.bytes = cache.bytes.saturating_add(bytes);
            cache.levels.push(VisualLevel {
                mesh: handle,
                error: level.max_error,
                bytes,
            });
            runtime.cache_bytes += bytes; // 每帧登记一层、最多16MiB，半套派生不能成为显示权威。
        }
        if upload.pending.is_empty() {
            runtime.cache.get_mut(&upload.key).unwrap().complete = true;
            runtime.requested.remove(&upload.key);
            status.prepared_sources += 1;
            status.detail = "静态网格LOD已完整采用；原始作者源与碰撞继续保留".into();
        } else {
            runtime.upload = Some(upload);
        }
        return;
    }
    if runtime.task.is_some() || runtime.release.is_some() {
        return;
    }
    if runtime
        .capture
        .as_ref()
        .is_some_and(|capture| !runtime.current(capture.key))
    {
        let capture = runtime.capture.take().unwrap();
        runtime.requested.remove(&capture.key);
        runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(capture);
        }));
        return;
    }
    if runtime.capture.is_none() {
        for _ in 0..TARGET_BUDGET {
            let Some(key) = runtime.requests.pop_front() else {
                break;
            };
            if !runtime.current(key) || !runtime.references.contains_key(&key) {
                runtime.requested.remove(&key);
                continue;
            }
            let Some(source) = meshes.get(key.id) else {
                runtime.requests.push_back(key);
                break;
            };
            match Capture::new(key, source) {
                Ok(capture) => {
                    runtime.capture = Some(capture);
                    break;
                }
                Err(error) => failure(&mut runtime, &mut status, key, error),
            }
        }
    }
    if let Some(capture) = runtime.capture.as_mut() {
        let key = capture.key;
        let result = meshes
            .get(key.id)
            .ok_or_else(|| "原始网格在捕获期间失效".to_owned())
            .and_then(|source| capture.step(source, COPY_BUDGET));
        match result {
            Ok(true) => {
                let capture = runtime.capture.take().unwrap();
                runtime.task = Some((
                    key,
                    AsyncComputeTaskPool::get().spawn(async move {
                        feathrise_terrain::build_static_mesh_lods(capture.finish()?)
                    }),
                ));
            }
            Ok(false) => status.detail = "正在分帧冻结完整静态网格属性".into(),
            Err(error) => {
                let capture = runtime.capture.take();
                runtime.release = Some(AsyncComputeTaskPool::get().spawn(async move {
                    drop(capture);
                }));
                failure(&mut runtime, &mut status, key, error);
            }
        }
    }
}

fn views(world: &mut World) -> Vec<View> {
    let mut result = Vec::new();
    let mut cameras = world.query_filtered::<(
        &Camera,
        &Projection,
        &GlobalTransform,
        Option<&bevy::camera::visibility::RenderLayers>,
    ), With<Camera3d>>();
    for (index, (camera, projection, global, layers)) in cameras.iter(world).take(17).enumerate() {
        if index == 16 {
            return Vec::new();
        } // 多视图超出本帧固定规格时全保原，不能忽略第17个实际镜头的误差。
        if !camera.is_active {
            continue;
        }
        let Some(size) = camera.physical_viewport_size().filter(|size| size.y > 0) else {
            continue;
        };
        let inverse = global.to_matrix().inverse();
        if !inverse.is_finite() {
            continue;
        }
        let (focal_pixels, orthographic) = match projection {
            Projection::Perspective(projection) => {
                (size.y as f32 / (2.0 * (projection.fov * 0.5).tan()), false)
            }
            Projection::Orthographic(projection) => {
                (size.y as f32 / projection.area.height(), true)
            }
            _ => continue,
        };
        if focal_pixels.is_finite() && focal_pixels > 0.0 {
            result.push(View {
                inverse,
                focal_pixels,
                orthographic,
                layers: layers.cloned().unwrap_or_default(),
            });
        }
    }
    result
}

pub(super) fn select(world: &mut World) {
    let views = views(world);
    let authoring = world.resource::<MeshLodAuthoringState>().active;
    let global = world
        .get_resource::<feathrise_settings::RenderSettings>()
        .map(|settings| settings.mesh_lod.sanitized())
        .filter(|settings| settings.display_enabled)
        .map(|settings| MeshDisplayLodSettings {
            max_error_pixels: settings.max_error_pixels,
            hysteresis_ratio: settings.hysteresis_ratio,
        });
    world.resource_scope(|world, mut runtime: Mut<MeshLodRuntime>| {
        for _ in 0..TARGET_BUDGET.min(runtime.cursor.len()) {
            let entity = runtime.cursor.pop_front().unwrap();
            let Some(mut target) = runtime.targets.remove(&entity) else {
                continue;
            };
            let Some(current_mesh) = world.get::<Mesh3d>(entity).map(|mesh| mesh.0.clone()) else {
                if target.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&target.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.coarse.remove(&entity);
                runtime.release_reference(target.key);
                if let Ok(mut entity) = world.get_entity_mut(entity) {
                    entity.remove::<StaticMeshLodOriginal>();
                }
                continue;
            };
            let source_owner = owner(world, entity)
                .filter(|owner| owner.root == target.owner && supported_display(world, entity));
            let current_is_ours = current_mesh == target.original
                || runtime.cache.get(&target.key).is_some_and(|cache| {
                    cache.levels.iter().any(|level| level.mesh == current_mesh)
                });
            if source_owner.is_none() {
                if target.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&target.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.coarse.remove(&entity);
                runtime.release_reference(target.key);
                if current_is_ours {
                    world.entity_mut(entity).insert(Mesh3d(target.original));
                }
                world.entity_mut(entity).remove::<StaticMeshLodOriginal>();
                continue; // 非来源或层级切换场景后撤销登记，不永久轮转或反复请求旧作者源。
            }
            if !current_is_ours {
                if target.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&target.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.coarse.remove(&entity);
                runtime.release_reference(target.key);
                world.entity_mut(entity).remove::<StaticMeshLodOriginal>();
                runtime.enqueue(entity);
                continue; // 外部换源立即退旧票据，下次登记只读新原始句柄。
            }
            let key = runtime.key(target.original.id());
            if key != target.key {
                if target.applied != 0
                    && let Some(cache) = runtime.cache.get_mut(&target.key)
                {
                    cache.pins = cache.pins.saturating_sub(1);
                }
                runtime.release_reference(target.key);
                runtime.coarse.remove(&entity);
                target.key = key;
                target.applied = 0;
                runtime.retain_reference(key);
                world
                    .entity_mut(entity)
                    .insert(Mesh3d(target.original.clone()));
            }
            let policy = source_owner
                .as_ref()
                .filter(|owner| owner.active)
                .and_then(|owner| owner.policy.resolve_display(global))
                .filter(|_| !authoring && !views.is_empty());
            if policy.is_some() {
                runtime.request(key);
            }
            let mut selected = 0;
            if let (Some(policy), Some(cache), Some(pose), Some(bounds)) = (
                policy,
                runtime.cache.get(&key),
                world.get::<GlobalTransform>(entity),
                world.get::<Aabb>(entity),
            ) && cache.complete
                && !cache.failed
            {
                let radius = bounds.center.length() + bounds.half_extents.length();
                let layers = world
                    .get::<bevy::camera::visibility::RenderLayers>(entity)
                    .cloned()
                    .unwrap_or_default();
                for (index, level) in cache.levels.iter().enumerate() {
                    let mut relevant = views
                        .iter()
                        .filter(|view| view.layers.intersects(&layers))
                        .peekable();
                    if relevant.peek().is_none() {
                        continue;
                    } // 资产/NPC离屏相机不能决定场景网格精度。
                    let pixels = relevant
                        .map(|view| view.error_pixels(pose, radius, level.error))
                        .fold(0.0f32, f32::max);
                    let limit = policy.max_error_pixels
                        * if index + 1 == target.applied {
                            1.0 + policy.hysteresis_ratio
                        } else {
                            1.0 - policy.hysteresis_ratio
                        };
                    if pixels <= limit {
                        selected = index + 1;
                    }
                }
            }
            if selected != target.applied {
                if let Some(cache) = runtime.cache.get_mut(&key) {
                    if target.applied != 0 {
                        cache.pins = cache.pins.saturating_sub(1);
                    }
                    if selected != 0 {
                        cache.pins += 1;
                    }
                }
                if target.applied != 0 && selected == 0 {
                    runtime.credit_revision = runtime.credit_revision.wrapping_add(1);
                }
                let handle = if selected == 0 {
                    target.original.clone()
                } else {
                    runtime.cache[&key].levels[selected - 1].mesh.clone()
                };
                world.entity_mut(entity).insert(Mesh3d(handle));
                target.applied = selected;
                if selected == 0 {
                    runtime.coarse.remove(&entity);
                } else {
                    runtime.coarse.insert(entity);
                }
            }
            runtime.targets.insert(entity, target);
            runtime.cursor.push_back(entity);
        }
    });
}

pub(super) fn authoring_ready(world: &mut World) {
    let ready = world.resource::<MeshLodAuthoringState>().active
        && world.resource::<MeshLodRuntime>().coarse.is_empty()
        && super::collision::coarse_empty(world);
    world.resource_mut::<MeshLodRuntimeStatus>().authoring_ready = ready;
}
