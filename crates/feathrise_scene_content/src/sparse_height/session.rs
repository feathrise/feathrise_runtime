//! 当前场景会话的稀疏浅坑覆盖；不进入作者块、F12草稿或任何保存事务。

use super::{SPARSE_HEIGHT_TILE_CELLS, SparseHeightSpec, SparseHeightTileKey}; // 世界格点保持唯一归属。
use crate::{SceneId, SurfaceCraterResponse};
use bevy::prelude::{Message, Resource, Vec2, Vec3}; // 请求与会话覆盖通过ECS传递。
use std::collections::BTreeMap; // 块索引只存真实发生破坏的区域。
use std::sync::Arc; // 后台GPU页与碰撞任务共享不可变浅坑快照。

const MAX_CRATER_TILES: usize = 256; // 会话最多占用256个实际作者块。
const MAX_STAMPS_PER_TILE: usize = 64; // 单格点采样的叠加次数受硬门约束。
const MAX_CRATER_SOURCE_SNAPSHOTS: usize = 16; // 多次热重涂后最多冻结16代完整作者画布。

/// 战斗一次施放产生的独立地形效果；origin并非地面高度，场景须查真实落点。
#[derive(Message, Clone, Debug)]
pub struct SparseHeightCraterRequest {
    pub scene_id: SceneId,   // 施放开始所在场景，拒绝跨场景迟到消息。
    pub origin: Vec3,    // 施法者位置，只用于朝下命中真实地面。
    pub radius: f32,     // 独立于战斗范围的米制浅坑半径。
    pub depth: f32,      // 单次最大下沉米数。
    pub ground_query_mask: u32, // 应用显式选择可查询层；共享地形不推断玩家或怪物身份。
}

/// 已验证的局部圆形笔印；不持有场景网格或作者高度。
#[derive(Clone, Debug)]
pub struct CraterStamp {
    center: Vec2,                        // 对象源固定X/Z，与Clipmap观察中心无关，随所属网格姿态移动。
    radius: f32,                         // 共享边的所有者块在相同世界坐标得到相同偏移。
    radii: Vec2,                        // 独立网格缩放后仍使用世界圆刷，源坐标中可为椭圆。
    depth: f32,                          // 多次独立施放可以累加，不再受创建时旧u16下界限制。
    surface: Arc<SurfaceCraterResponse>, // 当次材质响应冻结，场景重涂不会改变旧坑。
    revision: u64,                       // 碰撞只需重建上次采用后实际受本次技能影响的世界块。
}

impl CraterStamp {
    /// 作者平台不能把运行时浅坑写入保存；后台按冻结笔印检查局部矩形。
    pub fn intersects_rect(&self, minimum: Vec2, maximum: Vec2) -> bool {
        ((self.center.clamp(minimum, maximum) - self.center) / self.radii).length_squared() < 1.0
    }
}

/// 任务只克隆相关块的Arc，避免按全图拷贝会话破坏。
pub type SparseCraterTiles = BTreeMap<SparseHeightTileKey, Arc<Vec<CraterStamp>>>;

/// 一个场景运行会话中的破坏状态；作者保存绝不引用本资源。
#[derive(Resource, Default)]
pub struct SparseHeightSessionCrater {
    scene_id: Option<SceneId>,                    // 无目标场景时不允许发布浅坑。
    revision: u64,                            // 每次成功施放与清空均推进身份。
    tiles: SparseCraterTiles,                 // 仅受影响块拥有不可变笔印序列。
    stamps: Vec<CraterStamp>, // 每次成功施放只记一次世界笔印，避免跨块重复规划碰撞。
    sources: Vec<Arc<SurfaceCraterResponse>>, // 有界持有不同作者修订，不因反复重涂无限增长内存。
    depth_bound: f64, // 每次成功施放累加的保守源Y偏移；范围消费者O(1)读取，不扫描全部笔印。
}

impl SparseHeightSessionCrater {
    pub fn scene_id(&self) -> Option<&SceneId> {
        self.scene_id.as_ref()
    } // 消费者核对场景身份。
    pub fn revision(&self) -> u64 {
        self.revision
    } // 迟到任务核对破坏身份。
    pub fn tiles(&self) -> &SparseCraterTiles {
        &self.tiles
    } // 冻结任务时只Arc浅克隆。

