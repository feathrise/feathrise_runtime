//! 与观察相机无关的基础作者高度页缓存。
//!
//! 【功能设计】可见 Clipmap 环带只负责给作者工具确定世界 X/Z；普通摆放和 Mesh 植被
//! 最终使用基础作者网格（Mip 0）的固定三角剖分求 Y。这里按实际查询局部加载至多
//! 六十四页，不建立整图高度缓存；正式显示页最多三个 IO，本域只保留一个 IO，二者
//! 合计仍不超过四个在途任务。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use bevy::asset::LoadState;
use bevy::prelude::*;
use feathrise_map_content::{
    MapId, SparseCraterTiles, SparseHeightLiveOverride, SparseHeightManifestAsset,
    SparseHeightSessionCrater, SparseHeightSpec, SparseHeightTile, SparseHeightTileAsset,
    SparseHeightTileKey, sample_crater_height,
};

use super::ground::{ClipmapGroundContentRevision, ClipmapGroundHit, DynamicClipmapGroundSurface};
use super::residency::SparseHeightPageRuntime;

const MAX_AUTHORITY_PAGES: usize = 64; // 固定六十四页共享Arc；v3作者浮点页最多约16.1MiB，GPU上传仍独立受8MiB门约束。
const MAX_AUTHORITY_RECT_PAGES: usize = 36; // 实际格距可略小于请求值，128m块含边界最坏按6×6页设硬门。

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuthorityIdentity {
    pub(super) map_id: MapId,
    pub(super) content: ClipmapGroundContentRevision,
}

#[derive(Default)]
struct AuthorityRequestQueue {
    identity: Option<AuthorityIdentity>,
    pages: BTreeSet<SparseHeightTileKey>,
}

/// 查询快照共享的有界需求槽。旧任务只能提交与当前正式内容身份相同的页，不能挤掉新图请求。
#[derive(Clone, Default)]
pub(super) struct AuthorityRequestSink(Arc<Mutex<AuthorityRequestQueue>>);

impl AuthorityRequestSink {
    fn retarget(&self, identity: AuthorityIdentity) {
        let mut queue = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if queue.identity.as_ref() != Some(&identity) {
            queue.identity = Some(identity);
            queue.pages.clear();
        }
    }

    pub(super) fn clear(&self) {
        let mut queue = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.identity = None;
        queue.pages.clear();
    }

    fn request(&self, identity: &AuthorityIdentity, pages: &[SparseHeightTileKey]) -> bool {
        let mut queue = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if queue.identity.as_ref() != Some(identity) {
            return false; // 跨图或跨高度修订的迟到 worker 不得污染当前需求。
        }
        let additional = pages
            .iter()
            .filter(|key| !queue.pages.contains(key))
            .count();
        if queue.pages.len().saturating_add(additional) > MAX_AUTHORITY_PAGES {
            return false; // 整批拒绝，调用者稍后重试，不能只加载矩形的一部分。
        }
        queue.pages.extend(pages.iter().copied());
        true
    }

    fn drain(&self, identity: &AuthorityIdentity) -> BTreeSet<SparseHeightTileKey> {
        let mut queue = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if queue.identity.as_ref() != Some(identity) {
            return BTreeSet::new();
        }
        std::mem::take(&mut queue.pages)
    }
}

#[derive(Clone)]
pub(super) enum AuthorityPage {
    Flat,
    Tile(Arc<SparseHeightTile>),
    Failed(Arc<str>),
}

struct CachedAuthorityPage {
    value: AuthorityPage,
    last_requested: u64,
}

struct AuthorityLoad {
    identity: AuthorityIdentity,
    key: SparseHeightTileKey,
    sha256: String,
    handle: Handle<SparseHeightTileAsset>,
}

/// 一个查询修订的不可变基础页集合；旧后台任务可继续读取，但采用时仍由外层身份拒收。
pub(super) struct AuthorityHeightSnapshot {
    pub(super) identity: AuthorityIdentity,
    pub(super) revision: u64,
    pub(super) spec: SparseHeightSpec,
    pub(super) pages: BTreeMap<SparseHeightTileKey, AuthorityPage>,
    pub(super) craters: SparseCraterTiles,
}

