//! 布局就绪后的局部源页租约、复用身份与后台烘焙派发。
use super::super::resources::DynamicSceneRuntimeStatus;
use super::{build, driver::LayeredAssets, lifecycle::fail, resources::*}; // 不复制规划或采样逻辑。
use bevy::{asset::LoadState, tasks::AsyncComputeTaskPool}; // 有界异步IO和CPU构造。
use feathrise_scene_content::sparse_height::{
    SparseHeightLiveOverride, SparseHeightSessionCrater,
}; // 内容寻址源页。
use feathrise_scene_content::{SceneCollisionLodStatus, SparseHeightManifest}; // 完整来源与诊断。
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
}; // 只浅克隆大型不可变源。 // 错误回显。
const MAX_LOADING_PAGES: usize = 4; // 本域同时请求的未完成源页上限，避免一次向AssetServer突发512个任务。
pub(super) fn load_and_cook(
    assets: &LayeredAssets,
    state: &mut LayeredCollisionRuntime,
    desired: Arc<Request>,
    manifest: &SparseHeightManifest,
    live: Option<&SparseHeightLiveOverride>,
    craters: Option<&SparseHeightSessionCrater>,
    feedback: &mut SceneCollisionLodStatus,
    status: &mut DynamicSceneRuntimeStatus,
) {
    // 不改变全局请求身份。
    if super::retirement::busy(state) {
        return;
    } // 独立入口也遵守退役背压，禁止旧缓存未释放就启动下一轮烘焙。
    let scene_id = desired.scene_id.clone(); // 同一请求的来源ID。
    let ticket = state.ticket; // 当前唯一票据。
    let needed = state.layout.as_ref().expect("布局存在").tiles.clone(); // 最多512个小键。
    state.leases.retain(|key, lease| {
        needed.contains(key)
            || matches!(
                assets.server.load_state(lease.handle.id()),
                LoadState::NotLoaded | LoadState::Loading
            )
    }); // 换需求时未完成IO仍占槽，完成后才释放无关租约。
    let mut loading = state
        .leases
        .values()
        .filter(|lease| {
            matches!(
                assets.server.load_state(lease.handle.id()),
                LoadState::NotLoaded | LoadState::Loading
            )
        })
        .count(); // 上一需求仍在加载的页也计入背压。
    let mut sources = BTreeMap::new(); // 全部源只克隆Arc。
    let mut ready = true; // 请求所有有界页后统一等待，避免逐页串行IO。
    for key in &needed {
        // 不扫描整个场景索引。
        if let Some(tile) = live.and_then(|live| live.tile(*key)) {
            sources.insert(*key, Arc::clone(tile));
            continue;
        } // 显式平地覆盖优先。
        let Some(reference) = manifest.tile_ref(*key) else {
            continue;
        }; // 清单缺席才是真正的隐式平地。
        if state
            .leases
            .get(key)
            .is_none_or(|lease| lease.scene_id != scene_id || lease.hash != reference.sha256)
        {
            if state.leases.get(key).is_some_and(|lease| {
                matches!(
                    assets.server.load_state(lease.handle.id()),
                    LoadState::NotLoaded | LoadState::Loading
                )
            }) {
                ready = false;
                continue;
            } // 同键旧内容或旧场景加载实际结束后才换租约，不能伪装取消释放槽位。
            if loading >= MAX_LOADING_PAGES {
                ready = false;
                continue;
            } // 等待已请求源页，不阻塞主线程或暗中使用平地。
            state.leases.insert(
                *key,
                Lease {
                    scene_id: scene_id.clone(), // 页坐标跨场景不构成相同资产身份。
                    hash: reference.sha256.clone(),
                    handle: assets.server.load(manifest.tile_asset_path(*key, &reference.sha256)),
                },
            );
            loading += 1; // 同帧剩余请求共享四个IO槽；实际缓存命中下帧归还额度。
        } // 内容寻址强句柄。
        let lease = state.leases.get(key).expect("页租约已建立"); // 同一循环不会移除。
        if matches!(
            assets.server.load_state(lease.handle.id()),
            LoadState::Failed(_)
        ) {
            fail(
                state,
                feedback,
                status,
                format!("碰撞源页({}, {})加载失败", key.x, key.z),
            );
            return;
        } // 非平地失败不回退零高度。
        if let Some(asset) = assets.tiles.get(&lease.handle) {
            // 只接纳已校验且完整的块。
            if let Err(error) = asset.validate_against(manifest, reference) {
                fail(state, feedback, status, error);
                return;
            } // 摘要和场景均验证。
            sources.insert(*key, Arc::clone(&asset.tile)); // 与资产世界共享高度数组。
        } else {
            ready = false;
        } // 后续页仍继续提出需求。
    } // 批次未齐不启动cooking。
    if !ready {
        return;
    } // 主循环照常运行。
    let crater_tiles = craters.map_or_else(BTreeMap::new, |session| {
        needed
            .iter()
            .filter_map(|key| {
                session
                    .tiles()
                    .get(key)
                    .map(|tile| (*key, Arc::clone(tile)))
            })
            .collect()
    }); // 只冻结本窗口破坏。
    let layout = state.layout.take().expect("完整候选布局"); // 唯一所有权交后台，不深拷贝几何。
    let mut stamps = BTreeMap::new(); // 每块独立内容身份。
    let mut reusable = BTreeSet::new(); // 保留未变化形状。
    for (key, chunk) in &layout.chunks {
        // 最多512个块，页集合局部有界。
        let stamp = SourceStamp {
            sources: chunk
                .tiles
                .iter()
                .map(|tile_key| {
                    // 正式摘要和覆盖身份共同决定高度。
                    let live_ptr = live
                        .and_then(|live| live.tile(*tile_key))
                        .map_or(0, |tile| Arc::as_ptr(tile) as usize); // 实时块替换必换身份。
                    let crater_ptr = crater_tiles
                        .get(tile_key)
                        .map_or(0, |tile| Arc::as_ptr(tile) as usize); // 只重建实际变化坑块。
                    let hash = if live_ptr == 0 {
                        manifest
                            .tile_ref(*tile_key)
                            .map_or_else(String::new, |reference| reference.sha256.clone())
                    } else {
                        String::new()
                    }; // 草稿遮盖正式页时保存不导致重复重建。
                    (*tile_key, hash, live_ptr, crater_ptr) // 不依赖全局修订导致全图烘焙。
                })
                .collect(),
        }; // 源集合完整。
        if state
            .resident
            .get(key)
            .is_some_and(|old| old.plan == chunk.plan && old.stamp == stamp)
        {
            reusable.insert(*key);
        } // 邻接变化必须重建接缝。
        stamps.insert(*key, stamp); // 构造任务持有完整签名。
    } // 内部源循环不采样格点。
    state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        TaskResult {
            ticket,
            request: Arc::clone(&desired), // 形状完成值与布局阶段沿用同一完整需求身份。
            completion: Completion::Cooked(
                build::cook_parallel(
                    Arc::clone(&desired), // 所有worker共享同一冻结请求身份。
                    layout,
                    stamps,
                    reusable,
                    sources,
                    crater_tiles,
                    AsyncComputeTaskPool::get(), // 复用既有CPU池，不额外建立线程。
                    super::parallel::MAX_WORKERS, // 与布局阶段共用并行上限。
                )
                .await,
            ), // await只发生在后台，完整形状集合统一交付。
        }
    })); // 高度采样和Avian结构全部后台完成。
} // 完整源到齐后仅派发一个CPU任务。