    pub fn depth_bound(&self) -> f32 {
        self.depth_bound.min(f64::from(f32::MAX)) as f32
    }
    pub fn changed_footprints_since(
        &self,
        revision: u64,
    ) -> impl Iterator<Item = (Vec2, f32)> + '_ {
        self.stamps
            .iter()
            .filter(move |stamp| stamp.revision > revision)
            .map(|stamp| (stamp.center, stamp.radius)) // 仅本次修订之后的笔印可能改变旧Collider。
    }
    pub fn clear(&mut self, scene_id: Option<SceneId>) {
        self.tiles.clear(); // 同图重进也重置运行时破坏。
        self.stamps.clear(); // 旧图的局部重建范围也必须一起失效。
        self.sources.clear(); // 跨场景释放全部已冻结CPU作者源。
        self.depth_bound = 0.0;
        self.scene_id = scene_id; // 新图初始保持空覆盖。
        self.revision = self.revision.wrapping_add(1).max(1); // 拒绝旧任务复活。
    }

    pub fn apply(
        &mut self,
        spec: &SparseHeightSpec,
        center: Vec2,
        radius: f32,
        depth: f32,
        surface: Arc<SurfaceCraterResponse>,
    ) -> Result<(), String> {
        self.apply_world_metric(spec, center, radius, depth, Vec3::ONE, surface)
    }

    /// 技能半径/深度始终为世界米；旋转已由落点逆变换处理，正缩放只改变源椭圆和源Y深度。
    pub fn apply_world_metric(
        &mut self,
        spec: &SparseHeightSpec,
        center: Vec2,
        radius: f32,
        depth: f32,
        source_scale: Vec3,
        surface: Arc<SurfaceCraterResponse>,
    ) -> Result<(), String> {
        if self.scene_id.is_none()
            || !center.is_finite()
            || !radius.is_finite()
            || !depth.is_finite()
            || !(0.0..=8.0).contains(&radius)
            || !(0.0..=2.0).contains(&depth)
            || radius == 0.0
            || depth == 0.0
            || !source_scale.is_finite()
            || source_scale.min_element() <= 0.0
            || center.x < spec.world_min[0]
            || center.x > spec.world_max[0]
            || center.y < spec.world_min[1]
            || center.y > spec.world_max[1]
        {
            return Err("地形浅坑请求无效或没有当前场景".into()); // 坏配置不能污染会话修订。
        }
        let radii = Vec2::splat(radius) / Vec2::new(source_scale.x, source_scale.z);
        let depth = depth / source_scale.y;
        if !radii.is_finite() || radii.min_element() <= 0.0 || !depth.is_finite() || depth <= 0.0 {
            return Err("地形浅坑的源空间米制变换无效".into());
        }
        let lower = (center - radii).to_array(); // 世界圆在非等比缩放的源坐标中为椭圆。
        let upper = (center + radii).to_array(); // 不按整图遍历块。
        let mut keys = Vec::new(); // 世界8米硬门之外，缩放后的源空间覆盖仍受最多256块保护。
        let counts = spec.tile_counts(); // 场景范围已由受检清单声明。
        let mut spans = [(0_u32, 0_u32); 2]; // 各轴包含共享边可能落入的所有者块。
        for axis in 0..2 {
            let from = ((f64::from(lower[axis]) - f64::from(spec.world_min[axis]))
                / spec.actual_spacing_meters[axis])
                .floor()
                .max(0.0) as u64;
            let to = ((f64::from(upper[axis]) - f64::from(spec.world_min[axis]))
                / spec.actual_spacing_meters[axis])
                .ceil()
                .max(0.0) as u64;
            spans[axis] = (
                (from / SPARSE_HEIGHT_TILE_CELLS).min(u64::from(counts[axis] - 1)) as u32,
                (to / SPARSE_HEIGHT_TILE_CELLS).min(u64::from(counts[axis] - 1)) as u32,
            );
        }
        if (u64::from(spans[0].1 - spans[0].0) + 1)
            .saturating_mul(u64::from(spans[1].1 - spans[1].0) + 1) > MAX_CRATER_TILES as u64 {
            return Err("当前浅坑源空间覆盖块超过会话预算".into()); // 极小对象缩放也不能先枚举巨量块再拒绝。
        }
        for z in spans[1].0..=spans[1].1 {
            for x in spans[0].0..=spans[0].1 {
                keys.push(SparseHeightTileKey { x, z });
            }
        } // 块边请求一次覆盖双方所有者。
        if self.tiles.len()
            + keys
                .iter()
                .filter(|key| !self.tiles.contains_key(key))
                .count()
            > MAX_CRATER_TILES
            || keys.iter().any(|key| {
                self.tiles
                    .get(key)
                    .is_some_and(|stamps| stamps.len() >= MAX_STAMPS_PER_TILE)
            })
        {
            return Err("当前场景会话的浅坑块或局部连续破坏超过预算".into()); // 失败前不改变任意块。
        }
        let new_source = !self.sources.iter().any(|old| Arc::ptr_eq(old, &surface));
        if new_source && self.sources.len() == MAX_CRATER_SOURCE_SNAPSHOTS {
            return Err("当前场景会话的浅坑地表修订快照超过预算".into()); // 保留旧坑，避免一笔一整图无限积累。
        }
        let revision = self.revision.wrapping_add(1).max(1); // 所有被同次技能触及的块共享一个修订。
        let stamp = CraterStamp {
            center,
            radius: radii.max_element(), // 碰撞脏区使用保守包围圆，不缩小椭圆实际影响范围。
            radii,
            depth,
            surface,
            revision,
        }; // 一次施放形成一个连续圆形笔印。
        if new_source {
            self.sources.push(Arc::clone(&stamp.surface)); // 只追踪成功采用过的唯一源身份。
        }
        for key in keys {
            let mut stamps = self
                .tiles
                .get(&key)
                .map_or_else(Vec::new, |old| old.as_ref().clone()); // Arc快照在后台继续有效。
            stamps.push(stamp.clone()); // Arc共享冻结源，只复制局部笔印列表。
            self.tiles.insert(key, Arc::new(stamps)); // 新修订整体由ECS资源发布。
        }
        self.stamps.push(stamp); // 先完整写入全部局部块，再发布单次施放身份。
        self.depth_bound += f64::from(depth); // 材质响应≤1，所有笔印全重叠仍不会低于该保守下界。
        self.revision = revision; // GPU与碰撞都读取同一修订。
        Ok(())
    }
}

