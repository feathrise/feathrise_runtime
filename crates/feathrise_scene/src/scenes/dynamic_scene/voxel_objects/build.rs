//! 共享席位内逐块冻结密度、计算网格与BVH；显示和碰撞按各自焦点计划共同发布。
use super::{
    density::{DensityReader, finish_lattice, plan_lattice},
    resources::{Built, Identity, VoxelWorkLease},
    window::{self, SourceIndex},
};
use avian3d::prelude::{Collider, TrimeshFlags};
use bevy::{camera::primitives::Aabb, prelude::*, tasks::AsyncComputeTaskPool};
use feathrise_scene_content::SceneVoxelTerrainSource;
use feathrise_terrain::{
    TerrainMeshInput,
    volume::{build_density_mesh_fractional, sparse::paged::*},
};
use std::{collections::HashMap, io::Read, path::Path, sync::Arc};

pub(super) async fn prepare(
    asset_root: feathrise_scene_content::RuntimeAssetRoot,
    source: SceneVoxelTerrainSource,
    cached: Option<Arc<SourceIndex>>,
    identity: Identity,
    lease: VoxelWorkLease,
) -> Result<Built, String> {
    let lease = Arc::new(lease); // IO任务被取消时，正在运行的计算子任务仍持有同一席位。
    let index = if let Some(cached) = cached {
        cached
    } else {
        let credit = lease.clone();
        AsyncComputeTaskPool::get()
            .spawn(async move {
                let _credit = credit;
                SourceIndex::new(source).map(Arc::new)
            })
            .await?
    };
    if index.source.cell_size_meters != identity.descriptor.cell_size_meters
        || index.source.min != identity.descriptor.min
        || index.source.max != identity.descriptor.max
    {
        return Err("体素对象与作者源的规格不一致".into());
    }
    let root = asset_root.asset_root(); // 来源已在启动任务时冻结，用户工程缺页不能读取SDK同名块。
    let focus = |key: [i32; 3]| {
        (Vec3::from_array(key.map(|v| v as f32)) + Vec3::splat(0.5)) * identity.anchor_extent
    };
    let display_foci = vec![focus(identity.display_center)];
    let physics_foci = identity
        .physics_centers
        .iter()
        .copied()
        .map(focus)
        .collect::<Vec<_>>();
    let mut reader = DensityReader::new(index.clone());
    let (display_geometry, display_limited) = build_set(
        root,
        &index,
        &mut reader,
        &display_foci,
        &identity,
        false,
        &lease,
    )
    .await?;
    let (collision_geometry, collision_limited) = build_set(
        root,
        &index,
        &mut reader,
        &physics_foci,
        &identity,
        true,
        &lease,
    )
    .await?;
    let limited = display_limited || collision_limited;
    let mut field_foci = display_foci;
    let player = focus(identity.player_center);
    if player != field_foci[0] {
        field_foci.push(player);
    }
    let (fields, fields_limited) =
        load_fields(root, &index, &field_foci, identity.scale, &lease).await?;
    let credit = lease.clone();
    AsyncComputeTaskPool::get()
        .spawn(async move {
            let _credit = credit;
            let display_query = triangle_shape(&display_geometry)?;
            let mut collider = triangle_shape(&collision_geometry)?;
            if let Some(collider) = &mut collider {
                collider.set_scale(identity.scale, 10);
            }
            let meshes =
                super::super::volume_stream::build_surface_chunks(display_geometry, &fields)?
                    .into_iter()
                    .map(|mesh| {
                        let mut min = Vec3::splat(f32::INFINITY);
                        let mut max = Vec3::splat(f32::NEG_INFINITY);
                        if let Some(bevy::mesh::VertexAttributeValues::Float32x3(points)) =
                            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                        {
                            for point in points {
                                let p = Vec3::from_array(*point);
                                min = min.min(p);
                                max = max.max(p);
                            }
                        }
                        (mesh, Aabb::from_min_max(min, max))
                    })
                    .collect();
            Ok(Built {
                identity,
                meshes,
                collider,
                display_query,
                index,
                fields,
                limited: limited || fields_limited,
            })
        })
        .await
}

/// 输出、焊接表和候选工作集也必须有界；超过信用时重新计划较粗窗口，不能拒绝整个大对象。
async fn build_set(
    root: &Path,
    index: &Arc<SourceIndex>,
    reader: &mut DensityReader,
    foci: &[Vec3],
    identity: &Identity,
    collision: bool,
    lease: &Arc<VoxelWorkLease>,
) -> Result<(TerrainMeshInput, bool), String> {
    let mut maximum = if collision {
        window::COLLISION_CHUNKS
    } else {
        window::DISPLAY_CHUNKS
    };
    let mut reduced = false;
    loop {
        let source = index.clone();
        let focus = foci.to_vec();
        let scale = identity.scale;
        let credit = lease.clone();
        let lods = if collision {
            identity.descriptor.collision_lods.clone()
        } else {
            identity.descriptor.display_lods.clone()
        };
        let plan = AsyncComputeTaskPool::get()
            .spawn(async move {
                let _credit = credit;
                window::plan_with_limit(&source, &focus, scale, &lods, collision, maximum)
            })
            .await;
        if let Some(geometry) = build_level(root, index, reader, plan.chunks, lease).await? {
            return Ok((geometry, plan.limited || reduced));
        }
        if maximum == 1 {
            return Err("单体素块超过固定派生格式预算".into());
        }
        maximum = (maximum / 2).max(1);
        reduced = true; // 最多八轮；后续轮直接复用仍有效的原生块缓存。
    }
}

