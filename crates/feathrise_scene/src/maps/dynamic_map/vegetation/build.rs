//! Mesh植被不可变输入冻结与单空间块精确后台散布。

use std::sync::Arc; // 区域任务只浅克隆密度、GLB高度或Clipmap局部页快照。

use bevy::prelude::{Vec2, Vec3}; // 净空读取源X/Z，近场粗筛同时冻结真实三维根点范围。
use feathrise_map_content::{
    BuildingWorldQuad, MapVegetationInstanceTransform, MapVegetationLayer,
    generate_vegetation_instances_in_region,
}; // 区域入口保证拼块后与全图作者结果一致，建筑净空只派生最终结果。

use super::resources::{
    DynamicBladeLayerBuild, DynamicMeshChunkBuild, DynamicMeshChunkGroundWait, DynamicMeshChunkKey,
    DynamicMeshChunkOutcome, DynamicMeshLayerBuild, DynamicVegetationBuild,
    DynamicVegetationGroundSource,
}; // 后台值不携带Bevy资产句柄或ECS对象。
use crate::maps::dynamic_map::clipmap::{
    ClipmapGroundContentRevision, ClipmapGroundHit, ClipmapGroundQueryRevision,
    ClipmapGroundSnapshot,
}; // Clipmap Mesh只读取同代局部基础作者页，不把相机可见LOD写进实例Y。

/// 动态Mesh固定使用一百二十八米空间块；它决定工作粒度，不限制地图或作者实例总数。
pub(super) const DYNAMIC_MESH_CHUNK_SIZE_METERS: f32 = 128.0;

/// 一层后台作业只保存纯值数组，不把Bevy资产或ECS对象带入线程池。
pub(super) struct DynamicMeshLayerJob {
    pub(super) layer_index: usize,
    pub(super) layer: MapVegetationLayer,
    pub(super) density: Arc<[u8]>,
}

/// 一层 Blade 原始 R8 的后台冻结输入。
pub(super) struct DynamicBladeLayerJob {
    pub(super) runtime_layer_id: u64, // 正式层身份用于完成后替换四个LOD窗口。
    pub(super) resolution: [u32; 2],  // 原始基础级尺寸。
    pub(super) density: Arc<[u8]>,    // 草稿直接共享后台完成源，不在每次刷制时复制全图。
}

/// 一次地图修订的后台输入。
pub(super) struct DynamicVegetationBuildJob {
    pub(super) volume_mesh_layers: Vec<MapVegetationLayer>, // 已有逐层校验时冻结小型目录，不扫描三维作者格。
    pub(super) generation: u32,
    pub(super) revision: u32,
    pub(super) mesh_layers: Vec<DynamicMeshLayerJob>,
    pub(super) blade_layers: Vec<DynamicBladeLayerJob>,
    pub(super) ground: DynamicVegetationGroundSource,
    pub(super) world_min: [f32; 2],
    pub(super) world_max: [f32; 2],
}

/// 把当前完整修订冻结为共享CPU源；不扫描候选，也不建立全图逐株数组。
pub(super) fn build_dynamic_vegetation(
    job: DynamicVegetationBuildJob,
) -> Result<DynamicVegetationBuild, String> {
    let mut mesh_layers = Vec::with_capacity(job.mesh_layers.len());
    for input in job.mesh_layers {
        mesh_layers.push(DynamicMeshLayerBuild {
            layer_index: input.layer_index,
            layer: input.layer, // 正式层完整进入当前修订源，区域任务不得读取后续可变资产。
            density: input.density, // 草稿Arc或磁盘基础级冻结值直接进入共享源。
        });
    }
    let blade_layers = job
        .blade_layers
        .into_iter()
        .map(|input| DynamicBladeLayerBuild {
            runtime_layer_id: input.runtime_layer_id, // 窗口身份在修订内冻结。
            resolution: input.resolution,             // 派生图片保持同尺寸。
            density: Arc::from(input.density),        // 原始Vec零拷贝冻结为共享切片。
        })
        .collect();
    Ok(DynamicVegetationBuild {
        volume_mesh_layers: job.volume_mesh_layers.into(),
        generation: job.generation,
        revision: job.revision,
        mesh_layers,
        blade_layers,
        ground: job.ground,       // 后续单块按后端取得真实高度输入。
        world_min: job.world_min, // 稳定候选原点在修订内不可变。
        world_max: job.world_max, // 最外空间块使用同一地图闭边界。
    })
}

