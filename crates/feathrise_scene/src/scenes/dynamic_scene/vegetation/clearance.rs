//! 动态 Blade 植被的建筑净空派生与原子密度句柄替换。
//!
//! 【功能设计】每次都从当前植被修订冻结的原始 R8 基础级复制，后台按建筑四边形
//! 保守清零相交纹素格；主线程只登记完成图片并一次替换当前根的全部四级窗口。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use std::collections::BTreeMap; // 正式层身份确定性映射到唯一派生Image。
use std::sync::Arc; // 原始R8与建筑四边形只浅克隆进single-flight任务。

use bevy::asset::RenderAssetUsages; // 临时密度需同时保留主世界诊断和渲染上传。
use bevy::image::ImageSampler; // 数值密度继续使用与正式KTX2一致的最近采样。
use bevy::prelude::*; // ECS、Image资产、Vec2与Commands。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 单Mip线性R8图片契约。
use bevy::tasks::{AsyncComputeTaskPool, futures::check_ready}; // CPU栅格化离开帧关键路径，主线程只非阻塞轮询。
use feathrise_environment::vegetation::VegetationBladeLayer; // 四个LOD窗口通过正式层ID成组替换。
use feathrise_environment::vegetation::blade::{GpuBladeDensityMap, GpuBladeSourceRevision}; // 密度与真实作者发布版本同边界交给GPU。
use feathrise_scene_content::{BuildingFootprintRect, BuildingWorldQuad}; // 纹素世界格与建筑净空使用同一凸四边形公式。

use super::components::DynamicBladeWindow; // 只修改动态场景当前Blade窗口。
use super::resources::{
    DynamicBladeClearanceBuild, DynamicBladeClearanceLayerBuild, DynamicBladeLayerBuild,
    DynamicVegetationPhase, DynamicVegetationState, DynamicVegetationStatus,
}; // single-flight身份和完整完成值都归属唯一动态植被事务。
use crate::BuildingVegetationClearanceState; // 正式placement与F12未保存草稿共用来源。

/// 后台遮罩任务的纯值输入；大数组全部通过Arc共享。
struct DynamicBladeClearanceJob {
    generation: u32,                      // 当前动态场景代次。
    revision: u32,                        // 当前植被资源修订。
    clearance_revision: u64,              // 当前建筑净空修订。
    layers: Vec<DynamicBladeLayerBuild>,  // 每层原始R8只浅克隆Arc。
    clearances: Arc<[BuildingWorldQuad]>, // 完整建筑净空数组。
    world_min: [f32; 2],                  // 密度纹理世界最小X/Z。
    world_max: [f32; 2],                  // 密度纹理世界最大X/Z。
    draft: Option<Arc<super::DynamicBladeDraftSnapshot>>, // 共享最新完整作者R8，避免旧根缓存覆盖刚刷的新草。
    draft_revision: Option<u64>, // 同一布局允许完整中间密度采用，下一任务继续追最新修订。
}

/// 判断两个闭凸四边形是否具有面积或边界交集。
fn quads_intersect(left: BuildingWorldQuad, right: BuildingWorldQuad) -> bool {
    left.overlaps(right)
        || left.corners.iter().any(|point| right.contains(*point))
        || right.corners.iter().any(|point| left.contains(*point)) // SAT接触边界也按净空处理。
}

