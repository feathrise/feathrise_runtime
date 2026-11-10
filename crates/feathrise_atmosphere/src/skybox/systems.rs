//! 作者动态天空/天空盒请求处理、Image事件观察和原子相机采用。

use bevy::asset::{AssetEvent, AssetLoadFailedEvent, LoadState, RenderAssetUsages}; // 热更新成功、失败、当前状态与渲染用途按Image身份仲裁。
use bevy::core_pipeline::Skybox; // 天空盒模式原子安装主 Cube，动态模式明确归还可见背景所有权。
use bevy::ecs::message::{MessageCursor, Messages}; // 固定预算读取全局Image消息并检测历史淘汰。
use bevy::prelude::*; // Commands、Assets、AssetServer与ECS查询。
use bevy::render::render_resource::{
    TextureDimension, TextureSampleType, TextureUsages, TextureViewDimension,
}; // 解码后严格验证立方体视图、可过滤采样格式和GPU用途。

use crate::{WorldSkyProfile, time::WorldEnvironmentMapProfile}; // 可见 Cube 与 PBR 环境照明分别保留昼夜基准。

use super::components::{
    AuthoredSkyMode, AuthoredSkyboxPhase, AuthoredSkyboxRequest, AuthoredSkyboxRuntime,
    AuthoredSkyboxStatus, TrackedAuthoredSkybox, TrackedSkyboxAssetKind,
}; // 同一领域内共享请求身份和回退快照。
use super::environment::{
    AuthoredEnvironmentMapAssets, MAX_AUTHORED_DIFFUSE_EDGE, invalidate_scaled_diffuse_cubes,
    resolve_environment_maps,
}; // 只对有界低分辨率辐照Cube编码独立补光比例，GGX镜面保持原资产。

/// 单帧最多消费的全局 Image 成功/修改消息；积压保留到后续帧，淘汰则保守复验两台相机各自最多三张依赖图。
const SKYBOX_IMAGE_EVENT_BUDGET: usize = 128;
/// 加载失败通常极少，独立小预算避免异常资产风暴扩大当前系统单帧工作量。
const SKYBOX_FAILURE_EVENT_BUDGET: usize = 32;

/// 当前模式两件套或三件套的权威检查结果；只有 Ready 允许进入原子采用。
enum TrackedAssetInspection {
    Ready,           // 当前模式要求的全部 Image 都已解码并通过各自格式门。
    Loading(String), // 至少一张仍在异步加载或尚未进入Assets<Image>。
    Failed(String),  // AssetServer已确认至少一张加载失败。
    Invalid(String), // 文件已解码但Cube、用途或Mip契约不正确。
}

/// 每次事件只检查当前最多两台相机的两至三张明确句柄，不扫描全资产表。
fn inspect_tracked_assets(
    asset_server: &AssetServer, // LoadState是成功、等待与失败的当前权威来源。
    images: &Assets<Image>,     // 已解码Image提供Cube与Mip元数据。
    tracked: &TrackedAuthoredSkybox, // 同一修订冻结的模式、路径和强句柄集合。
) -> TrackedAssetInspection {
    for (path, handle, kind) in tracked.assets() {
        let label = match kind {
            TrackedSkyboxAssetKind::Visible => "可见天空", // 主 Cube 只在天空盒模式检查。
            TrackedSkyboxAssetKind::Diffuse => "漫反射辐照", // 两种模式都检查单级低频 Cube。
            TrackedSkyboxAssetKind::Specular => "GGX镜面", // 两种模式都检查完整 Mip 链。
        };
        match asset_server.load_state(handle.id()) {
            LoadState::Failed(error) => {
                return TrackedAssetInspection::Failed(format!(
                    "{label}加载失败 assets/{path}：{error}"
                )); // 任一依赖失败都撤销整套环境，禁止半套采用。
            }
            LoadState::NotLoaded | LoadState::Loading => {
                return TrackedAssetInspection::Loading(format!("正在加载{label} assets/{path}")); // 主线程只发布阶段并立即让出。
            }
            LoadState::Loaded => {
                let Some(image) = images.get(handle.id()) else {
                    return TrackedAssetInspection::Loading(format!(
                        "{label}已解码但主世界Image暂不可用 assets/{path}"
                    )); // 热更交接窗口等待下一帧原子采用。
                };
                let validation = match kind {
                    TrackedSkyboxAssetKind::Visible => validate_visible_cube_image(image), // 可见背景只要求合法可采样 Cube。
                    TrackedSkyboxAssetKind::Diffuse => validate_diffuse_cube_image(image), // 漫反射必须是单级卷积结果。
                    TrackedSkyboxAssetKind::Specular => validate_specular_cube_image(image), // 镜面必须有完整粗糙度Mip链。
                };
                if let Err(detail) = validation {
                    return TrackedAssetInspection::Invalid(format!(
                        "{label}格式无效 assets/{path}：{detail}"
                    )); // 指明实际失败角色，方便作者重新生成缺失派生图。
                }
            }
        }
    }
    TrackedAssetInspection::Ready // 当前模式全部依赖均 Loaded 且格式正确。
}