async fn build_level(
    root: &Path,
    index: &Arc<SourceIndex>,
    reader: &mut DensityReader,
    chunks: Vec<window::Chunk>,
    lease: &Arc<VoxelWorkLease>,
) -> Result<Option<TerrainMeshInput>, String> {
    let chunks: Arc<[window::Chunk]> = chunks.into();
    let mut geometry = TerrainMeshInput {
        positions: Vec::new(),
        indices: Vec::new(),
    };
    let mut welded = HashMap::new();
    for &chunk in chunks.iter() {
        let source = index.clone();
        let neighbors = chunks.clone();
        let cache = lease.cache.clone();
        let credit = lease.clone();
        let (key, cached) = AsyncComputeTaskPool::get()
            .spawn(async move {
                let _credit = credit;
                let key = super::cache::key(&source, chunk, &neighbors);
                let value = cache.lock().map_err(|_| "体素块缓存锁失效")?.get(&key);
                Ok::<_, String>((key, value))
            })
            .await?;
        let input = if let Some(cached) = cached {
            cached
        } else {
            let source = index.clone();
            let neighbors = chunks.clone();
            let credit = lease.clone();
            let lattice = AsyncComputeTaskPool::get()
                .spawn(async move {
                    let _credit = credit;
                    plan_lattice(&source, chunk, &neighbors)
                })
                .await;
            let lattice = reader.read(root, lattice)?;
            let domain = index.domain;
            let cache = lease.cache.clone();
            let credit = lease.clone();
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let _credit = credit;
                    let density = finish_lattice(lattice);
                    let mut input =
                        build_density_mesh_fractional(window::CELLS, 1.0, density, Vec3::ZERO)?;
                    let divisions = (1u32 << chunk.depth) as f32;
                    for point in &mut input.positions {
                        // 共享面使用同一二进制分数和相同仿射顺序，不能各块累加世界坐标。
                        let unit = (Vec3::from_array(chunk.key.map(|v| v as f32))
                            + Vec3::from_array(*point) / window::CELLS as f32)
                            / divisions;
                        *point = (domain.min + unit * (domain.max - domain.min)).to_array();
                    }
                    Ok::<_, String>(
                        cache
                            .lock()
                            .map_err(|_| "体素块缓存锁失效")?
                            .insert(key, input),
                    )
                })
                .await?
        };
        let credit = lease.clone();
        let result = AsyncComputeTaskPool::get()
            .spawn(async move {
                let _credit = credit;
                // 80字节保守覆盖每个焊接顶点、HashMap槽及位置Vec；索引另计。
                if (geometry.positions.len() + input.positions.len()) * 80
                    + (geometry.indices.len() + input.indices.len()) * 4
                    > 64 * 1024 * 1024
                {
                    return None;
                }
                // 沿用成熟分页聚合的位型焊接；共享法线只在整个集合分上传块之前派生一次。
                let mut remap = Vec::with_capacity(input.positions.len());
                for &point in &input.positions {
                    let key = point.map(|v| if v == 0.0 { 0 } else { v.to_bits() });
                    let id = *welded.entry(key).or_insert_with(|| {
                        let id = geometry.positions.len() as u32;
                        geometry.positions.push(point);
                        id
                    });
                    remap.push(id);
                }
                geometry
                    .indices
                    .extend(input.indices.iter().map(|id| remap[*id as usize]));
                Some((geometry, welded))
            })
            .await;
        let Some((next, next_welded)) = result else {
            return Ok(None);
        };
        geometry = next;
        welded = next_welded;
    }
    Ok(Some(geometry))
}

fn triangle_shape(input: &TerrainMeshInput) -> Result<Option<Collider>, String> {
    if input.indices.is_empty() {
        return Ok(None);
    }
    Collider::try_trimesh_with_config(
        input
            .positions
            .iter()
            .copied()
            .map(Vec3::from_array)
            .collect(),
        input
            .indices
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect(),
        TrimeshFlags::FIX_INTERNAL_EDGES,
    )
    .map(Some)
    .map_err(|error| format!("体素真实三角面构建失败：{error}"))
}

