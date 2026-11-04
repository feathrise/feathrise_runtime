//! 中性关键资产失败判定；具体角色与技能的预载清单由原游戏构造。

use bevy::{asset::UntypedHandle, prelude::*};

/// 保留原资产自身、直接及递归依赖三层失败检查；句柄尚未登记不等同成功。
pub fn has_failed_asset(asset_server: &AssetServer, handles: &[UntypedHandle]) -> bool {
    let mut failed = false;
    for handle in handles {
        let Some((load, dependencies, recursive_dependencies)) =
            asset_server.get_load_states(handle)
        else {
            continue;
        };
        if load.is_failed() || dependencies.is_failed() || recursive_dependencies.is_failed() {
            warn!("关键加载资源失败: {:?}", handle.path());
            failed = true;
        }
    }
    failed
}
