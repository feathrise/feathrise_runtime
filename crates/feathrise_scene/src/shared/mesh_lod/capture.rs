//! 正式场景保留来源票据；属性捕获与独立作者视口共用同一有界实现。
use super::types::SourceKey;
use bevy::prelude::Mesh;
use feathrise_terrain::StaticMeshCapture;

pub(super) struct Capture {
    pub key: SourceKey,
    inner: StaticMeshCapture,
}

impl Capture {
    pub fn new(key: SourceKey, source: &Mesh) -> Result<Self, String> {
        Ok(Self { key, inner: StaticMeshCapture::new(source)? })
    }
    pub fn step(&mut self, source: &Mesh, bytes: usize) -> Result<bool, String> {
        self.inner.step(source, bytes)
    }
    pub fn finish(self) -> Result<Mesh, String> { self.inner.finish() }
}

pub(super) use feathrise_terrain::static_mesh_bytes as mesh_bytes;