/// 把相机可见背景恢复到天空运行时首次接管前的状态。
fn restore_original_visible_sky(
    commands: &mut Commands,         // 延迟命令在链式调度边界落地。
    entity: Entity,                  // 只修改当前明确请求相机。
    runtime: &AuthoredSkyboxRuntime, // 原值在首次请求时冻结，后续热换不覆盖。
) {
    let mut entity_commands = commands.entity(entity); // 可见 Cube 与亮度基准共同归还。
    if let Some(skybox) = runtime.original_skybox.clone() {
        entity_commands.insert(skybox); // 外部已有 Cube 完整恢复句柄、亮度和方向。
    } else {
        entity_commands.remove::<Skybox>(); // 外部无可见背景时清除作者天空盒。
    }
    if let Some(profile) = runtime.original_sky_profile {
        entity_commands.insert(profile); // 外部昼夜基准与原 Cube 同步恢复。
    } else {
        entity_commands.remove::<WorldSkyProfile>(); // 作者新增基准不泄漏到动态模式。
    }
}

/// 把相机恢复到首次接管前的环境贴图与昼夜强度基准。
fn restore_original_environment(
    commands: &mut Commands,         // 延迟命令由链式调度在后续系统前落地。
    entity: Entity,                  // 只修改当前明确请求相机。
    runtime: &AuthoredSkyboxRuntime, // 原状态在首次请求时冻结，后续切换场景不覆盖。
) {
    let mut entity_commands = commands.entity(entity); // 同一实体的插入与移除保持一个命令入口。
    if let Some(environment_map) = runtime.original_environment_map.clone() {
        entity_commands.insert(environment_map); // 恢复外部过滤图句柄、强度、旋转和lightmap策略。
    } else {
        entity_commands.remove::<EnvironmentMapLight>(); // 作者绑定的镜面环境必须完整清除。
    }
    if let Some(generated) = runtime.original_generated_environment_map.clone() {
        entity_commands.insert(generated); // 外部原本使用实时过滤时也精确归还其源和参数。
    } else {
        entity_commands.remove::<GeneratedEnvironmentMapLight>(); // 禁止失败后残留作者生成描述重新分配输出图。
    }
    if let Some(profile) = runtime.original_environment_profile {
        entity_commands.insert(profile); // 恢复外部环境反射的昼夜基准。
    } else {
        entity_commands.remove::<WorldEnvironmentMapProfile>(); // 作者新增的反射基准与贴图共同清除。
    }
}

/// 同时归还可见背景与环境 IBL，并清除当前作者活动身份。
fn restore_original_sky(
    commands: &mut Commands,             // 两组组件通过同一帧延迟命令恢复。
    entity: Entity,                      // 当前天空请求相机。
    runtime: &mut AuthoredSkyboxRuntime, // 恢复后同步清空活动身份。
) {
    restore_original_visible_sky(commands, entity, runtime); // 先归还可见背景。
    restore_original_environment(commands, entity, runtime); // 再归还 PBR 环境贴图。
    runtime.active_source = None; // 后续失败不得把已清除结果当作可保留旧环境。
    runtime.active_mode = None; // 当前不再有作者可见来源。
    runtime.active_visible_handle = None; // 动态系统据此安全启用原生大气。
}

/// 判断当前候选失败时是否有一套身份不同的已采用环境可继续显示。
fn has_distinct_active_sky(
    runtime: &AuthoredSkyboxRuntime, // 当前相机作者运行时。
    tracked: &TrackedAuthoredSkybox, // 本次正在加载或失败的候选。
) -> bool {
    runtime.active_source.as_deref().is_some_and(|source| {
        source != tracked.paths.source.as_str() || runtime.active_mode != Some(tracked.mode) // 同路径同模式热更失败不能继续宣称旧句柄有效。
    })
}