/// 从原始R8复制并保守清零与任一建筑净空相交的最近采样纹素格。
fn mask_blade_density(
    source: &[u8],
    resolution: [u32; 2],
    world_min: [f32; 2],
    world_max: [f32; 2],
    clearances: &[BuildingWorldQuad],
) -> Result<Vec<u8>, String> {
    let expected = resolution[0] as usize * resolution[1] as usize;
    if resolution[0] == 0 || resolution[1] == 0 || source.len() != expected {
        return Err("Blade建筑净空输入的R8尺寸无效".to_owned()); // 坏源不能生成错位临时图片。
    }
    let minimum = Vec2::from_array(world_min);
    let maximum = Vec2::from_array(world_max);
    if !minimum.is_finite() || !maximum.is_finite() || !minimum.cmplt(maximum).all() {
        return Err("Blade建筑净空输入的世界范围无效".to_owned()); // 除零或NaN不得进入像素换算。
    }
    let mut masked = source.to_vec(); // 每次从不可变原始基础级重建，删除建筑可以完整恢复。
    if clearances.is_empty() {
        return Ok(masked); // 空净空逐字节保留作者值。
    }
    let resolution_vec = Vec2::new(resolution[0] as f32, resolution[1] as f32);
    let maximum_pixel = (resolution_vec - Vec2::ONE).max(Vec2::ZERO);
    let texel_step = Vec2::new(
        if resolution[0] > 1 {
            (maximum.x - minimum.x) / maximum_pixel.x
        } else {
            maximum.x - minimum.x
        },
        if resolution[1] > 1 {
            (maximum.y - minimum.y) / maximum_pixel.y
        } else {
            maximum.y - minimum.y
        },
    ); // shader使用round映射，因此内部纹素各覆盖相邻中心之间的半格。
    for clearance in clearances {
        let bounds = clearance.aabb(); // 先把候选像素限制到建筑世界AABB。
        if bounds.max.x < minimum.x
            || bounds.max.y < minimum.y
            || bounds.min.x > maximum.x
            || bounds.min.y > maximum.y
        {
            continue; // 完全位于密度世界外的净空不应误清边缘纹素。
        }
        let pixel_min = ((((bounds.min - minimum) / (maximum - minimum)) * maximum_pixel
            - Vec2::splat(0.5))
        .floor())
        .clamp(Vec2::ZERO, maximum_pixel);
        let pixel_max = ((((bounds.max - minimum) / (maximum - minimum)) * maximum_pixel
            + Vec2::splat(0.5))
        .ceil())
        .clamp(Vec2::ZERO, maximum_pixel);
        let start_x = pixel_min.x as u32;
        let start_z = pixel_min.y as u32;
        let end_x = pixel_max.x as u32;
        let end_z = pixel_max.y as u32;
        for z in start_z..=end_z {
            for x in start_x..=end_x {
                let center = Vec2::new(
                    if resolution[0] > 1 {
                        minimum.x + x as f32 * texel_step.x
                    } else {
                        (minimum.x + maximum.x) * 0.5
                    },
                    if resolution[1] > 1 {
                        minimum.y + z as f32 * texel_step.y
                    } else {
                        (minimum.y + maximum.y) * 0.5
                    },
                );
                let half_extents = texel_step * 0.5 + Vec2::splat(0.001); // 轻微外扩让边界草也稳定落入净空。
                let texel = BuildingWorldQuad::from_local_rect(
                    BuildingFootprintRect {
                        center: [0.0, 0.0],                    // 世界中心通过translation单独传入。
                        half_extents: half_extents.to_array(), // 最近采样单元的保守半尺寸。
                    },
                    [center.x, 0.0, center.y], // 二维Z存入世界translation第三轴。
                    0.0,                       // 纹素格始终轴对齐。
                    [1.0; 3],                  // 世界尺寸已经完整计算，不再缩放。
                );
                if quads_intersect(*clearance, texel) {
                    masked[(z * resolution[0] + x) as usize] = 0; // 黑纹素乘任何增益仍严格不生草。
                }
            }
        }
    }
    Ok(masked)
}

/// 在CPU池中为全部Blade层生成同一净空修订的完成值。
fn build_blade_clearance(
    job: DynamicBladeClearanceJob,
) -> Result<DynamicBladeClearanceBuild, String> {
    let latest = job.draft.as_ref().map(|draft| {
        draft.definition.layers.iter().filter(|layer| layer.kind == feathrise_scene_content::SceneVegetationKind::Blade)
            .map(|layer| {
                let id = layer.runtime_layer_id.ok_or("Blade作者层缺少运行时身份")?;
                let density = draft.sources.get(&layer.id).ok_or("Blade作者层缺少完整密度")?;
                Ok((id, (layer.resolution, density.clone())))
            }).collect::<Result<BTreeMap<_, _>, String>>()
    }).transpose()?; // 描述索引与密度选择都在worker完成，不在输入帧扫描或复制像素。
    let mut layers = Vec::with_capacity(job.layers.len());
    for layer in job.layers {
        let density = if let Some(latest) = &latest {
            let (resolution, density) = latest.get(&layer.runtime_layer_id).ok_or("Blade草窗布局与作者输出不一致")?;
            if *resolution != layer.resolution { return Err("Blade密度尺寸与当前草窗不一致".into()); }
            density
        } else { &layer.density };
        layers.push(DynamicBladeClearanceLayerBuild {
            runtime_layer_id: layer.runtime_layer_id, // 完成值保留正式层身份。
            resolution: layer.resolution,             // 主线程据此建立单MipImage。
            density: mask_blade_density(
                density,
                layer.resolution,
                job.world_min,
                job.world_max,
                &job.clearances,
            )?, // 任一层坏输入拒绝整份结果，不混用不同净空修订。
        });
    }
    Ok(DynamicBladeClearanceBuild {
        generation: job.generation,                 // 迟到场景结果门。
        revision: job.revision,                     // 迟到植被结果门。
        clearance_revision: job.clearance_revision, // 迟到建筑结果门。
        draft_revision: job.draft_revision,
        layers,                                     // 全部层共同完成。
    })
}