/// 单块任务的完整高度输入；Clipmap快照只共享当前局部页，不申请整图数组。
pub(super) enum DynamicMeshChunkGround {
    Heightfield {
        heights: Arc<[u16]>,
        resolution: [u32; 2],
        base_height: f32,
        height_scale: f32,
    },
    Clipmap {
        snapshot: ClipmapGroundSnapshot,
        content_revision: ClipmapGroundContentRevision,
        query_revision: ClipmapGroundQueryRevision,
    },
}

/// single-flight空间块任务输入；大数组全部通过`Arc`浅克隆。
pub(super) struct DynamicMeshChunkJob {
    pub(super) generation: u32,                      // 当前地图代次。
    pub(super) revision: u32,                        // 当前密度与高度修订。
    pub(super) clearance_revision: u64,              // 当前完整建筑净空快照修订。
    pub(super) clearances: Arc<[BuildingWorldQuad]>, // 全块任务共享不可变世界四边形，不复制建筑数组。
    pub(super) key: DynamicMeshChunkKey,             // 当前层和世界块身份。
    pub(super) layer: MapVegetationLayer,            // 当前正式层纯值描述。
    pub(super) density: Arc<[u8]>,                   // 当前层完整R8基础级只读快照。
    pub(super) ground: DynamicMeshChunkGround,       // GLB整图或当前Clipmap局部页。
    pub(super) world_min: [f32; 2],                  // 全图稳定散布原点。
    pub(super) world_max: [f32; 2],                  // 全图合法散布终点。
    pub(super) region_min: [f32; 2],                 // 当前半开常驻块最小角。
    pub(super) region_max: [f32; 2],                 // 当前半开常驻块最大角。
}

/// 从已经确定生成的作者实例中移除建筑净空内部点，不改写原始 R8。
fn retain_outside_building_clearances(
    transforms: &mut Vec<MapVegetationInstanceTransform>,
    clearances: &[BuildingWorldQuad],
) {
    if clearances.is_empty() {
        return; // 删除最后建筑后从原作者结果完整恢复，空净空不扫描实例。
    }
    transforms.retain(|transform| {
        let point = Vec2::new(transform.translation[0], transform.translation[2]); // 植被根的世界X/Z决定是否落入建筑。
        !clearances.iter().any(|quad| quad.contains(point)) // 四边形边界也视为净空，避免草贴墙穿入。
    });
}