impl AuthorityHeightSnapshot {
    pub(super) fn cell_and_fraction(&self, world_xz: Vec2) -> Option<([u64; 2], Vec2)> {
        if !world_xz.is_finite()
            || world_xz.cmplt(Vec2::from_array(self.spec.world_min)).any()
            || world_xz.cmpgt(Vec2::from_array(self.spec.world_max)).any()
        {
            return None;
        }
        let mut cell = [0_u64; 2];
        let mut fraction = [0.0_f32; 2];
        for axis in 0..2 {
            let logical = ((f64::from(world_xz[axis]) - f64::from(self.spec.world_min[axis]))
                / self.spec.actual_spacing_meters[axis])
                .clamp(0.0, self.spec.cells[axis] as f64);
            cell[axis] = if logical >= self.spec.cells[axis] as f64 {
                self.spec.cells[axis] - 1
            } else {
                logical.floor() as u64
            };
            fraction[axis] = (logical - cell[axis] as f64) as f32;
        }
        Some((cell, Vec2::from_array(fraction)))
    }

    fn required_pages_for_rect(
        &self,
        minimum: Vec2,
        maximum: Vec2,
    ) -> Result<Vec<SparseHeightTileKey>, String> {
        if !minimum.is_finite() || !maximum.is_finite() || minimum.cmpgt(maximum).any() {
            return Err("权威高度查询范围无效".to_owned());
        }
        let world_min = Vec2::from_array(self.spec.world_min);
        let world_max = Vec2::from_array(self.spec.world_max);
        let lower = minimum.max(world_min);
        let upper = maximum.min(world_max);
        if lower.cmpgt(upper).any() {
            return Err("权威高度查询范围位于地图外".to_owned());
        }
        let mut first = [0_u64; 2];
        let mut last = [0_u64; 2];
        for axis in 0..2 {
            let logical_min = ((f64::from(lower[axis]) - f64::from(self.spec.world_min[axis]))
                / self.spec.actual_spacing_meters[axis])
                .clamp(0.0, self.spec.cells[axis] as f64);
            let logical_max = ((f64::from(upper[axis]) - f64::from(self.spec.world_min[axis]))
                / self.spec.actual_spacing_meters[axis])
                .clamp(0.0, self.spec.cells[axis] as f64);
            let first_cell = if logical_min >= self.spec.cells[axis] as f64 {
                self.spec.cells[axis] - 1
            } else {
                logical_min.floor() as u64
            };
            let last_cell = if logical_max >= self.spec.cells[axis] as f64 {
                self.spec.cells[axis] - 1
            } else {
                logical_max.floor() as u64
            };
            first[axis] = first_cell;
            last[axis] = last_cell.saturating_add(1).min(self.spec.cells[axis]);
        }
        let (first_key, _) = self
            .spec
            .tile_for_grid_point(first[0], first[1])
            .ok_or("权威高度查询无法定位起始作者页")?;
        let (last_key, _) = self
            .spec
            .tile_for_grid_point(last[0], last[1])
            .ok_or("权威高度查询无法定位结束作者页")?;
        let count = usize::try_from(
            u64::from(last_key.x - first_key.x + 1) * u64::from(last_key.z - first_key.z + 1),
        )
        .map_err(|_| "权威高度查询页数量无法表达")?;
        if count > MAX_AUTHORITY_RECT_PAGES {
            return Err(format!(
                "单次权威高度查询需要{count}页，超过{MAX_AUTHORITY_RECT_PAGES}页局部预算"
            ));
        }
        let mut keys = Vec::with_capacity(count);
        for z in first_key.z..=last_key.z {
            for x in first_key.x..=last_key.x {
                keys.push(SparseHeightTileKey { x, z });
            }
        }
        Ok(keys)
    }