/// 在权威作者高度上合成会话偏移；GPU、碰撞和世界查询共用这个纯函数。
pub fn sample_crater_height(
    spec: &SparseHeightSpec,
    tiles: &SparseCraterTiles,
    base: [u64; 2],
    height: f32,
) -> f32 {
    let Some((key, _)) = spec.tile_for_grid_point(base[0], base[1]) else {
        return height;
    }; // 场景外的查询保持原高度。
    let Some(stamps) = tiles.get(&key) else {
        return height;
    }; // 未破坏地表无额外工作。
    let Some([world_x, world_z]) = spec.grid_point_world(base[0], base[1]) else {
        return height;
    }; // 所有消费者复用唯一格点。
    let mut delta = 0.0_f64; // 同图多次技能累积浅坑；f64中间值避免有限f32叠加先溢出。
    for stamp in stamps.iter() {
        let offset = Vec2::new(world_x as f32, world_z as f32) - stamp.center; // 不以命中目标或相机位置为中心。
        let normalized = (offset / stamp.radii).length_squared(); // 世界圆在源空间保持正确椭圆，边缘零位移。
        if normalized < 1.0 {
            delta += f64::from(stamp.depth)
                * f64::from(stamp
                    .surface
                    .sample(Vec2::new(world_x as f32, world_z as f32)))
                * f64::from((1.0 - normalized).powi(2)); // 每个格点各自混合岩土，而非只取坑中心材质。
        } // 平滑浅坑而非垂直洞壁。
    }
    (f64::from(height) - delta).max(-f64::from(f32::MAX)) as f32 // 只守浮点可表示边界，不把旧作者量程当作地底。
}

#[cfg(test)]
mod tests {
    use super::*; // 测试真实格点边界与作者数据隔离。