/// 在工作线程完整生成一个空间块；没有候选或实例数量裁剪。
pub(super) fn build_dynamic_mesh_chunk(
    job: DynamicMeshChunkJob,
) -> Result<DynamicMeshChunkOutcome, String> {
    let (mut transforms, ground_revision) = match &job.ground {
        DynamicMeshChunkGround::Heightfield {
            heights,
            resolution,
            base_height,
            height_scale,
        } => (
            generate_vegetation_instances_in_region(
                &job.layer,     // 作者格距、种子和姿态范围保持原值。
                &job.density,   // 作者密度决定当前块每个候选是否存在。
                heights,        // 当前正式GLB高度场负责贴地。
                *resolution,    // 复用正式三角形剖分。
                job.world_min,  // 候选身份始终锚定全图原点。
                job.world_max,  // 最外块允许命中地图最大边界。
                *base_height,   // 正式高度基线。
                *height_scale,  // 正式高度量程。
                job.region_min, // 仅返回当前块半开范围。
                job.region_max, // 相邻块合并不会重复边界候选。
            )?,
            None,
        ),
        DynamicMeshChunkGround::Clipmap {
            snapshot,
            content_revision,
            query_revision,
        } => {
            // 【功能设计】先用同一散布纯函数生成稳定XZ/ID/朝向/缩放，再只替换Y。
            // 二乘二零高度只是无分配的候选载体，不会成为可见平地或改变概率序列。
            let mut transforms = generate_vegetation_instances_in_region(
                &job.layer,
                &job.density,
                &[0_u16; 4],
                [2, 2],
                job.world_min,
                job.world_max,
                0.0,
                1.0,
                job.region_min,
                job.region_max,
            )?;
            for transform in &mut transforms {
                let world = Vec2::new(transform.translation[0], transform.translation[2]);
                match snapshot.sample_authoritative_surface(world) {
                    ClipmapGroundHit::Hit(hit) => transform.translation[1] = hit.y,
                    ClipmapGroundHit::WaitingForHeight | ClipmapGroundHit::Miss => {
                        return Ok(DynamicMeshChunkOutcome::WaitingForGround(
                            DynamicMeshChunkGroundWait {
                                generation: job.generation,
                                revision: job.revision,
                                clearance_revision: job.clearance_revision,
                                ground_revision: *content_revision,
                                query_revision: *query_revision,
                                key: job.key,
                            },
                        )); // 不安装缺株的半块，也不以当前可见粗LOD或世界零平面替代作者高度。
                    }
                    ClipmapGroundHit::Failed => {
                        return Err("Clipmap Mesh植被取得了非法地表查询结果".to_owned());
                    }
                }
            }
            (transforms, Some(*content_revision))
        }
    };
    retain_outside_building_clearances(&mut transforms, &job.clearances); // 派生过滤不修改共享作者密度。
    // 【功能设计】作者高度在网格倾斜后会投影到世界XZ；必须在已有单块后台任务内冻结
    // 过滤后真实根点的三维范围，不能在主帧按全图或逐株重新求界。
    let source_root_bounds = transforms.iter().fold(None::<[Vec3; 2]>, |bounds, instance| {
        let root = Vec3::from_array(instance.translation);
        Some(match bounds {
            Some([minimum, maximum]) => [root.min(minimum), root.max(maximum)],
            None => [root, root],
        })
    });
    Ok(DynamicMeshChunkOutcome::Ready(DynamicMeshChunkBuild {
        generation: job.generation, // 完成值保留代次供主线程拒收迟到结果。
        revision: job.revision,     // 完成值保留修订供热更身份门核对。
        clearance_revision: job.clearance_revision, // 建筑换代后的旧完成值必须直接丢弃。
        ground_revision,            // 高度内容换代后的旧完成值必须直接丢弃。
        key: job.key,               // 安装目标必须仍是当前期望块。
        transforms,                 // 当前块全部作者实例原样返回。
        source_root_bounds,         // 空块保持None，采用时不再扫描作者实例。
    }))
}

#[cfg(test)]
mod tests {
    use feathrise_map_content::BuildingFootprintRect; // 构造与正式预设一致的世界净空。
    use feathrise_map_content::MapVegetationKind; // Clipmap散布回归使用正式Mesh层类型。

    use super::*; // 直接验证派生过滤不会破坏作者候选。

    fn instance(id: u64, x: f32, z: f32) -> MapVegetationInstanceTransform {
        MapVegetationInstanceTransform {
            instance_id: id,          // 稳定身份应随保留项原样留下。
            translation: [x, 0.0, z], // 净空只读取世界X/Z。
            yaw_radians: 0.0,         // 姿态不影响根点净空。
            scale: 1.0,               // 缩放不影响根点净空。
        }
    }

