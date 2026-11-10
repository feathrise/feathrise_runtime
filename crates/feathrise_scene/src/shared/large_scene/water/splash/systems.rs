//! 固定48槽真实近景水花系统与启动资源准备。
use super::super::{
    components::DynamicWaterSurface,
    footprint::{WaterInteractionFootprint, water_interaction_sample},
    material::DynamicWaterMaterial,
};
use avian3d::prelude::{SpatialQuery, SpatialQueryFilter}; // 复用物理空间树排除已有屋顶碰撞。
use bevy::{light::NotShadowCaster, math::Vec3Swizzles, prelude::*}; // 小水滴不投影，保留昼夜PBR照明。
use feathrise_scene_core::view::PrimarySceneView; // 只允许主玩法相机的真实近景。
use crate::SceneRuntimeObserver; // 围绕唯一玩家选取近景候选。
use feathrise_environment::terrain_surface::{
    SurfacePuddleMask, SurfaceWeatherGpu, TerrainSurfaceMaterial, surface_effects_quality_profile,
}; // 复用当前完整地表数据。
use feathrise_scene_content::ActiveDynamicScene; // 动态场景同位置切换仍有唯一SceneId。
use feathrise_scene_core::state::ScenePhase; // 正式场景身份排除菜单和加载阶段。 // 不复制真实水面足迹。

use crate::components::DynamicSparseTerrainCollider; // 新 Clipmap 的独立碰撞实体仍可授权真实地面积水。
use crate::scenes::dynamic_scene::{ClipmapWaterMaterial, ClipmapWaterSurface}; // 静水水花按同代R8与真实三角水深授权。