async fn load_fields(
    root: &Path,
    index: &Arc<SourceIndex>,
    foci: &[Vec3],
    scale: Vec3,
    lease: &Arc<VoxelWorkLease>,
) -> Result<(crate::VolumeSurfaceFields, bool), String> {
    let (mut snapshot, limited) = if let Some(path) = &index.source.surface_fields_path {
        let path = feathrise_scene_content::authoring_io::safe_content_path(root, path)?;
        let file = std::fs::File::open(&path)
            .map_err(|error| format!("读取体素三维字段版本失败：{error}"))?;
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > 4096 {
            return Err("三维字段版本载体超过4KiB格式范围".into());
        }
        let source = decode_paged_volume(&bytes)?.ok_or("体素三维字段版本不含v3作者根")?;
        if source.cell_size != index.source.cell_size_meters {
            return Err("体素字段作者格距与密度源不一致".into());
        }
        // 摄影机高度不等于被看见的表面高度；附近字段焦点先限制到真实作者体积。
        let min = Vec3::from_array(index.source.min);
        let max = Vec3::from_array(index.source.max);
        let foci = foci
            .iter()
            .map(|p| p.clamp(min, max).to_array())
            .collect::<Vec<_>>();
        let objects = path.parent().unwrap().join("objects");
        let mut maximum = 8 * 1024 * 1024;
        let mut reduced = false;
        loop {
            let (bytes, limited) = read_native_surface_fields_with_budget(
                &objects,
                &source,
                &foci,
                scale.to_array(),
                64.,
                maximum,
            )?;
            let volume = source.clone();
            let credit = lease.clone();
            let digest = index.digest;
            let top = index.source.max[1];
            let (snapshot, buffer) = AsyncComputeTaskPool::get()
                .spawn(async move {
                    let _credit = credit;
                    let snapshot =
                        decode_surface_fields(&volume, bytes)?.with_native_geometry(digest, top);
                    let buffer = field_buffer(&snapshot)?;
                    Ok::<_, String>((snapshot, buffer))
                })
                .await?;
            if buffer.cast_slice::<u8>().map_or(0, <[u8]>::len)
                <= crate::shared::large_scene::MAX_TERRAIN_GPU_UPLOAD_BYTES_PER_FRAME
            {
                return Ok((
                    crate::VolumeSurfaceFields(
                        Arc::new(snapshot),
                        Arc::new(std::sync::Mutex::new(Some(buffer))),
                    ),
                    limited || reduced,
                ));
            }
            let credit = lease.clone();
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let _credit = credit;
                    drop(snapshot);
                    drop(buffer);
                })
                .await;
            if maximum == 2 * 1024 * 1024 {
                return Err("单个字段叶超过GPU固定格式信用".into());
            }
            maximum /= 2;
            reduced = true; // 8→4→2MiB，只缩小本次附近驻留，原作者根完全保留。
        }
    } else {
        (
            VolumeFieldSnapshot::empty(index.source.cell_size_meters),
            false,
        )
    };
    snapshot = snapshot.with_native_geometry(index.digest, index.source.max[1]);
    let credit = lease.clone();
    AsyncComputeTaskPool::get()
        .spawn(async move {
            let _credit = credit;
            let buffer = field_buffer(&snapshot)?;
            Ok((
                crate::VolumeSurfaceFields(
                    Arc::new(snapshot),
                    Arc::new(std::sync::Mutex::new(Some(buffer))),
                ),
                limited,
            ))
        })
        .await
}
fn field_buffer(
    snapshot: &VolumeFieldSnapshot,
) -> Result<bevy::render::storage::ShaderBuffer, String> {
    feathrise_environment::terrain_surface::build_volume_fields_with_puddles(
        snapshot.cell_size,
        snapshot.samples.values().map(|sample| {
            (
                sample.position,
                sample.normal,
                sample.surface.as_slice(),
                sample.puddles,
            )
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_coarse_mesher_keeps_thin_top_bottom_and_side_surfaces() {
        let source = SceneVoxelTerrainSource {
            schema_version: feathrise_scene_content::SCENE_SCHEMA_VERSION,
            scene_id: feathrise_scene_content::SceneId::parse("MAP-native").unwrap(),
            terrain_id: feathrise_scene_content::TerrainId::parse("box").unwrap(),
            cell_size_meters: 0.5,
            min: [-2., -0.1, -2.],
            max: [2., 0., 2.],
            blocks: Default::default(),
            surface_fields_path: None,
        };
        let index = Arc::new(SourceIndex::new(source).unwrap());
        let chunk = window::Chunk {
            depth: 0,
            key: [0; 3],
        };
        let mut reader = DensityReader::new(index.clone());
        let density = finish_lattice(
            reader
                .read(Path::new("."), plan_lattice(&index, chunk, &[chunk]))
                .unwrap(),
        );
        let mut mesh = build_density_mesh_fractional(32, 1.0, density, Vec3::ZERO).unwrap();
        for point in &mut mesh.positions {
            *point = (index.domain.min
                + Vec3::from_array(*point) / 32.0 * (index.domain.max - index.domain.min))
                .to_array();
        }
        let shape = triangle_shape(&mesh).unwrap().unwrap();
        for (origin, direction, expected) in [
            (Vec3::new(0., 10., 0.), Vec3::NEG_Y, 10.),
            (Vec3::new(0., -10., 0.), Vec3::Y, 9.9),
            (Vec3::new(10., -0.05, 0.), Vec3::NEG_X, 8.),
        ] {
            let ray = avian3d::parry::query::Ray::new(origin, direction);
            assert!(
                (shape.shape().cast_local_ray(&ray, 100., false).unwrap() - expected).abs() < 0.02
            );
        }
    }
}
