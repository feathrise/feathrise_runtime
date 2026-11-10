// systems.rs — Billboard 文字 ECS 系统

use bevy::camera::visibility::RenderLayers; // 时间引导层只允许主玩法相机收集。
use bevy::core_pipeline::prepass::{DeferredPrepass, MotionVectorPrepass};
use bevy::ecs::system::SystemParam; // 聚合Billboard生命周期共同使用的动态资产库。
use bevy::ecs::{entity_disabling::Disabled, query::Allow}; // 替换可以回收仍被停驻标记隔离的旧实例。
use bevy::prelude::*; // Bevy 常用预导入
use bevy::render::camera::TemporalJitter; // RR 活动态由时间抖动与两类预通道共同判定。
use bevy::render::render_resource::TextureFormat; // 离屏纹理格式

use crate::components::{
    BillboardCamera, BillboardTemporalGuide, BillboardText, BillboardTextHandle, BillboardTextLink,
};
use crate::helpers::{build_billboard_mesh, mesh_width_for_pixel_width};
use crate::materials::{
    BILLBOARD_PROJECTION_JITTERED, BILLBOARD_PROJECTION_UNJITTERED, BillboardColorMaterial,
    BillboardTemporalGuideMaterial, billboard_color_material, billboard_temporal_guide_material,
};
use crate::resources::BillboardBakePool;

/// 时间重建引导系统的公开排序边界。
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum BillboardTextSystems {
    /// 在画质运行时租约落地后、可见性传播前同步主相机与引导子实体。
    SyncTemporalGuides,
}

/// 时间引导层使用内联位图最后一层，避免进入默认、小场景、角色预览或离屏烘焙相机。
const BILLBOARD_TEMPORAL_GUIDE_LAYER: usize = 63;

/// 记录当前由本插件临时扩展 RenderLayers 的唯一主相机及精确回退信息。
#[derive(Resource, Default)]
pub(crate) struct BillboardTemporalGuideState {
    active_camera: Option<Entity>,
    camera_layers_were_explicit: bool,
    owns_camera_layer: bool,
    projection_is_stable: bool,
}

/// Billboard 销毁路径共同使用的动态资产库。
#[derive(SystemParam)]
pub struct BillboardAssetStores<'w> {
    images: ResMut<'w, Assets<Image>>, // 每个Billboard独占的最终文字纹理。
    meshes: ResMut<'w, Assets<Mesh>>,  // 最终四边形与临时装饰网格。
    materials: ResMut<'w, Assets<BillboardColorMaterial>>, // 最终世界空间无光照材质。
    temporal_guide_materials: ResMut<'w, Assets<BillboardTemporalGuideMaterial>>, // RR字形引导材质。
    color_materials: ResMut<'w, Assets<ColorMaterial>>, // 临时2D装饰材质。
}

impl BillboardAssetStores<'_> {
    /// 释放一个 Billboard 独占的全部动态资产。
    fn release(&mut self, tag: &BillboardTextLink) {
        self.materials.remove(tag.material.id()); // 最终3D材质不与其他Billboard共享。
        self.temporal_guide_materials
            .remove(tag.temporal_guide_material.id()); // 引导层同样逐Billboard独占同一烘焙纹理。
        self.images.remove(tag.image.id()); // 最终离屏纹理不与其他Billboard共享。
        self.meshes.remove(tag.mesh.id()); // 最终四边形按当前文字宽度独立生成。
        for deco_mesh in &tag.decoration_meshes {
            self.meshes.remove(deco_mesh.id()); // 烘焙中断时同步释放短命装饰网格。
        }
        for deco_material in &tag.decoration_materials {
            self.color_materials.remove(deco_material.id()); // 烘焙中断时同步释放短命装饰材质。
        }
    }
}