use super::{
    components::SplashDrop, constants::*, helpers::*, resources::SplashWaterTarget,
    types::SplashClock,
}; // 分离数据与逻辑。
/// 在既有遍历选择的真实脚下水体上发射；没有水体时可由真实地形碰撞与积水核心授权。
#[allow(clippy::too_many_arguments)] // 固定池需要明确读取天气、物理、地表资产与水体身份。
pub(in super::super) fn update_rain_splashes(
    surface_weather: (
        Res<SurfaceWeatherGpu>,
        Option<Res<crate::ScenePhysicsPolicy>>,
        Option<Res<feathrise_environment::rain_shelter::RainShelterState>>,
        feathrise_grounding::ActiveGroundColliders,
        Query<&GlobalTransform, Without<SplashDrop>>,
    ), // 天气与遮雨合并为一个SystemParam，保持Bevy参数数量上限。
    mask: (
        Res<SurfacePuddleMask>,
        Option<Res<feathrise_scene_core::residency::InstanceResourceTable<SurfacePuddleMask>>>,
        Option<Res<feathrise_scene_core::residency::InstanceResourceTable<crate::shared::surface_weather::PuddleRuntimeState>>>,
    ), // 按实际命中来源取完整遮罩，不能采样父图或另一实例。
    images: Res<Assets<Image>>,     // 每候选最多读取四字节。
    target: Res<SplashWaterTarget>, // 由既有遍历选取，不扫描水体集合。
    water: Query<
        (
            &GlobalTransform,
            &DynamicWaterSurface,
            &MeshMaterial3d<DynamicWaterMaterial>,
        ),
        Without<SplashDrop>,
    >, // 单实体定向访问且与水滴写侧互斥。
    water_materials: Res<Assets<DynamicWaterMaterial>>, // 四组宏观波和八槽玩家波的实际GPU参数。
    clipmap_water: (
        Query<
            (
                &ClipmapWaterSurface,
                &DynamicWaterSurface,
                &MeshMaterial3d<ClipmapWaterMaterial>,
            ),
            Without<SplashDrop>,
        >,
        Option<Res<Assets<ClipmapWaterMaterial>>>,
    ), // 与GLB目标互斥，仍按既有单一脚下实体做O(1)定向查询。
    time: Res<Time>,                // GPU globals使用同一个虚拟时间并按一小时回绕。
    players: Query<(Entity, &GlobalTransform), (With<SceneRuntimeObserver>, Without<SplashDrop>)>, // 当前玩家世界位置与重生身份。
    cameras: Query<&GlobalTransform, (With<PrimarySceneView>, Without<SplashDrop>)>, // 排除水滴写侧并读取唯一玩法相机。
    terrain: Query<
        (),
        Or<(
            With<MeshMaterial3d<TerrainSurfaceMaterial>>,
            With<DynamicSparseTerrainCollider>,
        )>,
    >, // 旧同实体地表和新独立稀疏 Collider 都可授权积水。
    spatial: SpatialQuery, // 沿用已建成的Avian空间索引。
    mut drops: Query<
        (
            &mut SplashDrop,
            &mut Transform,
            &mut GlobalTransform,
            &mut Visibility,
        ),
        Without<SceneRuntimeObserver>,
    >, // 固定48槽。
    mut clock: Local<SplashClock>, // 节流与候选序号。
    scene: Option<Res<State<ScenePhase>>>, // 无正式玩法身份的独立测试或工具不发射。
    active_scene: Option<Res<ActiveDynamicScene>>, // 正式动态场景身份。
) {
    // 开始固定预算水花帧。
    let (weather, physics, shelter, active_sources, source_frames) = surface_weather;
    let (mask, child_masks, child_puddles) = mask; // 天气仍由父图统一，遮罩和范围由实际来源决定。
    let now = weather.weather.w; // 暂停时共享秒不动。
    let delta = elapsed_delta(clock.previous, now); // 存活水滴消费完整暂停感知差，发射单独限制一批。
    clock.previous = Some(now); // 建立下帧基线。
    let player_entity = players.single().ok().map(|(entity, _)| entity); // 同位置重生仍是新身份。
    let player = players
        .single()
        .ok()
        .map(|(_, transform)| transform.translation()); // 无玩家即无近景。
    let camera = cameras.single().ok().map(GlobalTransform::translation); // 辅助/无相机场景不产生水花。
    let mask_identity = mask.image.as_ref().map(Handle::id); // 场景私有遮罩与完整版本身份。
    let current_phase = scene.as_ref().map(|phase| *phase.get()); // 小型枚举身份。
    let current_scene_id = active_scene.as_ref().and_then(|scene| scene.scene_id.as_ref()); // 借用SceneId避免每帧分配。
    let identity_changed =
        clock.previous_phase != current_phase || clock.previous_scene_id.as_ref() != current_scene_id; // 同坐标切换场景也清池。
    if identity_changed {
        clock.previous_phase = current_phase;
        clock.previous_scene_id = current_scene_id.cloned();
    } // 只在身份变化时复制小字符串。
    let reset = identity_changed
        || clock.previous_mask != mask_identity
        || clock.previous_entity != player_entity
        || clock.previous_target != target.0
        || clock.previous_bounds != weather.bounds
        || player
            .zip(clock.previous_player)
            .is_some_and(|(new, old)| new.distance_squared(old) > 64.0); // 切换场景、足迹换代或传送清池。
    clock.previous_mask = mask_identity; // 同坐标同范围切换场景也不能留下旧图水花。
    clock.previous_entity = player_entity; // 玩家同位置重生也清除旧滴。
    clock.previous_player = player;
    clock.previous_target = target.0;
    clock.previous_bounds = weather.bounds; // 记录同帧身份。
    let gameplay = current_phase.is_some_and(|phase| ScenePhase::active_phases().contains(&phase)); // 菜单或加载中明确不发射。
    let profile = surface_effects_quality_profile(weather.wind_quality.w); // CPU与Shader采用同一质量契约。
    let distance_squared = profile.splash_distance * profile.splash_distance; // 真实相机裁剪跟随档位。
    let enabled = gameplay
        && player.is_some()
        && camera.is_some()
        && weather.weather.z > 0.05
        && profile.splash_capacity > 0; // 每滴实际距离裁剪，避免玩家锚点造成整个池突变。
    let mut retained = 0; // 单次遍历裁剪降档后的存活预算，暂停同样生效。
    for (mut drop, mut transform, mut global, mut visibility) in &mut drops {
        // 只访问固定池，不随场景规模增长。
        if drop.active
            && shelter
                .as_ref()
                .is_some_and(|shelter| shelter.is_changed() && shelter.exposure(drop.origin) < 0.5)
        {
            drop.active = false;
        } // 暂停移动屋檐也撤销其下旧滴，稳定代理不重复测试存活池。
        if !enabled
            || reset
            || retained >= profile.splash_capacity
            || camera.is_none_or(|camera| {
                camera.distance_squared(transform.translation) > distance_squared
            })
        {
            drop.active = false;
        } // 晴天、离场、切换场景或降档立即清除旧水花。
        if drop.active {
            // 已发射水滴按暂停感知秒推进。
            drop.age += delta; // 寿命无天气历史污染。
            drop.active = drop.age < LIFE; // 固定寿命上界。
            transform.translation =
                drop.origin + drop.velocity * drop.age - Vec3::Y * (4.9 * drop.age * drop.age); // 真实重力抛物线。
            *global = GlobalTransform::from(*transform); // PostUpdate传播已完成，显式更新同帧全球位置。
        } // 完成单槽更新。
        if drop.active {
            retained += 1;
        } // 只统计寿命与距离检查后仍有效的水滴。
        visibility.set_if_neq(if drop.active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        }); // 空槽稳定帧不制造显隐脏标记。
    } // 完成固定槽更新。
    if !enabled || delta <= 0.0 {
        clock.accumulator = 0.0;
        return;
    } // 暂停不发射也不积债。
    clock.accumulator += delta; // 高档10Hz、极高20Hz，使用同一正间隔预算。
    if clock.accumulator < profile.splash_interval {
        return;
    } // 其余帧仅更新存活池。
    clock.accumulator = 0.0; // 每帧最多一批，不补发长帧欠账。
    let center = player.unwrap(); // enabled已确认玩家存在。
    let filter = SpatialQueryFilter::from_mask(physics.as_deref().copied().unwrap_or_default().surface_query_mask); // 近景积水只消费应用提供的查询掩码。
    let mut available = drops.iter_mut().filter(|(drop, _, _, _)| !drop.active); // 空槽迭代器只走一遍，避免每候选重复扫描池。
    for _ in 0..profile
        .splash_candidates
        .min(profile.splash_capacity.saturating_sub(retained))
    {
        // 每批受候选数与剩余容量双重限制。
        clock.sequence = clock.sequence.wrapping_add(1); // 每次候选拥有稳定独立序号。
        let seed = clock.sequence; // 水花使用独立稀疏视觉采样，不宣称与Shader每滴同点。
        if random(seed ^ 773) > weather.weather.z {
            continue;
        } // 雨量连续控制候选接纳。
        let point = center.xz() + Vec2::new(random(seed) - 0.5, random(seed ^ 991) - 0.5) * 10.0; // 玩家周围10m局部区域。
        let mut origin = None; // 未证明表面时绝不发射。
        if let Some((transform, surface, material)) =
            target.0.filter(|entity| active_sources.is_active(*entity)).and_then(|entity| water.get(entity).ok())
        {
            // 只读取当前脚下水体。
            let local = transform
                .affine()
                .inverse()
                .transform_point3(Vec3::new(point.x, center.y, point.y)); // 换算真实水面局部坐标。
            if water_interaction_sample(
                &surface.interaction_footprint,
                surface.rest_water_level,
                local.xz(),
            ).is_some() {
                // 可靠足迹查询。
                if let Some(material) = water_materials.get(&material.0) {
                    // 必须取得实际当前波形参数。
                    let frame = &material.extension.uniform.current; // 与GPU主通道同源。
                    let elapsed = (time.elapsed_secs_f64() % 3600.0) as f32; // 与Bevy globals小时回绕一致。
                    let animated = match &surface.interaction_footprint {
                        // 各类足迹只消费已完成数据。
                        WaterInteractionFootprint::Painted(footprint) => footprint
                            .sample_displaced_position(
                                local.xz(),
                                surface.rest_water_level,
                                |position, alpha| {
                                    position
                                        + still_displacement(frame, elapsed, position.xz(), alpha)
                                },
                            ), // 三个真实顶点分别变形再插值，避免稀疏片面悬空。
                        WaterInteractionFootprint::Ribbon(footprint) => {
                            super::super::footprint::ribbon_splash_position(
                                footprint,
                                frame,
                                elapsed,
                                local.xz(),
                                |point| player_height(frame, point),
                            )
                        } // 同源实际三角逐顶点波高再插值，保留坡河与玩家事件波。
                        WaterInteractionFootprint::Clipmap { .. } => None, // Clipmap实体使用另一具体材质查询，不会进入此分支。
                        WaterInteractionFootprint::Disabled => None, // 没有互动几何不能伪造落点。
                    }; // 完成类型分支。
                    origin = animated.map(|point| transform.transform_point(point)); // 波面最终点转换到世界。
                } // 缺材质保持无水花。
            } // 岸外自动不发射。
        } else if let Some((surface, interaction, material)) =
            target.0.filter(|entity| active_sources.is_active(*entity)).and_then(|entity| clipmap_water.0.get(entity).ok())
        {
            if let Some(materials) = clipmap_water.1.as_ref()
                && let Some(material) = materials.get(&material.0)
            {
                let source_frame = material.extension.source_frame();
                let local = source_frame.inverse().transform_point3(Vec3::new(point.x, center.y, point.y));
                if let Some(sample) = water_interaction_sample(
                &interaction.interaction_footprint,
                interaction.rest_water_level,
                local.xz(),
                ) {
                let frame = &material.base.extension.uniform.current;
                let elapsed = (time.elapsed_secs_f64() % 3600.0) as f32;
                let alpha = surface.coverage_at(local.xz());
                let rest = Vec3::new(local.x, sample.surface_local_y, local.z);
                origin = Some(source_frame.transform_point3(rest + still_displacement(frame, elapsed, local.xz(), alpha)));
                } // 波浪、岸线与遮罩都先在源坐标求值，再一次变换到世界。
            } // R8空洞、浅水、缺页或跨高度修订均不产生离水水花。
        } // 完成单水体候选。
        if origin.is_none() && (weather.options.x > 0.5 || child_masks.is_some()) {
            // 积水只认当前完整启用遮罩。
            if let Some(hit) = spatial.cast_ray_predicate(
                Vec3::new(point.x, center.y + 8.0, point.y),
                Dir3::NEG_Y,
                16.0,
                true,
                &filter,
                &|entity| active_sources.is_active(entity),
            ) {
                // 固定近景垂直范围。
                if terrain.contains(hit.entity) && hit.normal.y >= 0.997 {
                    // 屋顶或普通建筑绝不冒充地形。
                    let sample = match active_sources.owner(hit.entity) {
                        Some(Some(owner)) => child_masks.as_ref().and_then(|table| table.values.get(&owner))
                            .zip(child_puddles.as_ref().and_then(|table| table.values.get(&owner)))
                            .zip(source_frames.get(owner).ok())
                            .map(|((mask, state), frame)| (mask, state.world_bounds(), frame.affine().inverse().transform_point3(Vec3::new(point.x, center.y, point.y)).xz())),
                        Some(None) if weather.options.x > 0.5 => Some((&*mask, weather.bounds, point)),
                        _ => None,
                    }; // 缺少源图完整遮罩时拒绝，不穿透采样父图。
                    if let Some(potential) = sample.and_then(|(mask, bounds, point)| mask.image.as_ref()
                        .and_then(|handle| images.get(handle))
                        .and_then(|image| mask_potential(image, bounds, point)))
                    {
                        // 不持有旧场景像素。
                        if puddle_core(potential, weather.weather.y, hit.normal.y) {
                            origin =
                                Some(Vec3::new(point.x, center.y + 8.0 - hit.distance, point.y));
                        } // 最坏噪声下仍完整覆盖才授权。
                    } // 完成当前遮罩采样。
                } // 完成地形成员检查。
            } // 完成地表物理查询。
        } // 完成积水候选。
        let Some(origin) = origin else {
            continue;
        }; // 坏图、岸外与无碰撞地形安全跳过。
        if shelter
            .as_ref()
            .is_some_and(|shelter| shelter.exposure(origin) < 0.5)
        {
            continue;
        } // 无物理碰撞的显式屋檐也拒绝发射。
        if camera.is_none_or(|camera| camera.distance_squared(origin) > distance_squared) {
            continue;
        } // 每个落点受当前档位真实相机距离裁剪。
        if spatial
            .cast_ray_predicate(origin + Vec3::Y * 0.02, Dir3::Y, 128.0, true, &filter, &|entity| active_sources.is_active(entity))
            .is_some()
        {
            continue;
        } // 已登记屋顶/桥面阻止水花。
        let Some((mut drop, mut transform, mut global, mut visibility)) = available.next() else {
            break;
        }; // 池满丢弃，不扩容。
        let angle = random(seed ^ 113) * std::f32::consts::TAU; // 不重复固定飞溅方向。
        drop.origin = origin + Vec3::Y * 0.012; // 抬离表面避免深度闪烁。
        drop.velocity = Vec3::new(
            angle.cos() * 0.22,
            1.25 + random(seed ^ 431) * 0.25,
            angle.sin() * 0.22,
        ) + splash_wind_velocity(weather.wind_quality); // 约8至11cm冠状弹道，风仅改变新滴水平初速。
        drop.age = 0.0;
        drop.active = true; // 重新占用空槽。
        transform.translation = drop.origin;
        *global = GlobalTransform::from(*transform);
        *visibility = Visibility::Visible; // 当前帧立即发布完整槽状态。
    } // 完成四候选固定预算。
} // 结束近景水花系统。

pub(in super::super) fn initialize_rain_splashes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Startup仅构建一份固定小网格，运行帧零资产构建。
    let mesh = meshes.add(Sphere::new(0.006).mesh().uv(6, 4)); // 所有水滴共用低面数6mm小球。
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.48, 0.56, 0.60),
        perceptual_roughness: 0.25,
        ..default()
    }); // 昼夜由正常光照驱动。
    for _ in 0..CAPACITY {
        // 一次建立硬上限池。
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            Visibility::Hidden,
            NotShadowCaster,
            SplashDrop {
                origin: Vec3::ZERO,
                velocity: Vec3::ZERO,
                age: LIFE,
                active: false,
            },
        )); // 小滴共享网格材质，空槽隐藏。
    } // 完成池安装。
} // 结束启动阶段资源准备。