/// 非阻塞推进唯一Blade净空任务，并在完成后替换当前根的全部密度句柄。
#[allow(clippy::too_many_arguments)]
pub(in crate::scenes::dynamic_scene) fn drive_dynamic_blade_clearance(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    windows: InstanceQuery<(Entity, &ChildOf, &VegetationBladeLayer), With<DynamicBladeWindow>>,
    clearances: InstanceRes<BuildingVegetationClearanceState>,
    draft: crate::scenes::dynamic_scene::author_sources::TerrainAuthorRes<super::DynamicBladeDraft>,
    mut state: InstanceResMut<DynamicVegetationState>,
    mut status: InstanceResMut<DynamicVegetationStatus>,
) {
    let snapshot = state
        .scene_id
        .as_ref()
        .and_then(|scene_id| clearances.snapshot_for(scene_id)); // 同图作者快照优先，跨场景来源被公共资源拒绝。
    let clearance_revision = snapshot.as_ref().map_or(0, |snapshot| snapshot.revision);
    let density_revision = state.scene_id.as_ref().and_then(|map| draft.revision_for_scene(map));
    let completed = state.blade_clearance_task.as_mut().and_then(check_ready); // 稳定帧只有一次Option和非阻塞轮询。
    if let Some(result) = completed {
        state.blade_clearance_task = None;
        state.blade_clearance_task_identity = None;
        match result {
            Ok(build)
                if state.current(build.generation, build.revision)
                    && build.clearance_revision == clearance_revision
                    && match (build.draft_revision, density_revision) {
                        (Some(before), Some(current)) => before <= current,
                        (None, None) => true, _ => false,
                    } =>
            {
                let Some(active_root) = state.active_root else {
                    return; // 根已退役的迟到完成值没有应用目标。
                };
                let mut layer_images = BTreeMap::new(); // 每个正式层只登记一张共享四窗图片。
                for layer in build.layers {
                    let mut image = Image::new(
                        Extent3d {
                            width: layer.resolution[0],  // 严格保持作者基础级宽度。
                            height: layer.resolution[1], // 严格保持作者基础级高度。
                            depth_or_array_layers: 1,    // 普通二维单层纹理。
                        },
                        TextureDimension::D2,   // Blade shader读取texture_2d。
                        layer.density,          // 后台完成Vec直接移入Image。
                        TextureFormat::R8Unorm, // 线性红通道概率，不使用sRGB。
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // CPU诊断与GPU上传均可用。
                    );
                    image.sampler = ImageSampler::nearest(); // 与shader最近textureLoad语义保持一致。
                    layer_images.insert(layer.runtime_layer_id, images.add(image)); // 完整图片登记后再改任何窗口。
                }
                // 每层窗口数等于该层 Blade 规格档数；建筑净空只替换当前活动层的全部窗口。
                let current_windows = windows
                    .iter()
                    .filter(|(_, parent, layer)| {
                        parent.parent() == active_root && layer_images.contains_key(&layer.0.0)
                    })
                    .count();
                let expected_windows = status.blade_window_count;
                if expected_windows == 0 || current_windows != expected_windows {
                    status.phase = DynamicVegetationPhase::Failed;
                    status.detail = format!("Blade建筑净空只能定位{current_windows}/{expected_windows}个当前LOD窗口");
                    return; // 使用建根回执核对完整数量；零个匹配窗口不能以0==0取得采用信用。
                }
                let mut replaced = 0usize;
                for (entity, parent, layer) in &windows {
                    if parent.parent() != active_root {
                        continue; // 隐藏候选或旧退役根不接收当前活动结果。
                    }
                    let Some(image) = layer_images.get(&layer.0.0) else {
                        continue; // 非当前完成值正式层保持原绑定。
                    };
                    commands
                        .entity(entity)
                        .insert((GpuBladeDensityMap::new(image.clone()), GpuBladeSourceRevision(build.draft_revision.unwrap_or(0)))); // 同一边界提交真实绑定及来源，后台回读只能证明实际参与Compute的版本。
                    replaced += 1;
                }
                if replaced != expected_windows {
                    status.phase = DynamicVegetationPhase::Failed;
                    status.detail =
                        format!("Blade建筑净空只能定位{replaced}/{expected_windows}个当前LOD窗口");
                    return; // 根结构异常时不把部分替换标记为已采用。
                }
                state.blade_clearance_applied_revision = Some((build.clearance_revision, build.draft_revision)); // 草窗组原子换密度，不重建根。
                status.applied_density_revision = build.draft_revision; // 只在当前根的全部窗口同次绑定后发布，单纯草稿publish不能取得此回执。
            }
            Ok(_) => {} // 植被、场景或建筑净空换代后的迟到结果直接丢弃。
            Err(error) => {
                status.phase = DynamicVegetationPhase::Failed;
                status.detail = format!("Blade建筑植被净空生成失败：{error}");
                return; // 保留上一完整密度句柄并停止同修订忙重试。
            }
        }
    }

    if status.phase == DynamicVegetationPhase::Failed
        || state.dirty
        || state.blade_clearance_task.is_some()
        || state.blade_clearance_applied_revision == Some((clearance_revision, density_revision))
    {
        return; // 等待真实来源换代，稳定帧不复制像素或重复提交任务。
    }
    let Some(source) = state.mesh_source.clone() else {
        return; // 当前植被修订尚未与可见根共同采用。
    };
    if !state.current(source.generation, source.revision) || state.active_root.is_none() {
        return; // 候选或退役源不能启动活动窗口派生任务。
    }
    if source.blade_layers.is_empty() {
        state.blade_clearance_applied_revision = Some((clearance_revision, density_revision)); // 无Blade层是常数完成值。
        status.applied_density_revision = density_revision; // 合法空结果也须属于当前源；调用方另核对期望层/窗口，不能凭它证明非空草地。
        return;
    }
    let job = DynamicBladeClearanceJob {
        generation: source.generation,       // 当前场景代次。
        revision: source.revision,           // 当前植被修订。
        clearance_revision,                  // 当前建筑净空修订。
        layers: source.blade_layers.clone(), // 原始R8 Arc浅克隆。
        clearances: snapshot.as_ref().map_or_else(
            BuildingVegetationClearanceState::empty_regions,
            |snapshot| snapshot.regions.clone(),
        ), // 每次从完整不可变建筑数组派生。
        world_min: source.world_min,         // 与Blade窗口密度世界范围一致。
        world_max: source.world_max,         // 与Blade窗口密度世界范围一致。
        draft: state.scene_id.as_ref().and_then(|map| draft.frozen_for_scene(map)),
        draft_revision: density_revision,
    };
    state.blade_clearance_task_identity =
        Some((source.generation, source.revision, clearance_revision, density_revision)); // 先登记身份再提交single-flight任务。
    state.blade_clearance_task =
        Some(AsyncComputeTaskPool::get().spawn(async move { build_blade_clearance(job) })); // 完整像素复制和四边形栅格化均离开主线程。
}

