//! 正式大地图GLB水面的GPU材质初始化与固定参数同步系统。
//! 【功能设计】初始化只克隆一次静止网格补齐顶点颜色；逐帧不访问Assets<Mesh>，只维护玩家接触并上传八槽统一事件波。

use bevy::{
    prelude::*,             // Observer、Query、Assets、Mesh与变换等ECS类型。
}; // 完成Bevy类型导入。
use crate::{SceneRuntimeObserver, WaterInteractionSource}; // 查询唯一玩家的世界位置与Tnua实际起跳序号。

use crate::maps::dynamic_map::{ClipmapWaterMaterial, ClipmapWaterSurface}; // Clipmap静水共用玩家八槽状态，但使用专用高度/R8材质。

use super::components::{DynamicWaterCpuInteraction, DynamicWaterSurface}; // CPU查询只包含可互动水面，纯视觉整网格水由GPU全局时间驱动。
use super::material::{
    DynamicWaterMaterial,          // 查询并更新具体GPU水面扩展材质资产。
    update_dynamic_water_material, // 每帧只推进固定384字节当前与历史参数。
}; // 完成GPU水面材质入口导入。
use super::ripple::update_player_ripples; // CPU只推进接触状态与八槽统一玩家事件波。

/// 推进玩家接触波源，并把固定时间与八槽统一事件波同步到GPU材质。
pub(super) fn update_dynamic_water_surfaces(
    time: Res<Time>, // 当前时间驱动背景波，帧长推进玩家波源寿命和接触速度。
    player: Query<
        (&GlobalTransform, &WaterInteractionSource), // 同时读取玩家最新世界位置与FixedUpdate发布的实际起跳身份。
        With<SceneRuntimeObserver>,                     // 只匹配唯一的本地玩家根实体。
    >,
    mut water_surfaces: Query<
        (
            Entity,                                // 近景水花定向读取脚下水体，不增加第二次全水体遍历。
            &GlobalTransform,                      // 把玩家世界脚底转换到实际水面子网格局部空间。
            &MeshMaterial3d<DynamicWaterMaterial>, // 定位当前水面独占的固定参数材质资产。
            &mut DynamicWaterSurface, // 只读写岸线、接触状态、八槽统一波源和已消费起跳身份。
            Option<&InheritedVisibility>, // 隐藏的旧预览不能授权当前相机水下外观。
            Option<&Visibility>,
        ),
        (
            With<DynamicWaterCpuInteraction>,
            Without<ClipmapWaterSurface>,
        ), // 纯视觉整网格水/瀑布不进入逐帧循环，Clipmap由世界坐标专用查询处理。
    >,
    mut clipmap_water_surfaces: Query<
        (
            Entity,
            &ClipmapWaterSurface,
            &MeshMaterial3d<ClipmapWaterMaterial>,
            &Visibility,
            Option<&InheritedVisibility>, // 作者隐藏地形父根时，水实体自身仍为Inherited，不能继续授权水下外观。
            &mut DynamicWaterSurface,
        ),
        With<DynamicWaterCpuInteraction>,
    >,
    mut materials: ResMut<Assets<DynamicWaterMaterial>>, // 每张水面每帧只覆盖固定384字节材质参数。
    mut clipmap_materials: Option<ResMut<Assets<ClipmapWaterMaterial>>>, // 不安装动态地图插件的独立GLB场景可缺失。
    mut splash_target: ResMut<super::splash::SplashWaterTarget>, // 当前帧唯一脚下互动水体身份。
    rain_mask_cache: Option<Res<super::rain_mask::RainMaskCache>>, // 复用当前水材质遍历定位已存在高度图材质。
    mut rain_mask_materials: Option<ResMut<Assets<super::rain_mask::RainHeightMaterial>>>, // 独立水测试可缺失，正式插件初始化一次。
    active_sources: feathrise_grounding::ActiveGroundColliders,
    mut underwater: Option<ResMut<super::underwater::UnderwaterViewState>>, // 复用本遍历，避免为相机再扫描所有水面。
) {
    splash_target.0 = None; // 切图或玩家离水必须撤销旧水体身份。
    let elapsed_seconds = time.elapsed_secs(); // 当前连续时间作为背景缓波相位输入。
    let delta_seconds = time.delta_secs(); // 本帧时长用于波源老化和玩家速度估算。
    let player_world_state = player
        .single()
        .ok()
        .map(|(player_transform, takeoff_sequence)| {
            (
                player_transform.translation() - Vec3::Y * takeoff_sequence.foot_offset, // 在世界空间减去角色悬浮高度得到稳定脚底坐标。
                takeoff_sequence.takeoff_sequence, // 复制Tnua实际动作序号，让每张水面独立判断是否出现新起跳。
            ) // 返回同一玩家帧快照中的世界脚底与起跳身份。
        }); // 玩家切场景尚未生成时保留None并只播放GPU背景波。

    for (entity, water_global_transform, material_handle, mut water_surface, inherited, visibility) in &mut water_surfaces
    {
        if !active_sources.is_active(entity) { continue; } // 隐藏候选整批无权授权波纹与近景水花。
        // 当前地图可能有多张水面，每张独立维护局部岸线、八槽事件波和起跳消费身份。
        let player_local_foot_position = player_world_state.map(|(world_foot_position, _)| {
            // 使用水面子实体完整世界逆变换，兼容地图根平移、旋转和缩放。
            water_global_transform
                .affine()
                .inverse()
                .transform_point3(world_foot_position) // 把玩家世界脚底转换为波源局部坐标。
        }); // 完成可选玩家局部脚底换算。
        if player_local_foot_position.is_some_and(|point| {
            super::footprint::water_interaction_sample(
                &water_surface.interaction_footprint,
                water_surface.rest_water_level,
                Vec2::new(point.x, point.z),
            )
            .is_some_and(|sample| (point.y - sample.surface_local_y).abs() < 3.0)
        }) {
            // 只有真实脚下或贴近水面可授权近景发射。
            splash_target.0 = Some(entity); // 复用已存在的互动遍历，不构造整图索引。
        } // 完成近景水体选择。
        let _emitted_ripple = update_player_ripples(
            &mut water_surface,         // 推进当前水面的固定容量波源与接触状态。
            player_local_foot_position, // 玩家不存在时只让已有尾波继续衰减。
            player_world_state.map(|(_, takeoff_sequence)| takeoff_sequence), // 传入不会被渲染帧漏掉的Tnua实际起跳身份。
            delta_seconds, // 使用本帧时间估算移动与竖直速度。
        ); // 入水、走动与跳跃只写小型波源池，不生成实体或修改Mesh。

        let Some(mut material) = materials.get_mut(&material_handle.0) else {
            // 热重载或资产替换间隙可能暂时取不到扩展材质。
            continue; // 保留上一帧GPU参数并等待下一帧恢复。
        }; // 获得当前水面独占GPU材质。
        if inherited.is_none_or(|v| v.get()) && visibility.is_none_or(|v| *v != Visibility::Hidden)
            && let Some(view) = underwater.as_deref_mut() {
            view.observe_surface(&water_surface, water_global_transform.to_matrix(), material.base.base_color);
        }
        update_dynamic_water_material(
            &mut material,                 // 显式借用Bevy 0.19的AssetMut包装供纯材质helper更新。
            elapsed_seconds,               // 上传一项连续时间而不是CPU逐顶点背景波。
            &water_surface.player_ripples, // 上传最多八个局部中心、年龄和振幅。
            water_surface.flow_direction,  // 湖面流向与当前/历史波形同帧推进。
            water_surface.flow_speed,      // 湖面流速与当前/历史波形同帧推进。
        ); // 完成本帧固定成本GPU水面参数同步。
        if let (Some(cache), Some(captures)) = (
            rain_mask_cache.as_deref(),
            rain_mask_materials.as_deref_mut(),
        ) {
            super::rain_mask::sync_source_uniform(
                material_handle.0.id(),
                &material,
                cache,
                captures,
            );
        } // 同一循环顺手同步高度图640字节，玩家波不滞后一帧也不另扫全水体。
    } // 完成全部正式地图水面更新。

    let Some(clipmap_materials) = clipmap_materials.as_deref_mut() else {
        return;
    }; // 共享水插件可独立用于旧GLB/HN场景，不能强制要求Clipmap材质插件。
    for (entity, surface, material_handle, visibility, inherited, mut water_surface) in
        &mut clipmap_water_surfaces
    {
        if !active_sources.is_active(entity) { continue; }
        let Some(source_frame) = clipmap_materials.get(&material_handle.0).map(|material| material.extension.source_frame()) else { continue; };
        if !surface.is_published() {
            continue;
        } // 隐藏的逐层候选和退役层没有玩家互动权限。
        let player_world_foot = (*visibility != Visibility::Hidden)
            .then_some(player_world_state)
            .flatten()
            .map(|(position, _)| source_frame.inverse().transform_point3(position)); // 波源使用源图坐标，只逆变换实例根，不能套移动环带的局部矩阵。
        if player_world_foot.is_some_and(|point| {
            super::footprint::water_interaction_sample(
                &water_surface.interaction_footprint,
                water_surface.rest_water_level,
                point.xz(),
            )
            .is_some_and(|sample| (point.y - sample.surface_local_y).abs() < 3.0)
        }) {
            splash_target.0 = Some(entity);
        }
        let _emitted_ripple = update_player_ripples(
            &mut water_surface,
            player_world_foot,
            player_world_state.map(|(_, takeoff_sequence)| takeoff_sequence),
            delta_seconds,
        );
        let Some(mut material) = clipmap_materials.get_mut(&material_handle.0) else {
            continue;
        };
        if *visibility != Visibility::Hidden && inherited.is_none_or(|v| v.get())
            && let Some(view) = underwater.as_deref_mut() {
            view.observe_surface(&water_surface, source_frame, material.base.base.base_color);
        }
        update_dynamic_water_material(
            &mut material.base,
            elapsed_seconds,
            &water_surface.player_ripples,
            water_surface.flow_direction,
            water_surface.flow_speed,
        ); // 运动历史和当前波形以稳定世界位置计算，环带吸附不会重置或整体闪烁。
    }
} // 结束GPU水面固定参数同步系统。