    pub(super) fn grid_height(&self, x: u64, z: u64) -> Result<Option<f32>, Arc<str>> {
        let (key, local) = self
            .spec
            .tile_for_grid_point(x, z)
            .ok_or_else(|| Arc::<str>::from("权威高度格点超出地图范围"))?;
        let Some(page) = self.pages.get(&key) else {
            return Ok(None);
        };
        let height = match page {
            AuthorityPage::Flat => 0.0,
            AuthorityPage::Tile(tile) => tile
                .get_height(&self.spec, local)
                .map(|code| self.spec.decode_height(code))
                .ok_or_else(|| Arc::<str>::from("权威高度页形状与格点归属不一致"))?,
            AuthorityPage::Failed(error) => return Err(Arc::clone(error)),
        };
        Ok(Some(sample_crater_height(
            &self.spec,
            &self.craters,
            [x, z],
            height,
        )))
    }

    pub(super) fn sample_surface(&self, world_xz: Vec2) -> ClipmapGroundHit {
        let Some((cell, fraction)) = self.cell_and_fraction(world_xz) else {
            return if world_xz.is_finite() {
                ClipmapGroundHit::Miss
            } else {
                ClipmapGroundHit::Failed
            };
        };
        let corners = [
            [cell[0], cell[1]],
            [cell[0], cell[1] + 1],
            [cell[0] + 1, cell[1] + 1],
            [cell[0] + 1, cell[1]],
        ]; // 与正式基础网格 [0,1,3]、[3,1,2] 的固定对角线一致。
        let mut heights = [0.0_f32; 4];
        for (index, point) in corners.into_iter().enumerate() {
            match self.grid_height(point[0], point[1]) {
                Ok(Some(height)) => heights[index] = height,
                Ok(None) => return ClipmapGroundHit::WaitingForHeight,
                Err(_) => return ClipmapGroundHit::Failed,
            }
        }
        let y = if fraction.x + fraction.y <= 1.0 {
            heights[0] * (1.0 - fraction.x - fraction.y)
                + heights[3] * fraction.x
                + heights[1] * fraction.y
        } else {
            heights[3] * (1.0 - fraction.y)
                + heights[1] * (1.0 - fraction.x)
                + heights[2] * (fraction.x + fraction.y - 1.0)
        };
        ClipmapGroundHit::Hit(Vec3::new(world_xz.x, y, world_xz.y))
    }

    fn rect_is_ready(&self, minimum: Vec2, maximum: Vec2) -> Result<bool, String> {
        let keys = self.required_pages_for_rect(minimum, maximum)?;
        for key in keys {
            match self.pages.get(&key) {
                Some(AuthorityPage::Flat | AuthorityPage::Tile(_)) => {}
                Some(AuthorityPage::Failed(error)) => return Err(error.to_string()),
                None => return Ok(false),
            }
        }
        Ok(true)
    }
}

#[derive(Resource, Default)]
pub(crate) struct AuthoritativeHeightRuntime {
    identity: Option<AuthorityIdentity>,
    spec: Option<SparseHeightSpec>,
    cache: BTreeMap<SparseHeightTileKey, CachedAuthorityPage>,
    pending: BTreeSet<SparseHeightTileKey>,
    loading: Option<AuthorityLoad>,
    craters: SparseCraterTiles,
    revision: u64,
    published: Option<Arc<AuthorityHeightSnapshot>>,
    request_clock: u64,
}

impl AuthoritativeHeightRuntime {
    pub(in crate::maps::dynamic_map) fn background_busy(&self) -> bool { self.loading.is_some() }
    pub(in crate::maps::dynamic_map) fn background_finished(&self, server: &AssetServer) -> bool {
        self.loading.as_ref().is_none_or(|load| !matches!(server.load_state(load.handle.id()),
            bevy::asset::LoadState::Loading | bevy::asset::LoadState::NotLoaded))
    }
    fn reset(
        &mut self,
        identity: AuthorityIdentity,
        spec: SparseHeightSpec,
        craters: SparseCraterTiles,
    ) {
        self.identity = Some(identity);
        self.spec = Some(spec);
        self.cache.clear();
        self.pending.clear();
        self.craters = craters;
        self.request_clock = 0;
        self.publish(); // 即使尚无页也发布规格，使首个点查询可以提交精确所有者页。
    }