#[cfg(test)]
#[path = "clearance/live_tests.rs"]
mod live_tests;

#[cfg(test)]
mod tests {
    use super::*; // 纯栅格测试不建立Bevy App或GPU资源。

    fn clearance() -> BuildingWorldQuad {
        BuildingWorldQuad::from_local_rect(
            BuildingFootprintRect {
                center: [0.0, 0.0],       // 建筑净空以世界根为中心。
                half_extents: [1.0, 1.0], // 只覆盖四乘四纹理中心附近。
            },
            [2.0, 0.0, 2.0], // 四米见方世界中的中心位置。
            45.0,            // 旋转证明不是只按轴对齐范围清零。
            [1.0; 3],        // 单位缩放。
        )
    }

    #[test]
    fn blade_mask_clears_intersecting_texels_without_mutating_original_r8() {
        let source = vec![255_u8; 25]; // 五乘五作者基础级完整非零。
        let original = source.clone(); // 冻结调用前内容证明不被原地改写。
        let masked = mask_blade_density(&source, [5, 5], [0.0, 0.0], [4.0, 4.0], &[clearance()])
            .expect("合法R8与净空应生成派生图");
        assert_eq!(source, original); // 原始作者R8保持逐字节不变。
        assert_eq!(masked[2 * 5 + 2], 0); // 建筑中心纹素严格清零。
        assert_eq!(masked[0], 255); // 远离建筑的外部纹素保持作者概率。
    }

    #[test]
    fn empty_blade_clearance_restores_exact_original_density() {
        let source = (0_u8..25).collect::<Vec<_>>(); // 非均匀值可证明逐字节恢复。
        let masked = mask_blade_density(&source, [5, 5], [0.0, 0.0], [4.0, 4.0], &[])
            .expect("空净空仍应产生完整完成值");
        assert_eq!(masked, source); // 删除最后建筑后从原始R8完整重建。
    }
}