    fn soil() -> Arc<SurfaceCraterResponse> {
        Arc::new(
            SurfaceCraterResponse::new(
                Vec2::splat(-32.0),
                Vec2::splat(32.0),
                [1, 1],
                Arc::from([0_u8; 16]),
                Arc::from([255_u8, 0, 0, 0]),
                vec![1.0],
            )
            .unwrap(),
        )
    }

    #[test]
    fn crater_spans_four_author_tiles_and_clears_on_scene_change() {
        let spec =
            SparseHeightSpec::new([-32.0, -32.0], [32.0, 32.0], 0.125, -80.0, 160.0).unwrap(); // 世界原点正好是四块交点。
        let scene_id = SceneId::parse("MAP-crater-one").unwrap(); // 技能请求属于当前场景。
        let mut session = SparseHeightSessionCrater::default(); // 作者高度不会被任何技能编辑。
        session.clear(Some(scene_id.clone())); // 进入场景才建立会话身份。
        session.apply(&spec, Vec2::ZERO, 4.0, 0.5, soil()).unwrap(); // 技能在四块边界留下一个连续浅坑。
        assert_eq!(session.tiles().len(), 4); // 四个基础所有者块各持有同一笔印。
        assert_eq!(
            sample_crater_height(&spec, session.tiles(), [256, 256], 0.0),
            -0.5
        ); // 唯一所有者格点位于坑底。
        assert!(sample_crater_height(&spec, session.tiles(), [255, 256], 0.0) < 0.0); // 相邻块共享连续高度。
        assert_eq!(
            sample_crater_height(&spec, session.tiles(), [224, 256], 0.0),
            0.0
        ); // 半径之外仍是原地面。
        let first_revision = session.revision(); // 每次成功编辑需使GPU和碰撞一起换代。
        session.apply(&spec, Vec2::ZERO, 4.0, 0.5, soil()).unwrap(); // 同图多次施放可连续叠加。
        assert_eq!(
            sample_crater_height(&spec, session.tiles(), [256, 256], 0.0),
            -1.0
        ); // 不依赖伤害命中目标数。
        assert!(session.revision() > first_revision); // 同位置的连续技能也不能复用旧结果。
        session.clear(Some(SceneId::parse("MAP-crater-two").unwrap())); // 切换场景清空会话覆盖。
        assert!(session.tiles().is_empty()); // 原图作者数据并未被改动或保存。
        assert_eq!(
            sample_crater_height(&spec, session.tiles(), [256, 256], 0.0),
            0.0
        ); // 新图恢复原始高度。
    }

    #[test]
    fn rejected_crater_keeps_previous_revision_and_height() {
        let spec = SparseHeightSpec::new([-32.0, -32.0], [32.0, 32.0], 2.0, -80.0, 160.0).unwrap(); // 默认2米格距可见4米浅坑。
        let mut session = SparseHeightSessionCrater::default();
        session.clear(Some(SceneId::parse("MAP-crater-budget").unwrap()));
        session.apply(&spec, Vec2::ZERO, 4.0, 0.5, soil()).unwrap();
        let revision = session.revision();
        assert!(session.apply(&spec, Vec2::ZERO, 9.0, 0.5, soil()).is_err()); // 超出单次预算拒绝整个请求。
        assert_eq!(session.revision(), revision); // 失败不发布半修订。
        assert_eq!(
            sample_crater_height(&spec, session.tiles(), [16, 16], 0.0),
            -0.5
        ); // 旧有效地面保持。
    }