    fn publish(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.published = self
            .identity
            .clone()
            .zip(self.spec.clone())
            .map(|(identity, spec)| {
                Arc::new(AuthorityHeightSnapshot {
                    identity,
                    revision: self.revision,
                    spec,
                    pages: self
                        .cache
                        .iter()
                        .map(|(key, page)| (*key, page.value.clone()))
                        .collect(),
                    craters: self.craters.clone(),
                })
            });
    }

    fn insert(&mut self, key: SparseHeightTileKey, value: AuthorityPage) {
        while self.cache.len() >= MAX_AUTHORITY_PAGES && !self.cache.contains_key(&key) {
            let victim = self
                .cache
                .iter()
                .filter(|(candidate, _)| {
                    Some(**candidate) != self.loading.as_ref().map(|load| load.key)
                })
                .min_by_key(|(_, page)| page.last_requested)
                .map(|(key, _)| *key);
            let Some(victim) = victim else {
                break;
            };
            self.cache.remove(&victim);
        }
        self.cache.insert(
            key,
            CachedAuthorityPage {
                value,
                last_requested: self.request_clock,
            },
        );
        self.publish();
    }

    pub(super) fn snapshot_for(
        &self,
        content: ClipmapGroundContentRevision,
    ) -> Option<Arc<AuthorityHeightSnapshot>> {
        self.published
            .as_ref()
            .filter(|snapshot| snapshot.identity.content == content)
            .cloned()
    }
}

impl DynamicClipmapGroundSurface {
    /// 提交一个局部作者精度矩形，并返回它是否已完整可查询。
    pub fn request_authoritative_rect(&self, minimum: Vec2, maximum: Vec2) -> Result<bool, String> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| "当前Clipmap地表快照尚未就绪".to_owned())?;
        snapshot.request_authoritative_rect(minimum, maximum)
    }
}

impl super::ground::ClipmapGroundSnapshot {
    pub fn request_authoritative_rect(&self, minimum: Vec2, maximum: Vec2) -> Result<bool, String> {
        if !self.ready {
            return Ok(false);
        }
        let authority = self
            .authority
            .as_ref()
            .ok_or_else(|| "权威高度页缓存尚未绑定当前地表".to_owned())?;
        if authority.identity.content != self.content_revision {
            return Ok(false);
        }
        let keys = authority.required_pages_for_rect(minimum, maximum)?;
        if authority.rect_is_ready(minimum, maximum)? {
            return Ok(true);
        }
        let _ = self.authority_requests.request(&authority.identity, &keys); // 队列满时保持等待并在下一帧重试。
        Ok(false)
    }

    pub(super) fn authoritative_segment_is_known(&self, from: Vec2, to: Vec2) -> bool {
        self.authority
            .as_ref()
            .filter(|authority| authority.identity.content == self.content_revision)
            .is_some_and(|authority| {
                authority.rect_is_ready(from.min(to), from.max(to)) == Ok(true)
            })
    }
}