/// 请求变化时冻结相机原状态并建立两件套或三件套强句柄；等待期间保留上一有效环境。
pub(crate) fn begin_authored_skybox_requests(
    mut commands: Commands,         // 插入运行时、状态或恢复原组件。
    asset_server: Res<AssetServer>, // 只在请求真实变化时创建/复用Image句柄。
    requests: Query<
        (
            Entity,
            &AuthoredSkyboxRequest,
            Option<&AuthoredSkyboxRuntime>,
            Option<&Skybox>,
            Option<&WorldSkyProfile>,
            Option<&EnvironmentMapLight>,
            Option<&GeneratedEnvironmentMapLight>,
            Option<&WorldEnvironmentMapProfile>,
        ),
        Changed<AuthoredSkyboxRequest>,
    >, // 稳定帧不遍历没有变化的请求相机。
) {
    for (
        entity,
        request,
        existing_runtime,
        current_skybox,
        current_sky_profile,
        current_environment_map,
        current_generated_environment_map,
        current_environment_profile,
    ) in &requests
    {
        let mut runtime = existing_runtime
            .cloned()
            .unwrap_or_else(|| AuthoredSkyboxRuntime {
                original_skybox: current_skybox.cloned(), // 首次请求前的可见 Cube 完整冻结。
                original_sky_profile: current_sky_profile.copied(), // 外部天空亮度基准同步冻结。
                original_environment_map: current_environment_map.cloned(), // 保存外部过滤图完整组件。
                original_generated_environment_map: current_generated_environment_map.cloned(), // 保存外部实时过滤源。
                original_environment_profile: current_environment_profile.copied(), // 保存外部反射昼夜基准。
                active_source: None,         // 首次请求尚无作者环境被完整采用。
                active_mode: None,           // 首次请求尚无作者可见天空来源。
                active_visible_handle: None, // 首次请求尚无作者主 Cube。
                tracked: None,               // 下方验证成功后才登记Image强句柄。
            });
        if let Err(detail) = request.validate() {
            restore_original_sky(&mut commands, entity, &mut runtime); // 确定性字段错误立即回到动态天空安全基线。
            runtime.tracked = None; // 后续资产事件不能复活被拒绝的路径。
            commands.entity(entity).insert((
                runtime, // 保留首次原状态供作者修正后继续使用。
                AuthoredSkyboxStatus {
                    phase: AuthoredSkyboxPhase::Failed, // F12明确显示请求失败而非无限加载。
                    asset_path: request.asset_path.clone(), // 回显实际被拒路径。
                    revision: request.revision,         // 状态绑定本次作者修订。
                    detail,                             // 直接发布受检字段原因。
                    visible_skybox_active: false,       // 字段错误不得关闭动态大气或动态云。
                },
            ));
            continue;
        }
        let Some(paths) = request.asset_paths() else {
            restore_original_sky(&mut commands, entity, &mut runtime); // 清除资源同时归还可见背景和环境贴图。
            runtime.tracked = None; // 释放旧天空Image强句柄。
            commands.entity(entity).insert((
                runtime, // 原快照继续保留，后续重新选择不会把Disabled当作者基线。
                AuthoredSkyboxStatus {
                    phase: AuthoredSkyboxPhase::Disabled, // 明确区分未设置与失败。
                    asset_path: None,                     // 禁用态没有当前依赖。
                    revision: request.revision,           // 记录清除动作修订。
                    detail: "场景未指定天空资源，使用动态天空与相机原环境贴图".to_owned(), // 清除资源后的明确安全模式反馈。
                    visible_skybox_active: false, // 动态大气与动态云恢复资格；天体圆盘不受天空模式控制。
                },
            ));
            continue;
        };
        if request.mode == AuthoredSkyMode::Dynamic {
            restore_original_visible_sky(&mut commands, entity, &runtime); // 动态模式立即停止显示旧作者 Cube，IBL 可继续热换。
            runtime.active_visible_handle = None; // 后续动态系统不得把外部原背景误认成作者天空盒。
            if runtime.active_source.is_some() {
                runtime.active_mode = Some(AuthoredSkyMode::Dynamic); // 旧 IBL 在新两件套就绪前继续服务动态天空。
            }
        }
        let visible_handle = (request.mode == AuthoredSkyMode::Skybox)
            .then(|| asset_server.load(paths.source.clone())); // 只有天空盒模式请求主 Cube，动态模式不产生无用解码与上传。
        let diffuse_handle = asset_server.load(paths.diffuse.clone()); // 漫反射使用离线余弦卷积Cube。
        let specular_handle = asset_server.load(paths.specular.clone()); // 镜面使用离线GGX完整Mip链。
        runtime.tracked = Some(TrackedAuthoredSkybox {
            mode: request.mode,         // 冻结本次可见来源，采用时不重新读取请求。
            paths: paths.clone(),       // 加载失败和修复事件按两件套各自回显实际路径。
            visible_handle,             // 动态模式为 None，天空盒模式强持有主 Cube。
            diffuse_handle,             // 强持有天空补光到请求清除或相机销毁。
            specular_handle,            // 强持有粗糙度反射到请求清除或相机销毁。
            revision: request.revision, // 迟到事件需与状态修订一致。
            base_brightness: request.base_brightness, // 主 Cube 采用时使用冻结作者亮度。
            base_environment_diffuse_intensity: request.base_environment_diffuse_intensity, // 人物与地形天空补光单独冻结。
            base_environment_specular_intensity: request.base_environment_specular_intensity, // 水面与光滑材质反射单独冻结。
            rotation: request.rotation(), // 只从已验证有限角构造四元数。
        });
        let visible_skybox_active = runtime.active_visible_handle.is_some(); // 在移动运行时组件前冻结上一有效主 Cube 状态。
        commands.entity(entity).insert((
            runtime, // 原子替换该相机当前唯一跟踪候选。
            AuthoredSkyboxStatus {
                phase: AuthoredSkyboxPhase::Loading, // 下一系统若资产已缓存可同帧采用。
                asset_path: Some(paths.source.clone()), // 状态以作者选择的天空资源主路径为稳定身份。
                revision: request.revision,             // 状态与候选使用同一身份。
                detail: match request.mode {
                    AuthoredSkyMode::Dynamic => {
                        format!("正在加载动态天空IBL两件套 assets/{}", paths.source) // 主 Cube 不进入动态模式资源集合。
                    }
                    AuthoredSkyMode::Skybox => {
                        format!("正在加载天空盒与IBL三件套 assets/{}", paths.source) // 三张图必须一起通过格式门。
                    }
                },
                visible_skybox_active, // 热换天空盒时保留上一有效主 Cube。
            },
        ));
    }
}