    #[test]
    fn rock_soil_border_uses_each_grid_point_and_old_stamp_is_frozen() {
        let spec =
            SparseHeightSpec::new([-32.0, -32.0], [32.0, 32.0], 0.125, -80.0, 160.0).unwrap();
        let indices: Arc<[u8]> = (0..16)
            .flat_map(|pixel| {
                [u32::from(pixel >= 8), 0, 0, 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
            })
            .collect::<Vec<_>>()
            .into();
        let weights: Arc<[u8]> = [255, 0, 0, 0].repeat(16).into();
        let response = Arc::new(
            SurfaceCraterResponse::new(
                Vec2::splat(-32.0),
                Vec2::splat(32.0),
                [16, 1],
                indices.clone(),
                weights.clone(),
                vec![1.0, 0.1],
            )
            .unwrap(),
        );
        let mut session = SparseHeightSessionCrater::default();
        session.clear(Some(SceneId::parse("MAP-rock-soil").unwrap()));
        session
            .apply(&spec, Vec2::ZERO, 4.0, 0.5, response)
            .unwrap();
        assert!(
            (sample_crater_height(&spec, session.tiles(), [240, 256], 0.0) + 0.28125).abs() < 0.001
        );
        assert!(
            (sample_crater_height(&spec, session.tiles(), [272, 256], 0.0) + 0.028125).abs()
                < 0.001
        );
        assert!(
            (sample_crater_height(&spec, session.tiles(), [256, 256], 0.0) + 0.275).abs() < 0.001
        );
        let edited_response = Arc::new(
            SurfaceCraterResponse::new(
                Vec2::splat(-32.0),
                Vec2::splat(32.0),
                [16, 1],
                indices,
                weights,
                vec![0.0, 0.0],
            )
            .unwrap(),
        );
        session
            .apply(&spec, Vec2::ZERO, 4.0, 0.5, edited_response)
            .unwrap();
        assert!(
            (sample_crater_height(&spec, session.tiles(), [256, 256], 0.0) + 0.275).abs() < 0.001
        ); // 新预设不追溯修改旧坑。
    }

    #[test]
    fn snapshot_budget_rejects_without_changing_previous_crater() {
        let spec =
            SparseHeightSpec::new([-32.0, -32.0], [32.0, 32.0], 0.125, -80.0, 160.0).unwrap();
        let mut session = SparseHeightSessionCrater::default();
        session.clear(Some(SceneId::parse("MAP-crater-source-budget").unwrap()));
        for _ in 0..MAX_CRATER_SOURCE_SNAPSHOTS {
            session.apply(&spec, Vec2::ZERO, 4.0, 0.5, soil()).unwrap();
        }
        let revision = session.revision();
        let height = sample_crater_height(&spec, session.tiles(), [256, 256], 0.0);
        assert!(session.apply(&spec, Vec2::ZERO, 4.0, 0.5, soil()).is_err());
        assert_eq!(session.revision(), revision);
        assert_eq!(
            sample_crater_height(&spec, session.tiles(), [256, 256], 0.0),
            height
        );
    }

    #[test]
    fn world_metric_crater_preserves_world_circle_depth_and_rejects_scaled_footprint_over_budget() {
        let spec = SparseHeightSpec::new([-32.0; 2], [32.0; 2], 0.125, -8.0, 16.0).unwrap();
        let mut session = SparseHeightSessionCrater::default();
        session.clear(Some(SceneId::parse("MAP-crater-scale").unwrap()));
        session.apply_world_metric(&spec, Vec2::ZERO, 4.0, 0.5, Vec3::new(2.0, 3.0, 0.5), soil()).unwrap();
        let center = sample_crater_height(&spec, session.tiles(), [256, 256], 0.0);
        assert!((center * 3.0 + 0.5).abs() < 0.0001);
        let x = sample_crater_height(&spec, session.tiles(), [264, 256], 0.0);
        let z = sample_crater_height(&spec, session.tiles(), [256, 288], 0.0);
        assert!((x - z).abs() < 0.0001 && x < 0.0);
        let stamp = &session.tiles().values().next().unwrap()[0];
        assert!(!stamp.intersects_rect(Vec2::new(2.5, -0.1), Vec2::new(3.0, 0.1)),
            "平台检查使用真实椭圆，不能把半径8的包围圆当实际破坏");
        let revision = session.revision();
        let huge = SparseHeightSpec::new([-4096.0; 2], [4096.0; 2], 0.125, -8.0, 16.0).unwrap();
        assert!(session.apply_world_metric(&huge, Vec2::ZERO, 4.0, 0.5, Vec3::splat(0.001), soil()).is_err());
        assert_eq!(session.revision(), revision, "极小对象缩放先检查256块预算，拒绝保持旧完整坑");
        assert_eq!(sample_crater_height(&spec, session.tiles(), [256, 256], 0.0), center);
    }
}