    fn mesh_layer() -> MapVegetationLayer {
        MapVegetationLayer {
            id: "tree".to_owned(),
            label: "树木".to_owned(),
            kind: MapVegetationKind::Mesh,
            runtime_profile: Some("hn01_tree".to_owned()),
            runtime_layer_id: Some(7),
            prototype_id: Some(9),
            asset_path: "tree.glb".to_owned(),
            density_source_path: "tree.r8".to_owned(),
            density_texture_path: "tree.ktx2".to_owned(),
            resolution: [2, 2],
            spacing_meters: 1.0,
            scale_range: [0.8, 1.2],
            max_instances: 1,
            seed: 42,
        }
    }
    #[test]
    fn empty_outdoor_density_keeps_mesh_definition_for_underground_only_planting() {
        let build = build_dynamic_vegetation(DynamicVegetationBuildJob {
            generation: 1,
            revision: 1,
            volume_mesh_layers: vec![mesh_layer()],
            mesh_layers: Vec::new(),
            blade_layers: Vec::new(),
            ground: DynamicVegetationGroundSource::Clipmap,
            world_min: [0.; 2],
            world_max: [16.; 2],
        })
        .unwrap();
        assert!(build.mesh_layers.is_empty());
        assert_eq!(
            build.volume_mesh_layers[0].runtime_profile.as_deref(),
            Some("hn01_tree")
        );
    }

    fn chunk_job(ground: DynamicMeshChunkGround) -> DynamicMeshChunkJob {
        DynamicMeshChunkJob {
            generation: 3,
            revision: 5,
            clearance_revision: 7,
            clearances: Arc::from(Vec::<BuildingWorldQuad>::new()),
            key: DynamicMeshChunkKey {
                layer_index: 0,
                x: 0,
                z: 0,
            },
            layer: mesh_layer(),
            density: Arc::from(vec![u8::MAX; 4]),
            ground,
            world_min: [0.0, 0.0],
            world_max: [96.0, 64.0],
            region_min: [44.0, 28.0],
            region_max: [48.0, 32.0],
        }
    }

    #[test]
    fn mesh_clearance_removes_inside_instances_and_keeps_outside_instances() {
        let clearance = BuildingWorldQuad::from_local_rect(
            BuildingFootprintRect::default(), // 默认四米见方净空。
            [0.0, 0.0, 0.0],                  // 净空位于世界原点。
            45.0,                             // 旋转证明过滤使用四边形而非AABB。
            [1.0; 3],                         // 单位缩放保持边界明确。
        );
        let mut transforms = vec![instance(1, 0.0, 0.0), instance(2, 4.0, 0.0)]; // 一项内部、一项外部。
        retain_outside_building_clearances(&mut transforms, &[clearance]);
        assert_eq!(transforms.len(), 1); // 建筑内部实例被移除。
        assert_eq!(transforms[0].instance_id, 2); // 建筑外部作者实例保持原身份。
    }

    #[test]
    fn empty_mesh_clearance_restores_the_original_author_result() {
        let original = vec![instance(1, 0.0, 0.0), instance(2, 4.0, 0.0)]; // 模拟删除建筑后的原作者散布。
        let mut transforms = original.clone();
        retain_outside_building_clearances(&mut transforms, &[]);
        assert_eq!(transforms.len(), original.len()); // 空净空不能遗留上次过滤结果。
        assert_eq!(transforms[0].translation, original[0].translation); // 原始位置逐值恢复。
        assert_eq!(transforms[1].translation, original[1].translation); // 外部位置同样不被改写。
    }