/// Update 系统：消费 `BillboardText` 请求，烘焙出一个世界空间文字 Billboard。
///
/// 每处理一个请求：创建离屏纹理与3D四边形 → 加入固定相机池等待队列 →
/// 回写句柄并移除宿主上的请求组件。此处绝不按请求数量创建Camera2d。
pub fn spawn_billboard_texts(
    mut commands: Commands,            // 创建最终3D Billboard并消费请求组件。
    mut asset_stores: BillboardAssetStores, // 生成与替换共用完整资产集合，不让业务层分别回收文字资源。
    mut bake_pool: ResMut<BillboardBakePool>, // 只把新任务排队，不增加固定相机数量。
    query: Query<(Entity, &BillboardText, &Transform, Option<&BillboardTextHandle>)>,
    links: Query<&BillboardTextLink, Allow<Disabled>>, // 重发请求时查找当前实例的完整资产所有权。
) {
    // 请求消费后即被移除，所以只查询现存请求即可；驻留期间插入的请求不依赖过期的Added/Changed tick。
    for (owner, spec, transform, previous) in query.iter() {
        if let Some(previous) = previous {
            if let Ok(link) = links.get(previous.0) {
                // 同一锚点可重发原规格；排队、烘焙或已显示的旧实例都走TTL相同的完整回收路径。
                despawn_billboard(&mut commands, &mut asset_stores, previous.0, link);
            }
        }
        // 同一帧任意数量请求均可进入FIFO队列。
        let img = Image::new_target_texture(
            spec.texture_size.x,                 // 使用调用方指定的纹理宽度。
            spec.texture_size.y,                 // 使用调用方指定的纹理高度。
            TextureFormat::Rgba8Unorm,           // GPU写入使用线性RGBA8目标格式。
            Some(TextureFormat::Rgba8UnormSrgb), // 采样视图按sRGB解释文字颜色。
        );
        let img_handle = asset_stores.images.add(img); // 提前创建最终纹理，让业务层立即获得稳定Billboard句柄。
        let mat = asset_stores.materials.add(billboard_color_material(img_handle.clone()));
        let temporal_guide_material = asset_stores.temporal_guide_materials.add(
            billboard_temporal_guide_material(img_handle.clone()), // 与最终文字共享纹理和局部几何。
        );
        // 初始网格：裁切模式先用整宽占位（待 sync 裁切）；非裁切模式直接按纹理宽高比定型
        let texture_aspect = spec.texture_size.x as f32 / spec.texture_size.y.max(1) as f32;
        let full_mesh_width = spec.world_height * texture_aspect; // 整张纹理对应的世界宽度
        let init_mesh_width = if spec.crop_to_text {
            mesh_width_for_pixel_width(
                spec.texture_size.x as f32,
                spec.texture_size.y as f32,
                spec.world_height,
            )
        } else {
            full_mesh_width // 正方形/整图徽标：宽度由纹理宽高比固定
        };
        let mesh = asset_stores.meshes.add(build_billboard_mesh(
            init_mesh_width,
            spec.world_height,
            0.0,
            1.0,
        ));

        // 非裁切模式不等待布局，但所有任务都必须等固定相机写入最终纹理后才能显示。
        let needs_layout_sync = spec.crop_to_text;
        let initial_visibility = Visibility::Hidden; // 排队及烘焙阶段统一隐藏，避免短暂显示透明或旧目标内容。

        // 跟随目标：默认为承载请求的实体，可被 follow_target 覆盖
        let follow_owner = spec.follow_target.unwrap_or(owner);

        let billboard_entity = commands // 最终3D实体立即创建，但烘焙完成前保持隐藏。
            .spawn((
                Name::new("Billboard_Text_Quad"), // 调试层级中标识最终文字平面。
                Mesh3d(mesh.clone()),             // 世界空间文字平面网格。
                MeshMaterial3d(mat.clone()),      // 世界空间文字平面材质。
                initial_visibility,               // 裁切模式和排队阶段均由更新系统继续保持隐藏。
                Transform::from_translation(transform.translation + Vec3::Y * spec.y_offset), // 初始位置与旧实现一致。
                bevy::light::NotShadowCaster, // 头顶UI不向场景投射阴影。
            ))
            .id();

        let temporal_guide_entity = commands
            .spawn((
                Name::new("Billboard_Temporal_Guide"),
                BillboardTemporalGuide,
                Mesh3d(mesh.clone()), // 复用同一网格资产，裁切更新会同时作用于颜色层与引导层。
                MeshMaterial3d(temporal_guide_material.clone()),
                RenderLayers::from_layers(&[BILLBOARD_TEMPORAL_GUIDE_LAYER]),
                Visibility::Hidden, // 只有主相机具备完整RR输入链时才由PostUpdate切为Inherited。
                Transform::IDENTITY, // 子实体完整继承颜色层的位置、缩放和朝向。
                ChildOf(billboard_entity),
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();

        commands.entity(billboard_entity).insert(BillboardTextLink {
            owner: follow_owner,                        // 保存最终跟随目标。
            text_entity: None,                          // 获得相机槽位后才生成临时Text2d。
            outline_entities: Vec::new(),               // 获得相机槽位后才生成临时描边。
            decoration_entities: Vec::new(),            // 获得相机槽位后才生成临时装饰。
            mesh: mesh.clone(),                         // 最终3D网格供裁切和销毁使用。
            material: mat.clone(),                      // 最终3D材质供销毁时释放。
            temporal_guide_entity,                      // 子实体跟随父级并负责RR预通道输入。
            temporal_guide_material,                    // 销毁时同步释放独占引导材质。
            image: img_handle,                          // 固定相机槽位将写入这张最终纹理。
            decoration_meshes: Vec::new(),              // 活跃烘焙期间暂存动态装饰网格。
            decoration_materials: Vec::new(),           // 活跃烘焙期间暂存动态装饰材质。
            y_offset: spec.y_offset,                    // 保留调用方头顶偏移。
            scale: spec.scale,                          // 保留调用方世界缩放。
            follow: spec.follow,                        // 保留跟随或固定显示模式。
            distance_scaling: spec.distance_scaling,    // 保留近大远小设置。
            ref_distance: spec.ref_distance,            // 保留距离缩放参考值。
            max_distance: spec.max_distance,            // 保留最大显示距离。
            texture_width: spec.texture_size.x as f32,  // 后续布局裁切使用纹理宽度。
            texture_height: spec.texture_size.y as f32, // 后续世界宽度换算使用纹理高度。
            world_height: spec.world_height,            // 保留最终世界高度。
            padding_px: spec.padding_px,                // 保留文字水平防裁切边距。
            ttl: spec.ttl,                              // 只在烘焙就绪后开始倒计时。
            bake_spec: Some(spec.clone()),              // 排队期间保存生成2D源所需的完整规格。
            bake_ready: false,                          // 固定相机尚未写入最终纹理。
            needs_layout_sync,                          // 裁切文字需要等待TextLayoutInfo。
            bake_frames_remaining: 2,                   // 布局完成后至少保留一个完整最终渲染帧。
        });

        bake_pool.waiting.push_back(billboard_entity); // 所有超出固定槽位容量的请求自然留在FIFO队列。
        // 把生成的 Billboard 实体回写到请求实体上，供调用方后续销毁/重建
        commands
            .entity(owner)
            .insert(BillboardTextHandle(billboard_entity))
            .remove::<BillboardText>();
    }
}

/// PostUpdate：只在完整时间重建主视图中启用字形 Deferred/Motion 引导层。
///
/// `AlphaMode::Blend` 最终文字仍负责软边与原色；引导子实体使用专属 RenderLayer，
/// 因而不会被缺少 Deferred phase 的小场景、角色预览或其他 Forward 相机收集。
pub(crate) fn sync_billboard_temporal_guides(
    mut commands: Commands,
    mut state: ResMut<BillboardTemporalGuideState>,
    temporal_cameras: Query<
        (
            Entity,
            &Camera,
            Has<DeferredPrepass>,
            Has<MotionVectorPrepass>,
            Has<TemporalJitter>,
            Has<crate::components::BillboardTemporalGuideSuppressed>, // 独立界面合成不能把字形写回场景深度和 motion。
        ),
        (With<BillboardCamera>, With<Camera3d>),
    >,
    mut camera_layers: Query<Option<&mut RenderLayers>, With<Camera3d>>,
    mut color_materials: Option<ResMut<Assets<BillboardColorMaterial>>>,
    mut guide_materials: Option<ResMut<Assets<BillboardTemporalGuideMaterial>>>,
    mut projection_links: ParamSet<(
        Query<&BillboardTextLink>,
        Query<&BillboardTextLink, Added<BillboardTextLink>>,
    )>,
    mut guide_queries: ParamSet<(
        Query<&mut Visibility, With<BillboardTemporalGuide>>,
        Query<&mut Visibility, Added<BillboardTemporalGuide>>,
    )>,
) {
    let mut eligible = temporal_cameras
        .iter()
        .filter(|(_, camera, deferred, motion, jitter, suppressed)| {
            // 暂停资格属于相机级契约，不逐帧修改所有文字。
            !*suppressed && temporal_guide_camera_is_eligible(camera, *deferred, *motion, *jitter) // 普通模式完整保留原 Deferred+Motion+Jitter 规则。
        })
        .map(|(entity, ..)| entity);
    let candidate = eligible.next();
    let active_camera = if candidate.is_some() && eligible.next().is_none() {
        candidate // 只接受唯一主视图，避免一个全局引导可见状态跨多个相机产生歧义。
    } else {
        None
    };

    let stable_projection = temporal_cameras
        .iter()
        .any(|(_, camera, .., suppressed)| camera.is_active && suppressed);
    if state.projection_is_stable != stable_projection {
        if let (Some(color_materials), Some(guide_materials)) = (
            color_materials.as_deref_mut(),
            guide_materials.as_deref_mut(),
        ) {
            for link in projection_links.p0().iter() {
                set_billboard_projection_mode(
                    link,
                    stable_projection,
                    color_materials,
                    guide_materials,
                );
            }
        }
        state.projection_is_stable = stable_projection;
    } else if stable_projection {
        // 处于独立UI模式时只补同步本帧新建的材质，不扫描全部常驻名牌。
        if let (Some(color_materials), Some(guide_materials)) = (
            color_materials.as_deref_mut(),
            guide_materials.as_deref_mut(),
        ) {
            for link in projection_links.p1().iter() {
                set_billboard_projection_mode(link, true, color_materials, guide_materials);
            }
        }
    }

    if state.active_camera != active_camera {
        if let Some(previous_camera) = state.active_camera {
            release_temporal_guide_camera_layer(
                &mut commands,
                &mut camera_layers,
                previous_camera,
                state.camera_layers_were_explicit,
                state.owns_camera_layer,
            );
        }

        let (camera_layers_were_explicit, owns_camera_layer) = active_camera
            .map(|camera| {
                acquire_temporal_guide_camera_layer(&mut commands, &mut camera_layers, camera)
            })
            .unwrap_or_default();
        state.active_camera = active_camera;
        state.camera_layers_were_explicit = camera_layers_were_explicit;
        state.owns_camera_layer = owns_camera_layer;

        let desired_visibility = if active_camera.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for mut visibility in guide_queries.p0().iter_mut() {
            if *visibility != desired_visibility {
                *visibility = desired_visibility; // 模式切换才扫描全部现存名牌，不占稳定帧完整场景预算。
            }
        }
    } else {
        let desired_visibility = if active_camera.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for mut visibility in guide_queries.p1().iter_mut() {
            if *visibility != desired_visibility {
                *visibility = desired_visibility; // 稳定帧只初始化本帧新增的引导子实体。
            }
        }
    }
}

/// 同时切换最终颜色层和RR引导层，保证普通主通道与独立WorldUI的几何投影完全对齐。
fn set_billboard_projection_mode(
    link: &BillboardTextLink,
    stable: bool,
    color_materials: &mut Assets<BillboardColorMaterial>,
    guide_materials: &mut Assets<BillboardTemporalGuideMaterial>,
) {
    let mode = if stable {
        BILLBOARD_PROJECTION_UNJITTERED
    } else {
        BILLBOARD_PROJECTION_JITTERED
    };
    if let Some(mut material) = color_materials.get_mut(&link.material) {
        material.extension.projection_mode = mode;
    }
    if let Some(mut material) = guide_materials.get_mut(&link.temporal_guide_material) {
        material.extension.projection_mode = mode;
    }
}

fn temporal_guide_camera_is_eligible(
    camera: &Camera,
    deferred: bool,
    motion: bool,
    jitter: bool,
) -> bool {
    camera.is_active && deferred && motion && jitter
}

fn render_layers_contains(layers: &RenderLayers, layer: usize) -> bool {
    layers.iter().any(|current| current == layer)
}

fn acquire_temporal_guide_camera_layer(
    commands: &mut Commands,
    camera_layers: &mut Query<Option<&mut RenderLayers>, With<Camera3d>>,
    camera: Entity,
) -> (bool, bool) {
    match camera_layers.get_mut(camera) {
        Ok(Some(mut layers)) => {
            let owns_layer = !render_layers_contains(&layers, BILLBOARD_TEMPORAL_GUIDE_LAYER);
            if owns_layer {
                *layers = layers.clone().with(BILLBOARD_TEMPORAL_GUIDE_LAYER);
            }
            (true, owns_layer)
        }
        Ok(None) => {
            commands
                .entity(camera)
                .try_insert(RenderLayers::default().with(BILLBOARD_TEMPORAL_GUIDE_LAYER));
            (false, true)
        }
        Err(_) => (false, false), // 相机若同帧销毁，下一帧候选自然失效并保持引导隐藏。
    }
}

fn release_temporal_guide_camera_layer(
    commands: &mut Commands,
    camera_layers: &mut Query<Option<&mut RenderLayers>, With<Camera3d>>,
    camera: Entity,
    layers_were_explicit: bool,
    owns_layer: bool,
) {
    if !owns_layer {
        return; // 调用方原本已经持有保留层时，本插件不取得也不释放其所有权。
    }
    let Ok(Some(mut layers)) = camera_layers.get_mut(camera) else {
        return;
    };
    let restored = layers.clone().without(BILLBOARD_TEMPORAL_GUIDE_LAYER);
    if !layers_were_explicit && restored == RenderLayers::default() {
        commands.entity(camera).try_remove::<RenderLayers>(); // 精确恢复作者原本缺失组件的状态。
    } else {
        *layers = restored; // 只移除本插件保留层，保留运行期间其他系统追加的全部层。
    }
}

/// Update 系统：每帧跟随宿主、朝向相机、按距离缩放/隐藏，并在 ttl 到期或宿主消失时销毁。
pub fn update_billboard_texts(
    mut commands: Commands,
    time: Res<Time>,
    mut asset_stores: BillboardAssetStores,
    camera_query: Query<&Transform, (With<BillboardCamera>, Without<BillboardTextLink>)>,
    owner_query: Query<&Transform, Without<BillboardTextLink>>,
    mut tag_query: Query<(
        Entity,
        &mut BillboardTextLink,
        &mut Transform,
        &mut Visibility,
    )>,
) {
    let dt = time.delta_secs();
    let camera = camera_query.iter().next();
    for (entity, mut tag, mut transform, mut visibility) in tag_query.iter_mut() {
        let owner_position = if tag.follow {
            let Ok(owner_transform) = owner_query.get(tag.owner) else {
                // 宿主在排队、烘焙或显示任意阶段消失，都立即释放Billboard资源，队列中的旧实体会被安全跳过。
                despawn_billboard(&mut commands, &mut asset_stores, entity, &tag);
                continue;
            };
            Some(owner_transform.translation) // 单次查询同时完成存在性验证和本帧位置快照。
        } else {
            None // 固定文字不读取宿主，保持自身当前位置。
        };
        if !tag.bake_ready {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden; // 排队和固定相机烘焙期间都不显示未完成纹理，TTL也不在此阶段扣减。
            }
            continue;
        }
        // TTL 到期：销毁 Billboard 及其离屏子实体
        if let Some(ttl) = tag.ttl.as_mut() {
            *ttl -= dt;
            if *ttl <= 0.0 {
                despawn_billboard(&mut commands, &mut asset_stores, entity, &tag);
                continue;
            }
        }
        // 锚点位置：跟随宿主或保持原位
        let anchor_pos = match owner_position {
            Some(position) => position, // 跟随模式使用上方一次查询得到的稳定位置快照。
            None => transform.translation - Vec3::Y * tag.y_offset, // 固定模式减去高度偏移还原锚点。
        };
        let target_pos = Vec3::new(anchor_pos.x, anchor_pos.y + tag.y_offset, anchor_pos.z);
        let Some(cam_tf) = camera else {
            // 相机查询异常时仍保留前面的 TTL/宿主清理，只隐藏本帧显示，避免临时文字资源泄漏。
            if transform.translation != target_pos {
                transform.translation = target_pos;
            }
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        let dist = cam_tf.translation.distance(target_pos);
        if dist > tag.max_distance {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden; // 超出可见距离
            }
            continue;
        }
        if *visibility != Visibility::Visible {
            *visibility = Visibility::Visible;
        }
        let dist_scale = if tag.distance_scaling {
            (dist / tag.ref_distance).min(1.0)
        } else {
            1.0
        };
        let target_scale = Vec3::splat(dist_scale * tag.scale);
        if transform.translation != target_pos {
            transform.translation = target_pos;
        }
        if transform.scale != target_scale {
            transform.scale = target_scale; // 距离缩放 × 用户倍率
        }
        if transform.rotation != cam_tf.rotation {
            transform.rotation = cam_tf.rotation; // 朝向相机
        }
    }
}

/// 内部辅助：销毁一个 Billboard 实体及其当前烘焙任务的2D源实体。
fn despawn_billboard(
    commands: &mut Commands,
    asset_stores: &mut BillboardAssetStores,
    billboard: Entity,
    tag: &BillboardTextLink,
) {
    // TTL、宿主消失、外部重建可能同帧清理同一组实体；try_despawn 保持幂等并避免无效 Entity 警告。
    if let Some(text_entity) = tag.text_entity {
        commands.entity(text_entity).try_despawn(); // 固定相机归池由烘焙系统负责，此处只清理当前任务文字源。
    }
    for outline in tag.outline_entities.iter() {
        commands.entity(*outline).try_despawn(); // 一并销毁文字描边实体
    }
    for deco in tag.decoration_entities.iter() {
        commands.entity(*deco).try_despawn(); // 一并销毁底盘/描边圈
    }
    commands.entity(tag.temporal_guide_entity).try_despawn(); // 显式清理子实体，使动态材质释放与实体生命周期同一边界完成。
    commands.entity(billboard).try_despawn();
    // Billboard 的 mesh/material/image 都是按实例动态创建的；实体销毁时同步移除，避免 F3 资产计数越跑越高。
    asset_stores.release(tag); // 资产释放细节由聚合参数单点维护。
}

#[cfg(test)]
mod tests {
    use bevy::core_pipeline::prepass::{DeferredPrepass, MotionVectorPrepass};

    use super::*;

    #[test]
    fn temporal_guide_camera_requires_complete_active_chain() {
        let mut camera = Camera::default();
        assert!(temporal_guide_camera_is_eligible(&camera, true, true, true));
        assert!(!temporal_guide_camera_is_eligible(
            &camera, false, true, true
        ));
        assert!(!temporal_guide_camera_is_eligible(
            &camera, true, false, true
        ));
        assert!(!temporal_guide_camera_is_eligible(
            &camera, true, true, false
        ));
        camera.is_active = false;
        assert!(!temporal_guide_camera_is_eligible(
            &camera, true, true, true
        ));
    }

    #[test]
    fn temporal_guide_layer_tracks_chain_and_restores_camera_layers() {
        let mut app = App::new();
        app.init_resource::<BillboardTemporalGuideState>()
            .add_systems(Update, sync_billboard_temporal_guides);
        let camera = app
            .world_mut()
            .spawn((
                Camera::default(),
                Camera3d::default(),
                BillboardCamera,
                DeferredPrepass,
                MotionVectorPrepass,
                TemporalJitter::default(),
                RenderLayers::layer(0),
            ))
            .id();
        let guide = app
            .world_mut()
            .spawn((BillboardTemporalGuide, Visibility::Hidden))
            .id();

        app.update();
        let layers = app.world().get::<RenderLayers>(camera).unwrap();
        assert!(render_layers_contains(
            layers,
            BILLBOARD_TEMPORAL_GUIDE_LAYER
        ));
        assert_eq!(
            app.world().get::<Visibility>(guide),
            Some(&Visibility::Inherited)
        );

        app.world_mut()
            .entity_mut(camera)
            .insert(crate::components::BillboardTemporalGuideSuppressed); // 模拟本帧把文字交给独立界面合成。
        app.update(); // 正式引导系统必须同帧撤销引导可见性与相机层租用。
        assert_eq!(
            app.world().get::<Visibility>(guide),
            Some(&Visibility::Hidden)
        ); // UI 剥离时深度/motion 中不得残留文字引导。
        assert!(!render_layers_contains(
            app.world().get::<RenderLayers>(camera).unwrap(),
            BILLBOARD_TEMPORAL_GUIDE_LAYER
        )); // 原相机不再看到专用引导层。
        app.world_mut()
            .entity_mut(camera)
            .remove::<crate::components::BillboardTemporalGuideSuppressed>(); // 模拟关闭帧生成并归还普通时间重建行为。
        app.update(); // 不重建文字资产即可恢复原引导。
        assert_eq!(
            app.world().get::<Visibility>(guide),
            Some(&Visibility::Inherited)
        ); // 原字形引导必须重新可见。
        assert!(render_layers_contains(
            app.world().get::<RenderLayers>(camera).unwrap(),
            BILLBOARD_TEMPORAL_GUIDE_LAYER
        )); // 还原原时间链相机层。
        app.world_mut()
            .entity_mut(camera)
            .remove::<TemporalJitter>();
        app.update();
        let layers = app.world().get::<RenderLayers>(camera).unwrap();
        assert!(!render_layers_contains(
            layers,
            BILLBOARD_TEMPORAL_GUIDE_LAYER
        ));
        assert_eq!(
            app.world().get::<Visibility>(guide),
            Some(&Visibility::Hidden)
        );
    }

    #[test]
    fn billboard_cleanup_releases_temporal_guide_entity_and_assets() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<BillboardColorMaterial>>()
            .init_resource::<Assets<BillboardTemporalGuideMaterial>>()
            .init_resource::<Assets<ColorMaterial>>()
            .init_resource::<BillboardBakePool>()
            .add_systems(
                Update,
                (spawn_billboard_texts, update_billboard_texts).chain(),
            );
        let owner = app
            .world_mut()
            .spawn((Transform::default(), BillboardText::default()))
            .id();

        app.update();
        let billboard = app
            .world()
            .get::<BillboardTextHandle>(owner)
            .expect("请求实体应取得最终Billboard句柄")
            .0;
        let (guide, image, mesh, color_material, guide_material) = {
            let link = app
                .world()
                .get::<BillboardTextLink>(billboard)
                .expect("最终Billboard应保存完整资产所有权");
            (
                link.temporal_guide_entity,
                link.image.clone(),
                link.mesh.clone(),
                link.material.clone(),
                link.temporal_guide_material.clone(),
            )
        };
        assert_eq!(
            app.world()
                .get::<ChildOf>(guide)
                .expect("时间引导必须作为最终文字子实体生成")
                .parent(),
            billboard
        );
        assert!(
            app.world()
                .resource::<Assets<BillboardTemporalGuideMaterial>>()
                .get(&guide_material)
                .is_some()
        );

        app.world_mut().entity_mut(owner).despawn();
        app.update();

        assert!(app.world().get_entity(billboard).is_err());
        assert!(app.world().get_entity(guide).is_err());
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&image)
                .is_none()
        );
        assert!(app.world().resource::<Assets<Mesh>>().get(&mesh).is_none());
        assert!(
            app.world()
                .resource::<Assets<BillboardColorMaterial>>()
                .get(&color_material)
                .is_none()
        );
        assert!(
            app.world()
                .resource::<Assets<BillboardTemporalGuideMaterial>>()
                .get(&guide_material)
                .is_none()
        );
    }
}
