//! 作者与运行时共用的体积显示派生：有界IO冻结，计算池派生LOD和材质属性。
use bevy::{prelude::*, tasks::AsyncComputeTaskPool};
use feathrise_map_terrain::volume::sparse::paged::*;
use std::sync::atomic::AtomicU64;
use std::{path::Path, sync::Arc};

/// 作者块与实际采用网格一起发布；候选根隐藏期间不能驱动植被或水体。
#[derive(Component, Clone, Default)]
#[component(on_remove=retire_fields)]
pub struct VolumeSurfaceFields(
    pub Arc<VolumeFieldSnapshot>,
    pub Arc<std::sync::Mutex<Option<bevy::render::storage::ShaderBuffer>>>,
);
impl VolumeSurfaceFields {
    /// 只检查当前RenderDevice真实绑定上限；显存总量大也不能越过单个storage绑定的格式边界。
    pub fn validate_storage_limit(&self, limit: Option<u64>) -> Result<(), String> {
        if let Some(limit) = limit {
            let buffer = self.1.try_lock().map_err(|_| "三维材质缓冲尚未交接")?;
            let bytes = buffer
                .as_ref()
                .and_then(|b| b.data.as_ref())
                .map_or(0, Vec::len) as u64;
            if bytes > limit {
                return Err(format!(
                    "三维材质缓冲{:.1}MiB超过当前设备单绑定上限{:.1}MiB",
                    bytes as f64 / 1048576.,
                    limit as f64 / 1048576.
                ));
            }
        }
        Ok(())
    }
}

#[derive(Resource, Default)]
pub(super) struct FieldGarbage {
    queue: Vec<VolumeSurfaceFields>,
    task: Option<bevy::tasks::Task<()>>,
    in_flight: usize,
}
impl FieldGarbage {
    pub(super) fn saturated(&self) -> bool {
        self.queue.len() + self.in_flight >= 128
    }
}
fn retire_fields(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    let fields = world.get::<VolumeSurfaceFields>(context.entity).cloned(); // 同时保留尚未移交GPU的ShaderBuffer，取消时也不能在主帧析构大字节数组。
    if let Some(fields) = fields
        && let Some(mut garbage) = world.get_resource_mut::<FieldGarbage>()
    {
        debug_assert!(
            garbage.queue.len() + garbage.in_flight < 384,
            "体积根退役信用超限"
        );
        garbage.queue.push(fields);
    }
}
pub(super) fn release_fields(mut garbage: ResMut<FieldGarbage>) {
    if garbage
        .task
        .as_mut()
        .and_then(bevy::tasks::futures::check_ready)
        .is_some()
    {
        garbage.task = None;
        garbage.in_flight = 0;
    }
    if garbage.task.is_none() && !garbage.queue.is_empty() {
        let retired = std::mem::take(&mut garbage.queue);
        garbage.in_flight = retired.len();
        garbage.task = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired) }));
    }
}

pub struct VolumeDisplayBuild {
    pub meshes: Vec<Mesh>,
    pub footprint: Vec<[[f32; 2]; 2]>,
    pub fields: VolumeSurfaceFields,
}

/// 调用方必须持有唯一IO槽；内部计算串行等待，不生成隐藏后台队列。
pub async fn build_volume_display(
    objects: &Path,
    source: &PagedVolume,
    foci: &[[f32; 3]],
    lods: Arc<[VolumeDisplayLod]>,
) -> Result<VolumeDisplayBuild, String> {
    build_volume_display_with_progress(objects, source, foci, lods, None).await
}
pub(super) async fn build_volume_display_with_progress(
    objects: &Path,
    source: &PagedVolume,
    foci: &[[f32; 3]],
    lods: Arc<[VolumeDisplayLod]>,
    progress: Option<Arc<AtomicU64>>,
) -> Result<VolumeDisplayBuild, String> {
    let mut error = String::new();
    for radius in (1..=4).rev() {
        let resident = match load_display_volume_at_foci_with_progress(
            objects,
            source,
            foci,
            radius,
            progress.clone(),
        )
        .await
        {
            Ok(v) => v,
            Err(e) if is_residency_budget_error(&e) => {
                error = e;
                continue;
            }
            Err(e) => return Err(e),
        };
        let Some(input) = resident.mesh else {
            return Ok(VolumeDisplayBuild {
                meshes: Vec::new(),
                footprint: resident.footprint,
                fields: default(),
            });
        };
        // 先冻结细网格所覆盖作者块，LOD简化后仍保留洞内种植点与原始高度。
        let field_source = source.clone();
        let field_foci = foci.to_vec();
        let (input, plan) = AsyncComputeTaskPool::get()
            .spawn(async move {
                let plan = plan_surface_fields(
                    &field_source,
                    &input.positions,
                    &field_foci,
                    radius as u32,
                );
                (input, plan)
            })
            .await;
        let bytes = match read_surface_fields(objects, source, plan) {
            Ok(bytes) => bytes,
            Err(e) if is_residency_budget_error(&e) => {
                error = e;
                continue;
            }
            Err(e) => return Err(e),
        };
        let field_source = source.clone();
        let fields = match AsyncComputeTaskPool::get()
            .spawn(async move { decode_surface_fields(&field_source, bytes) })
            .await
        {
            Ok(fields) => Arc::new(fields),
            Err(e) if is_residency_budget_error(&e) => {
                error = e;
                continue;
            }
            Err(e) => return Err(e),
        };
        let field_view = fields.clone();
        let foci = foci.to_vec();
        let lods = lods.clone();
        let protected = (radius as f32 + 1.0) * source.extent();
        let result = AsyncComputeTaskPool::get()
            .spawn(async move {
                let input = apply_display_lods(input, &lods, &foci, protected)?;
                super::chunks::build_meshes(
                    input,
                    &VolumeSurfaceFields(field_view.clone(), default()),
                )
                .and_then(|meshes| {
                    let buffer =
                        feathrise_environment::terrain_surface::build_volume_fields_with_puddles(
                            field_view.cell_size,
                            field_view
                                .samples
                                .values()
                                .map(|s| (s.position, s.normal, s.surface.as_slice(), s.puddles)),
                        )?;
                    Ok((meshes, buffer))
                })
            })
            .await;
        match result {
            Ok((meshes, buffer)) => {
                return Ok(VolumeDisplayBuild {
                    meshes,
                    footprint: resident.footprint,
                    fields: VolumeSurfaceFields(
                        fields,
                        Arc::new(std::sync::Mutex::new(Some(buffer))),
                    ),
                });
            }
            Err(e) if is_residency_budget_error(&e) => error = e,
            Err(e) => return Err(e),
        }
    }
    Err(format!("体积显示局部驻留失败：{error}"))
}