/// 轮询唯一基础页 IO，采用实时作者页或合法隐式平地，并发布不可变局部快照。
pub(crate) fn drive_authoritative_height_pages(
    asset_server: Res<AssetServer>,
    manifests: Res<Assets<SparseHeightManifestAsset>>,
    tiles: Res<Assets<SparseHeightTileAsset>>,
    live: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<SparseHeightLiveOverride>,
    craters: InstanceRes<SparseHeightSessionCrater>,
    pages: InstanceRes<SparseHeightPageRuntime>,
    surface: InstanceRes<DynamicClipmapGroundSurface>,
    mut runtime: InstanceResMut<AuthoritativeHeightRuntime>,
) {
    let Some((map_id, manifest_handle, content)) = pages.authority_source() else {
        surface.authority_requests.clear();
        return; // 显示内容尚未完整采用时不从未来草稿拼出权威页。
    };
    let Some(manifest_asset) = manifests.get(&manifest_handle) else {
        return;
    };
    let manifest = &manifest_asset.manifest;
    if manifest.map_id != map_id {
        return; // 正式分页系统会发布具体清单错误，本域不重复污染状态。
    }
    let identity = AuthorityIdentity {
        map_id: map_id.clone(),
        content,
    };
    let crater_tiles =
        if content.crater == Some(craters.revision()) && craters.map_id() == Some(&map_id) {
            craters.tiles().clone()
        } else {
            BTreeMap::new()
        };
    if runtime.identity.as_ref() != Some(&identity) {
        runtime.reset(identity.clone(), manifest.spec.clone(), crater_tiles);
    }
    surface.authority_requests.retarget(identity.clone());
    runtime.request_clock = runtime.request_clock.wrapping_add(1).max(1);
    let request_clock = runtime.request_clock;
    for key in surface.authority_requests.drain(&identity) {
        if let Some(page) = runtime.cache.get_mut(&key) {
            page.last_requested = request_clock;
        } else if runtime.pending.len() < MAX_AUTHORITY_PAGES {
            runtime.pending.insert(key);
        }
    }

    if let Some(load) = runtime.loading.take() {
        match asset_server.load_state(load.handle.id()) {
            LoadState::NotLoaded | LoadState::Loading => {
                runtime.loading = Some(load);
                return; // 旧修订 IO 也占用唯一槽，直到真实结束。
            }
            LoadState::Failed(_) => {
                if load.identity == identity {
                    let message: Arc<str> =
                        format!("权威高度页({}, {})加载失败", load.key.x, load.key.z).into();
                    error!("地图{}的{}", map_id, message);
                    runtime.insert(load.key, AuthorityPage::Failed(message));
                }
            }
            LoadState::Loaded => {
                if load.identity == identity {
                    let value = tiles.get(&load.handle).map_or_else(
                        || Err("AssetServer报告权威高度页已加载但资产不可读取".to_owned()),
                        |asset| {
                            let reference = manifest
                                .tile_ref(load.key)
                                .ok_or_else(|| "权威高度页已不在当前清单中".to_owned())?;
                            if reference.sha256 != load.sha256 {
                                return Err("权威高度页摘要身份已经变化".to_owned());
                            }
                            asset.validate_against(manifest, reference)?;
                            Ok(AuthorityPage::Tile(Arc::clone(&asset.tile)))
                        },
                    );
                    match value {
                        Ok(value) => runtime.insert(load.key, value),
                        Err(error) => {
                            error!(
                                "地图{}的权威高度页({}, {})无效：{}",
                                map_id, load.key.x, load.key.z, error
                            );
                            runtime.insert(load.key, AuthorityPage::Failed(error.into()));
                        }
                    }
                }
            }
        }
    }
    if runtime.loading.is_some() {
        return;
    }

    while let Some(key) = runtime.pending.pop_first() {
        if runtime.cache.contains_key(&key) {
            continue;
        }
        if content.live == Some(live.revision()) && live.map_id() == Some(&map_id) {
            if let Some(tile) = live.tile(key) {
                if tile.key != key || manifest.spec.tile_shape(key) != Some(tile.shape) {
                    runtime.insert(
                        key,
                        AuthorityPage::Failed("实时权威高度页身份或形状无效".into()),
                    );
                } else {
                    runtime.insert(key, AuthorityPage::Tile(Arc::clone(tile)));
                }
                continue; // 内存页不占 IO，可在固定六十四页上限内立即采用。
            }
        }
        let Some(reference) = manifest.tile_ref(key) else {
            runtime.insert(key, AuthorityPage::Flat);
            continue; // 清单缺席明确表示合法世界 Y=0，不创建空白页。
        };
        let handle = asset_server.load(manifest.tile_asset_path(key, &reference.sha256));
        runtime.loading = Some(AuthorityLoad {
            identity: identity.clone(),
            key,
            sha256: reference.sha256.clone(),
            handle,
        });
        break; // 正式磁盘页始终只有一个在途句柄。
    }
}

