//! 应用完成内容准备后，一次提交本次加载必须等待的强句柄。

use bevy::{asset::UntypedHandle, prelude::*};
use feathrise_scene_core::state::{LoadingTransactionId, PendingSceneTransition, ScenePhase};

/// 资源缺席表示应用前置步骤尚未完成；有效的空清单表示程序生成资产无需外部加载。
#[derive(Resource, Default)]
pub struct CriticalSceneAssets {
    pub handles: Vec<UntypedHandle>,
    request: Option<(LoadingTransactionId, ScenePhase)>,
}

impl CriticalSceneAssets {
    /// 清单由提交它的应用绑定到实际请求；空清单同样必须有明确身份。
    pub fn for_transition(request: &PendingSceneTransition, handles: Vec<UntypedHandle>) -> Self {
        Self { handles, request: Some((request.transaction(), request.target)) }
    }

    pub fn is_for(&self, request: &PendingSceneTransition) -> bool {
        self.request == Some((request.transaction(), request.target))
    }
}

/// 应用每帧先关闭，再在PAK/工厂/目录等自身条件均满足后放行；资产句柄本身不能代替这些条件。
#[derive(Resource, Default)]
pub struct SceneLoadingPrerequisites {
    application_request: Option<(LoadingTransactionId, ScenePhase)>,
    /// 应用拥有的上一场景资源尚未完成退出，关键资产已加载也不能越过此门。
    pub retirement_pending: bool,
}

impl SceneLoadingPrerequisites {
    /// 应用撤销自己本帧的许可，不覆盖由角色/其他领域独立发布的退出门。
    pub fn block_application(&mut self) { self.application_request = None; }

    pub fn allow_transition(&mut self, request: &PendingSceneTransition) {
        self.application_request = Some((request.transaction(), request.target));
    }

    pub fn ready_for(&self, request: &PendingSceneTransition) -> bool {
        self.application_request == Some((request.transaction(), request.target))
    }
}