/// Image热更新或失败只触发匹配相机换代；同一路径修复后可重新进入加载与采用。
pub(crate) fn observe_authored_skybox_assets(
    mut commands: Commands, // 失败时恢复原环境 IBL，成功事件时更新状态阶段。
    asset_server: Res<AssetServer>, // 消息被淘汰时按当前权威LoadState保守恢复或复验。
    images: Res<Assets<Image>>, // 只在匹配事件或消息淘汰时读取两台相机各自最多三张明确依赖图。
    image_events: Res<Messages<AssetEvent<Image>>>, // 全局Image成功消息由私有游标按预算读取。
    failures: Res<Messages<AssetLoadFailedEvent<Image>>>, // 全局Image失败消息同样不得无界消费。
    mut image_cursor: Local<MessageCursor<AssetEvent<Image>>>, // 未消费尾部跨帧保留，且可检测被双缓冲淘汰的数量。
    mut failure_cursor: Local<MessageCursor<AssetLoadFailedEvent<Image>>>, // 失败消息使用独立身份游标。
    mut environment_assets: ResMut<AuthoredEnvironmentMapAssets>, // 热更新时失效最多四个小型漫反射比例缓存。
    mut runtimes: Query<(Entity, &mut AuthoredSkyboxRuntime, &AuthoredSkyboxStatus)>, // 通常只有玩法与F12两台相机。
) {
    let missed_messages = image_cursor
        .missed_messages(&image_events)
        .saturating_add(failure_cursor.missed_messages(&failures)); // 必须在read推进游标前取得真实淘汰证据。
    let changed_ids = image_cursor
        .read(&image_events)
        .take(SKYBOX_IMAGE_EVENT_BUDGET)
        .filter_map(|event| match *event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id }
            | AssetEvent::Removed { id } => Some(id), // 新内容与明确移除都必须重新仲裁整套绑定。
            AssetEvent::Unused { .. } => None, // 当前两件套由强句柄持有，不会因Unused成为活动资源移除证据。
        })
        .collect::<Vec<_>>(); // 单帧Image事件通常很少，列表只存Copy AssetId且生命周期为本系统。
    let failed_ids = failure_cursor
        .read(&failures)
        .take(SKYBOX_FAILURE_EVENT_BUDGET)
        .map(|failure| failure.id)
        .collect::<Vec<_>>(); // 同帧失败详情有固定上限，并供最多两台明确相机匹配。
    invalidate_scaled_diffuse_cubes(
        &mut environment_assets, // 丢弃已由旧源像素派生的补光图。
        &changed_ids,            // 正常新增、修改和移除事件按精确源身份失效。
        missed_messages > 0,     // 消息淘汰时保守清空全部四项，避免复用无法证明新鲜的图。
    );
    if missed_messages == 0 {
        invalidate_scaled_diffuse_cubes(
            &mut environment_assets, // 加载失败同样不得保留旧源比例缓存。
            &failed_ids,             // 失败句柄身份来自独立有界消息流。
            false,                   // 上方已经处理全量清空分支。
        );
    }
    if changed_ids.is_empty() && failed_ids.is_empty() && missed_messages == 0 {
        return; // 稳定帧不遍历任何天空相机。
    }
    for (entity, mut runtime, status) in &mut runtimes {
        let Some(tracked) = runtime.tracked.clone() else {
            continue; // 禁用或字段失败的相机没有可匹配Image。
        };
        if status.revision != tracked.revision {
            continue; // 防御延迟命令窗口，旧状态不得接受新候选事件。
        }
        let relevant_change = tracked.assets().any(|(_, handle, _)| {
            changed_ids.contains(&handle.id()) || failed_ids.contains(&handle.id())
        }); // 当前模式两张或三张 Image 任一事件都使整套绑定重新仲裁。
        if !relevant_change && missed_messages == 0 {
            continue; // 与当前两台相机无关的Image消息不触发组件写入。
        }
        match inspect_tracked_assets(&asset_server, &images, &tracked) {
            TrackedAssetInspection::Ready => {
                commands.entity(entity).insert(AuthoredSkyboxStatus {
                    phase: AuthoredSkyboxPhase::Loading, // 下一链式系统重新读取并原子安装当前完整资源组。
                    asset_path: Some(tracked.paths.source.clone()), // 诊断主身份保持作者历史源路径。
                    revision: tracked.revision,                     // 热更新不伪造作者修订。
                    detail: if missed_messages > 0 {
                        format!("天空资源消息积压丢失{missed_messages}条，当前资源组已重新验证")
                    } else {
                        "天空资源已更新，正在原子重新采用".to_owned()
                    }, // 明确说明是保守复验还是实际热更新。
                    visible_skybox_active: runtime.active_visible_handle.is_some(), // 新候选采用前保留上一有效背景状态。
                });
            }
            TrackedAssetInspection::Loading(detail) => {
                if status.phase != AuthoredSkyboxPhase::Loading || missed_messages > 0 {
                    let preserves_previous = has_distinct_active_sky(&runtime, &tracked); // 换路径时旧完整结果仍可安全显示。
                    if !preserves_previous {
                        restore_original_sky(&mut commands, entity, &mut runtime); // 同身份资源失效时回退动态天空，避免显示半更新 Cube。
                    }
                    commands.entity(entity).insert(AuthoredSkyboxStatus {
                        phase: AuthoredSkyboxPhase::Loading, // 保持异步等待且不阻塞帧。
                        asset_path: Some(tracked.paths.source.clone()), // 状态仍属于同一作者环境身份。
                        revision: tracked.revision,                     // 保持当前请求身份。
                        detail: if preserves_previous {
                            format!("{detail}；继续使用上一套有效天空资源") // 热换候选未完整前不闪回空背景。
                        } else {
                            detail // 首次加载或同身份热更使用动态天空安全回退。
                        },
                        visible_skybox_active: runtime.active_visible_handle.is_some(), // 只有真实保留的旧主 Cube 才关闭动态天空。
                    });
                }
            }
            TrackedAssetInspection::Failed(detail) | TrackedAssetInspection::Invalid(detail) => {
                let preserves_previous = has_distinct_active_sky(&runtime, &tracked); // 新路径失败不破坏已经完整采用的旧路径。
                if !preserves_previous {
                    restore_original_sky(&mut commands, entity, &mut runtime); // 首次失败或同身份损坏立即回退动态天空。
                }
                commands.entity(entity).insert(AuthoredSkyboxStatus {
                    phase: AuthoredSkyboxPhase::Failed, // 后续同句柄修复事件仍可重新进入Loading。
                    asset_path: Some(tracked.paths.source.clone()), // 保留作者主路径供F12定位。
                    revision: tracked.revision,         // 失败不改变作者修订。
                    detail: if preserves_previous {
                        format!("{detail}；已保留上一套有效天空资源") // 明确当前画面不是失败候选。
                    } else {
                        detail // 首次或同身份失败直接显示具体原因。
                    },
                    visible_skybox_active: runtime.active_visible_handle.is_some(), // 只决定大气与动态云是否需要接管回退。
                });
            }
        }
    }
}