    #[test]
    fn clipmap_mesh_keeps_glb_scatter_identity_and_only_replaces_height() {
        let glb = build_dynamic_mesh_chunk(chunk_job(DynamicMeshChunkGround::Heightfield {
            heights: Arc::from(vec![0_u16; 4]),
            resolution: [2, 2],
            base_height: 0.0,
            height_scale: 1.0,
        }))
        .expect("平坦GLB块应生成");
        let DynamicMeshChunkOutcome::Ready(glb) = glb else {
            panic!("GLB没有分页等待状态");
        };
        let snapshot = crate::maps::dynamic_map::clipmap::test_ground_snapshot();
        let content_revision = snapshot.content_revision();
        let query_revision = snapshot.query_revision();
        let clipmap = build_dynamic_mesh_chunk(chunk_job(DynamicMeshChunkGround::Clipmap {
            snapshot,
            content_revision,
            query_revision,
        }))
        .expect("Clipmap局部页应生成");
        let DynamicMeshChunkOutcome::Ready(clipmap) = clipmap else {
            panic!("测试范围应完全位于已知地表");
        };
        assert_eq!(clipmap.ground_revision, Some(content_revision));
        assert_eq!(clipmap.transforms.len(), glb.transforms.len());
        assert!(!clipmap.transforms.is_empty());
        assert_ne!(clipmap.source_root_bounds, glb.source_root_bounds,
            "单块后台范围必须采用真实作者高度，不能沿用候选载体的零高度平面");
        for (left, right) in clipmap.transforms.iter().zip(&glb.transforms) {
            assert_eq!(left.instance_id, right.instance_id);
            assert_eq!(left.translation[0], right.translation[0]);
            assert_eq!(left.translation[2], right.translation[2]);
            assert_eq!(left.yaw_radians, right.yaw_radians);
            assert_eq!(left.scale, right.scale);
            assert!(left.translation[1].is_finite());
            assert_ne!(left.translation[1], right.translation[1]);
        }
    }

    #[test]
    fn clipmap_mesh_height_does_not_change_with_view_revision() {
        let first_snapshot = crate::maps::dynamic_map::clipmap::test_ground_snapshot();
        let content_revision = first_snapshot.content_revision();
        let first_query = first_snapshot.query_revision();
        let mut moved_snapshot = first_snapshot.clone();
        moved_snapshot.revision[2] += 1; // 模拟相机移动触发环带吸附和显示LOD查询换代。
        let moved_query = moved_snapshot.query_revision();
        assert_ne!(first_query, moved_query);

        let first = build_dynamic_mesh_chunk(chunk_job(DynamicMeshChunkGround::Clipmap {
            snapshot: first_snapshot,
            content_revision,
            query_revision: first_query,
        }))
        .unwrap();
        let moved = build_dynamic_mesh_chunk(chunk_job(DynamicMeshChunkGround::Clipmap {
            snapshot: moved_snapshot,
            content_revision,
            query_revision: moved_query,
        }))
        .unwrap();
        let (DynamicMeshChunkOutcome::Ready(first), DynamicMeshChunkOutcome::Ready(moved)) =
            (first, moved)
        else {
            panic!("完整基础作者页不应随显示视图进入等待");
        };
        assert_eq!(first.transforms.len(), moved.transforms.len());
        assert!(
            first
                .transforms
                .iter()
                .zip(&moved.transforms)
                .all(|(left, right)| left.translation == right.translation)
        ); // XZ和权威Mip 0 Y均与相机无关。
    }

    #[test]
    fn clipmap_mesh_waits_for_height_without_publishing_a_partial_chunk() {
        let mut snapshot = crate::maps::dynamic_map::clipmap::test_ground_snapshot();
        snapshot.ready = false; // 非空地形尚未完成采用时不能回退零平面。
        let content_revision = snapshot.content_revision();
        let query_revision = snapshot.query_revision();
        let outcome = build_dynamic_mesh_chunk(chunk_job(DynamicMeshChunkGround::Clipmap {
            snapshot,
            content_revision,
            query_revision,
        }))
        .expect("等待高度不是坏作者数据");
        let DynamicMeshChunkOutcome::WaitingForGround(wait) = outcome else {
            panic!("未就绪高度不得发布可见块");
        };
        assert_eq!(wait.ground_revision, content_revision);
        assert_eq!(wait.query_revision, query_revision);
    }
}
