//! 本轮补丁请求与包身份；源码代次、地图代次和运行代次保持独立。
use feathrise_editor_protocol::Message;
use serde::Deserialize;

#[derive(Clone, Debug)]
pub(super) struct Request {
    pub patch_id: u64,
    pub source_generation: u64,
    pub base_build: String,
    pub artifact: String,
    pub sha256: String,
    pub project_id: String,
    pub session: u64,
    pub map_generation: u64,
    pub pid: u32,
    pub world_id: u64,
    pub aslr_reference: u64,
}

#[derive(Debug)]
pub(super) struct Outcome {
    pub request: Request,
    pub result: Result<(), String>,
}

impl Outcome {
    pub fn message(&self, epoch: u64) -> Message {
        Message::CodePatchResult { patch_id: self.request.patch_id,
            source_generation: self.request.source_generation, base_build: self.request.base_build.clone(),
            pid: self.request.pid, world_id: self.request.world_id, epoch,
            applied: self.result.is_ok(), reason: self.result.as_ref().err().cloned() }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
    pub project_id: String,
    pub session: u64,
    pub pid: u32,
    pub world_id: u64,
    pub base_build: String,
    pub source_generation: u64,
    pub patch_id: u64,
}

impl Target {
    pub fn matches(&self, request: &Request) -> bool {
        self.project_id == request.project_id && self.session == request.session
            && self.pid == request.pid && self.world_id == request.world_id
            && self.base_build == request.base_build && self.source_generation == request.source_generation
            && self.patch_id == request.patch_id
    }
}