/// 只轮询 Loading 相机并在当前模式的完整资源组存在时原子替换天空与环境贴图。
pub(crate) fn apply_authored_skyboxes(
    mut commands: Commands, // 成功采用或格式失败恢复都通过同一命令事务。
    mut images: ResMut<Assets<Image>>, // Loading边界只读两至三张源图，并可登记一张最大64²的补光比例图。
    mut environment_assets: ResMut<AuthoredEnvironmentMapAssets>, // 两台相机共享黑Cube和最多四项派生缓存。
    mut candidates: Query<(Entity, &mut AuthoredSkyboxRuntime, &AuthoredSkyboxStatus)>, // 最多两台相机，稳定Ready立即跳过。
) {
    for (entity, mut runtime, status) in &mut candidates {
        if status.phase != AuthoredSkyboxPhase::Loading {
            continue; // Ready、Disabled和Failed稳定帧没有资产轮询或组件写入。
        }
        let Some(tracked) = runtime.tracked.clone() else {
            continue; // 防御状态与运行时短暂不同步。
        };
        if status.revision != tracked.revision {
            continue; // 旧Loading状态不能采用新请求的句柄。
        }
        let visible_image = match tracked.visible_handle.as_ref() {
            Some(handle) => {
                let Some(image) = images.get(handle.id()) else {
                    continue; // 天空盒主 Cube 尚未解码时不采用后两张 IBL。
                };
                Some(image) // 三件套验证边界保留主 Cube 只读引用。
            }
            None => None, // 动态模式明确不加载或验证可见主 Cube。
        };
        let Some(diffuse_image) = images.get(tracked.diffuse_handle.id()) else {
            continue; // 漫反射辐照异步解码尚未完成，不采用半套环境。
        };
        let Some(specular_image) = images.get(tracked.specular_handle.id()) else {
            continue; // GGX镜面异步解码尚未完成，不采用半套环境。
        };
        let validation = visible_image
            .map_or(Ok(()), validate_visible_cube_image) // 天空盒模式先验证主 Cube，动态模式直接通过该阶段。
            .map_err(|detail| format!("可见天空：{detail}"))
            .and_then(|()| {
                validate_diffuse_cube_image(diffuse_image)
                    .map_err(|detail| format!("漫反射辐照：{detail}"))
            })
            .and_then(|()| {
                validate_specular_cube_image(specular_image)
                    .map_err(|detail| format!("GGX镜面：{detail}"))
            }); // 当前模式要求的两种或三种用途在一次采用边界内完成格式验证。
        if let Err(detail) = validation {
            let preserves_previous = has_distinct_active_sky(&runtime, &tracked); // 换路径或换模式时允许保留上一完整结果。
            if !preserves_previous {
                restore_original_sky(&mut commands, entity, &mut runtime); // 首次或同身份失败回退动态天空安全基线。
            }
            commands.entity(entity).insert(AuthoredSkyboxStatus {
                phase: AuthoredSkyboxPhase::Failed, // 同路径下次Modified可再次验证并恢复。
                asset_path: Some(tracked.paths.source.clone()), // 回显作者主资源身份。
                revision: tracked.revision,         // 失败绑定当前请求。
                detail: if preserves_previous {
                    format!("天空资源格式无效：{detail}；已保留上一套有效天空资源") // 新候选不破坏旧画面。
                } else {
                    format!("天空资源格式无效：{detail}") // 当前身份无法继续显示时明确回退原因。
                },
                visible_skybox_active: runtime.active_visible_handle.is_some(), // 动态表现按真实旧主 Cube 状态决定是否接管。
            });
            continue;
        }
        let resolved = match resolve_environment_maps(
            &mut environment_assets,  // 相同比例优先复用另一台相机已经创建的派生图。
            &mut images,              // 仅复制已经通过格式与固定尺寸门的漫反射源图。
            &tracked.diffuse_handle,  // 离线余弦卷积源保持唯一作者资源身份。
            &tracked.specular_handle, // 完整GGX链绝不进行CPU复制或重采样。
            tracked.base_environment_diffuse_intensity, // 独立人物与地形天空补光基准。
            tracked.base_environment_specular_intensity, // 独立水面与光滑材质反射基准。
        ) {
            Ok(resolved) => resolved, // 两个句柄和公共载体强度已完整就绪。
            Err(detail) => {
                let preserves_previous = has_distinct_active_sky(&runtime, &tracked); // 新候选派生失败可继续使用上一完整结果。
                if !preserves_previous {
                    restore_original_sky(&mut commands, entity, &mut runtime); // 当前身份派生失败时完整回退。
                }
                commands.entity(entity).insert(AuthoredSkyboxStatus {
                    phase: AuthoredSkyboxPhase::Failed, // 作者修正强度或源图热更后仍可重试。
                    asset_path: Some(tracked.paths.source.clone()), // 诊断保持主环境身份。
                    revision: tracked.revision,         // 失败绑定当前作者修订。
                    detail: if preserves_previous {
                        format!("天空补光独立强度未采用：{detail}；已保留上一套有效天空资源") // 旧完整结果继续稳定显示。
                    } else {
                        format!("天空补光独立强度未采用：{detail}") // 明确区别于资源加载失败。
                    },
                    visible_skybox_active: runtime.active_visible_handle.is_some(), // 失败回退后允许动态天空接管。
                });
                continue;
            }
        };
        let visible_handle = tracked.visible_handle.clone(); // 在写运行时身份前冻结可见主 Cube 句柄。
        if tracked.mode == AuthoredSkyMode::Dynamic {
            restore_original_visible_sky(&mut commands, entity, &runtime); // 动态模式只采用 IBL，不允许旧作者 Cube 残留。
        }
        runtime.active_source = Some(tracked.paths.source.clone()); // 完整采用后记录当前稳定环境身份。
        runtime.active_mode = Some(tracked.mode); // 后续热换据此区分旧有效结果和同身份重载。
        runtime.active_visible_handle = visible_handle.clone(); // 动态模式写 None，天空盒模式持有主 Cube。
        let mut entity_commands = commands.entity(entity); // 可见背景与 PBR 环境组件在同一采用边界写入。
        entity_commands.remove::<GeneratedEnvironmentMapLight>(); // 接管时停止外部实时生成器，首次原值已冻结供回退。
        entity_commands.insert((
            EnvironmentMapLight {
                diffuse_map: resolved.diffuse_map, // 小型辐照图像素已编码独立补光/反射比例。
                specular_map: resolved.specular_map, // 完整Mip链让水面在斜视角继续清晰反射天空。
                intensity: resolved.carrier_intensity, // 单一Bevy Uniform只承载镜面值或零镜面时的补光值。
                rotation: tracked.rotation, // 补光与反射方向共同采用作者设置，避免两个 IBL 通道错位。
                affects_lightmapped_mesh_diffuse: true, // 动态场景与烘焙网格都接收同一天空补光。
            },
            WorldEnvironmentMapProfile::new(
                tracked.base_environment_diffuse_intensity, // 昼夜系统保留完整补光作者语义。
                tracked.base_environment_specular_intensity, // 昼夜系统按镜面载体重算公共Uniform。
            ),
        ));
        let (detail, visible_skybox_active) = match (tracked.mode, visible_handle) {
            (AuthoredSkyMode::Dynamic, _) => (
                format!(
                    "动态天空已启用，漫反射辐照与GGX镜面已就绪 assets/{}",
                    tracked.paths.source
                ), // 动态模式不加载主 Cube，但完整采用两张 IBL。
                false, // 原生大气与动态云保持启用，天体圆盘在两种模式下都持续工作。
            ),
            (AuthoredSkyMode::Skybox, Some(image)) => {
                entity_commands.insert((
                    Skybox {
                        image: Some(image),                  // 可见背景使用作者主 HDR Cube。
                        brightness: tracked.base_brightness, // 后续光照系统写入昼夜与天气倍率。
                        rotation: tracked.rotation,          // 可见背景与两张 IBL 保持同一方向。
                    },
                    WorldSkyProfile::new(tracked.base_brightness), // 保存不受逐帧累乘的日间天空盒基准。
                ));
                (
                    format!(
                        "天空盒、漫反射辐照与GGX镜面已就绪 assets/{}",
                        tracked.paths.source
                    ), // 三件套已在同一帧完整采用。
                    true, // 只关闭原生大气与动态云，模拟太阳/月亮仍由世界时间系统维护。
                )
            }
            (AuthoredSkyMode::Skybox, None) => {
                unreachable!("天空盒候选通过验证时必须持有主Cube句柄")
            } // 构造器保证三件套身份完整。
        };
        entity_commands.insert(AuthoredSkyboxStatus {
            phase: AuthoredSkyboxPhase::Ready, // 当前模式完整资源组已原子应用。
            asset_path: Some(tracked.paths.source.clone()), // 状态主身份保持作者选择的主路径。
            revision: tracked.revision,        // Ready 属于当前请求修订。
            detail,                            // F12 明确显示两件套或三件套结果。
            visible_skybox_active, // 动态表现系统只读取真实采用结果，不因模式加载失败错误关闭。
        });
    }
}