pub(crate) fn clear_authoritative_height_pages(
    surface: InstanceRes<DynamicClipmapGroundSurface>,
    mut runtime: InstanceResMut<AuthoritativeHeightRuntime>,
) {
    surface.authority_requests.clear();
    let loading = runtime.loading.take(); // 已启动 AssetServer IO 可自然结束，但不再保留结果身份。
    *runtime = AuthoritativeHeightRuntime {
        loading,
        ..default()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(heights: [u32; 4]) -> AuthorityHeightSnapshot {
        let map_id = MapId::parse("MAP-authority-test").unwrap();
        let spec = SparseHeightSpec::new([0.0, 0.0], [64.0, 64.0], 64.0, -10.0, 20.0).unwrap();
        let key = SparseHeightTileKey { x: 0, z: 0 };
        let mut tile = SparseHeightTile::blank(&spec, key).unwrap();
        for (point, height) in [
            ([0, 0], heights[0]),
            ([0, 1], heights[1]),
            ([1, 1], heights[2]),
            ([1, 0], heights[3]),
        ] {
            tile.set(point, height).unwrap();
        }
        AuthorityHeightSnapshot {
            identity: AuthorityIdentity {
                map_id,
                content: ClipmapGroundContentRevision {
                    root: 1,
                    manifest: 2,
                    live: None,
                    crater: None,
                    topology: 3,
                },
            },
            revision: 1,
            spec,
            pages: BTreeMap::from([(key, AuthorityPage::Tile(Arc::new(tile)))]),
            craters: BTreeMap::new(),
        }
    }

    #[test]
    fn authoritative_surface_uses_fixed_base_mesh_diagonal() {
        let low = SparseHeightSpec::new([0.0, 0.0], [64.0, 64.0], 64.0, -10.0, 20.0)
            .unwrap()
            .encode_height(0.0)
            .unwrap();
        let high = SparseHeightSpec::new([0.0, 0.0], [64.0, 64.0], 64.0, -10.0, 20.0)
            .unwrap()
            .encode_height(8.0)
            .unwrap();
        let surface = snapshot([low, low, high, low]); // 只有右下角抬高，形成可辨鞍形。
        let ClipmapGroundHit::Hit(first) = surface.sample_surface(Vec2::new(16.0, 16.0)) else {
            panic!("第一三角形应命中");
        };
        let ClipmapGroundHit::Hit(second) = surface.sample_surface(Vec2::new(48.0, 48.0)) else {
            panic!("第二三角形应命中");
        };
        assert!(first.y.abs() < 0.001); // [0,1,3] 不读取右下角。
        assert!((second.y - 4.0).abs() < 0.001); // [3,1,2] 按实际三角面线性插值。
    }

    #[test]
    fn authoritative_surface_distinguishes_waiting_flat_and_failure() {
        let mut surface = snapshot([0; 4]);
        surface.pages.clear();
        assert_eq!(
            surface.sample_surface(Vec2::splat(16.0)),
            ClipmapGroundHit::WaitingForHeight
        );
        surface
            .pages
            .insert(SparseHeightTileKey { x: 0, z: 0 }, AuthorityPage::Flat);
        assert_eq!(
            surface.sample_surface(Vec2::splat(16.0)),
            ClipmapGroundHit::Hit(Vec3::new(16.0, 0.0, 16.0))
        );
        surface.pages.insert(
            SparseHeightTileKey { x: 0, z: 0 },
            AuthorityPage::Failed("坏页".into()),
        );
        assert_eq!(
            surface.sample_surface(Vec2::splat(16.0)),
            ClipmapGroundHit::Failed
        );
    }

    #[test]
    fn unbounded_float_authority_samples_real_height_without_legacy_clamp() {
        for meters in [-384.5_f32, 512.25] {
            let surface = snapshot([meters.to_bits(); 4]);
            assert_eq!(surface.sample_surface(Vec2::splat(16.0)),
                ClipmapGroundHit::Hit(Vec3::new(16.0, meters, 16.0)));
        }
    }

    #[test]
    fn authoritative_surface_uses_unique_page_owners_on_rectangular_map_edges() {
        let map_id = MapId::parse("MAP-authority-page-owner").unwrap();
        let spec = SparseHeightSpec::new([0.0, 0.0], [128.0, 75.0], 0.25, -20.0, 40.0).unwrap(); // 512×300单元同时覆盖内部页边和非方形尾页。
        let code = |height| spec.encode_height(height).unwrap();
        let mut left = SparseHeightTile::blank(&spec, SparseHeightTileKey { x: 0, z: 0 }).unwrap();
        let mut right = SparseHeightTile::blank(&spec, SparseHeightTileKey { x: 1, z: 0 }).unwrap();
        for z in [40, 41] {
            left.set([255, z], code(2.0)).unwrap(); // 内部共享边左侧最后一个所有者格点。
            right.set([0, z], code(6.0)).unwrap(); // 全局格点256只归右页local 0。
            right.set([1, z], code(8.0)).unwrap();
            right.set([255, z], code(10.0)).unwrap();
            right.set([256, z], code(14.0)).unwrap(); // X最大端点由最后一页额外拥有。
        }
        let mut corner =
            SparseHeightTile::blank(&spec, SparseHeightTileKey { x: 1, z: 1 }).unwrap();
        corner.set([256, 44], code(18.0)).unwrap(); // 非方形地图右下端点(512,300)。
        let content = ClipmapGroundContentRevision {
            root: 1,
            manifest: 2,
            live: None,
            crater: None,
            topology: 3,
        };
        let surface = AuthorityHeightSnapshot {
            identity: AuthorityIdentity { map_id, content },
            revision: 1,
            spec,
            pages: BTreeMap::from([
                (
                    SparseHeightTileKey { x: 0, z: 0 },
                    AuthorityPage::Tile(Arc::new(left)),
                ),
                (
                    SparseHeightTileKey { x: 1, z: 0 },
                    AuthorityPage::Tile(Arc::new(right)),
                ),
                (
                    SparseHeightTileKey { x: 1, z: 1 },
                    AuthorityPage::Tile(Arc::new(corner)),
                ),
            ]),
            craters: BTreeMap::new(),
        };

        let ClipmapGroundHit::Hit(bridged) = surface.sample_surface(Vec2::new(63.9375, 10.125))
        else {
            panic!("跨内部页边的基础三角形应命中");
        };
        assert!((bridged.y - 5.0).abs() < 0.001); // 同一三角面从左右唯一所有者页各取顶点。
        let ClipmapGroundHit::Hit(page_edge) =
            surface.sample_surface(Vec2::new(64.0, 10.125))
        else {
            panic!("精确内部页边应命中右页");
        };
        assert!((page_edge.y - 6.0).abs() < 0.001); // 精确页边从右页开始新的基础单元。
        let ClipmapGroundHit::Hit(x_edge) =
            surface.sample_surface(Vec2::new(128.0, 10.125))
        else {
            panic!("地图X最大边应命中最后一页");
        };
        assert!((x_edge.y - 14.0).abs() < 0.001); // X最大边不越界或回退到前一页。
        let ClipmapGroundHit::Hit(corner_hit) =
            surface.sample_surface(Vec2::new(128.0, 75.0))
        else {
            panic!("非方形地图右下端点应命中尾页");
        };
        assert!((corner_hit.y - 18.0).abs() < 0.001); // 尾页额外拥有最终格点；容差仅覆盖R16量化。

        let keys = surface
            .required_pages_for_rect(Vec2::new(63.875, 10.0), Vec2::new(64.0, 10.25))
            .unwrap();
        assert_eq!(
            keys,
            vec![
                SparseHeightTileKey { x: 0, z: 0 },
                SparseHeightTileKey { x: 1, z: 0 },
            ]
        ); // 预取矩形必须包含跨边三角形的两个所有者页。
    }
}