/// 确认可见天空确实可作为 Cube 纹理绑定，而不是仅凭 KTX2 扩展名猜测。
fn validate_visible_cube_image(image: &Image) -> Result<(), String> {
    validate_cube_image(image) // 可见背景无需粗糙度 Mip，但仍必须满足全部通用 Cube 契约。
}

/// 确认漫反射图是已离线卷积的单级Cube，避免错误资源暗中改变补光频率。
fn validate_diffuse_cube_image(image: &Image) -> Result<(), String> {
    validate_cube_image(image)?; // 先执行跨三种用途一致的Cube和GPU用途门。
    if image.texture_descriptor.mip_level_count != 1 {
        return Err(format!(
            "Mip数量为{}，漫反射辐照图必须恰好为1级",
            image.texture_descriptor.mip_level_count
        )); // 漫反射只采样预卷积结果，不需要额外粗糙度级别。
    }
    let width = image.texture_descriptor.size.width; // 通用门已经保证宽高相同且非零。
    if width > MAX_AUTHORED_DIFFUSE_EDGE {
        return Err(format!(
            "漫反射单面尺寸{width}超过运行时独立强度允许的{MAX_AUTHORED_DIFFUSE_EDGE}像素上限"
        )); // 同步比例派生必须保持固定小预算，禁止误把高分辨率源交给主线程复制。
    }
    if image.texture_descriptor.format != bevy::render::render_resource::TextureFormat::Rgba16Float
    {
        return Err(format!(
            "漫反射格式为{:?}，独立补光强度要求RGBA16Float",
            image.texture_descriptor.format
        )); // 逐通道半浮点缩放必须和离线生成格式严格一致。
    }
    if !image.asset_usage.contains(RenderAssetUsages::MAIN_WORLD) {
        return Err("漫反射图未保留MAIN_WORLD用途，无法在作者强度变化时派生小型比例图".to_owned()); // 只在主世界保留CPU像素才能安全独立缩放。
    }
    let expected_bytes = width as usize * width as usize * 6 * 8; // 单Mip、六面、RGBA16F固定为每像素八字节。
    let Some(data) = image.data.as_ref() else {
        return Err("漫反射图缺少主世界RGBA16F像素".to_owned()); // 不能退回共享强度破坏作者分离语义。
    };
    if data.len() != expected_bytes {
        return Err(format!(
            "漫反射像素为{}字节，{width}像素六面RGBA16F应为{expected_bytes}字节",
            data.len()
        )); // 防止带额外布局或截断数据在逐通道缩放时被误解释。
    }
    Ok(()) // 单级、小尺寸且保留CPU半浮点像素的Cube可独立调节补光。
}

/// 确认镜面图覆盖从清晰水面到全粗糙材质所需的完整Mip链。
fn validate_specular_cube_image(image: &Image) -> Result<(), String> {
    validate_cube_image(image)?; // 先验证Cube维度、格式与渲染用途。
    let width = image.texture_descriptor.size.width; // 通用门已保证宽高相等且非零。
    if !width.is_power_of_two() {
        return Err(format!(
            "镜面单面尺寸{width}不是二次幂，无法建立完整粗糙度Mip链"
        )); // 完整二分链要求二次幂。
    }
    let expected_mips = width.ilog2() + 1; // 从Mip0一直到一像素的级别总数。
    if image.texture_descriptor.mip_level_count != expected_mips {
        return Err(format!(
            "Mip数量为{}，{width}像素GGX镜面图必须包含完整{expected_mips}级",
            image.texture_descriptor.mip_level_count
        )); // 单Mip正是粗糙材质错误采样尖锐天空产生银边的根因。
    }
    Ok(()) // 完整Mip合法Cube可按感知粗糙度选择预过滤结果。
}

/// 确认所有环境Image共有的Cube、可过滤格式和渲染用途契约。
fn validate_cube_image(image: &Image) -> Result<(), String> {
    if image.texture_descriptor.dimension != TextureDimension::D2 {
        return Err(format!(
            "纹理维度为{:?}，环境 IBL 必须是二维面的立方体",
            image.texture_descriptor.dimension
        )); // 三维体纹理不能绑定texture_cube。
    }
    if image.texture_descriptor.size.depth_or_array_layers != 6 {
        return Err(format!(
            "面数为{}，环境 IBL 必须恰好包含6个面",
            image.texture_descriptor.size.depth_or_array_layers
        )); // 当前入口不接受CubeArray或普通单层图。
    }
    let width = image.texture_descriptor.size.width;
    let height = image.texture_descriptor.size.height;
    if width == 0 || width != height {
        return Err(format!(
            "单面尺寸为{width}×{height}，环境贴图必须是非零正方形"
        )); // Cube纹理的六个面必须采用同一正方形尺寸。
    }
    let format = image.texture_descriptor.format;
    if format.sample_type(None, None) != Some(TextureSampleType::Float { filterable: true }) {
        return Err(format!(
            "纹理格式为{format:?}，环境镜面反射要求跨设备可过滤的浮点颜色格式"
        )); // 拒绝整数与依赖可选FLOAT32_FILTERABLE特性的格式，防止RenderApp绑定验证失败。
    }
    if !image
        .texture_descriptor
        .usage
        .contains(TextureUsages::TEXTURE_BINDING)
    {
        return Err("纹理未声明TEXTURE_BINDING用途，无法供PBR环境补光与反射采样".to_owned());
    }
    if !image.asset_usage.contains(RenderAssetUsages::RENDER_WORLD) {
        return Err("纹理只存在于主世界，无法上传为PBR环境贴图".to_owned());
    }
    let view_dimension = image
        .texture_view_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.dimension); // KTX2加载器必须显式发布Cube视图。
    if view_dimension != Some(TextureViewDimension::Cube) {
        return Err(format!("纹理视图为{view_dimension:?}，必须是Cube")); // 防止RenderApp绑定阶段才报wgpu错误。
    }
    Ok(()) // 尺寸、face和视图三项共同通过后才允许采用。
}

#[cfg(test)]
mod tests;
