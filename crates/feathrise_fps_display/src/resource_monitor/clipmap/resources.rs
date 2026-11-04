//! Clipmap诊断只保存计数与六个短标签；一次快照的内存不随地图网格数量增长。

use bevy::prelude::*;
use feathrise_map_content::{MapId, MapTerrainDisplayLodStatus, MapTerrainDisplayLodWork};

pub(super) const ROW_LIMIT: usize = 6;

#[derive(Default)]
pub(super) struct Snapshot {
    pub count: u64,
    pub vertices: u64,
    pub triangles: u64,
    pub mesh_bytes: u64,
    pub pages: u64,
    pub work: MapTerrainDisplayLodWork,
    pub rows: Vec<String>,
}

impl Snapshot {
    pub fn observe(&mut self, label: &str, status: &MapTerrainDisplayLodStatus) {
        if status.map_id.is_none() { return; }
        self.count += 1;
        self.vertices = self.vertices.saturating_add(status.vertex_count as u64);
        self.triangles = self.triangles.saturating_add(status.triangle_count as u64);
        self.mesh_bytes = self.mesh_bytes.saturating_add(status.mesh_bytes);
        self.pages = self.pages.saturating_add(status.resident_height_pages as u64);
        macro_rules! add_work { ($($field:ident),*) => { $(
            self.work.$field = self.work.$field.saturating_add(status.work.$field);
        )* }; }
        add_work!(mesh_builds, mesh_discarded, mesh_uploads, page_uploads, display_adoptions,
            mesh_upload_bytes, page_upload_bytes);
        if self.rows.len() < ROW_LIMIT {
            let label: String = label.chars().take(24).collect();
            self.rows.push(format!("{label}：{}顶点 / {}三角\n  构建 {:.2}ms · 预热等待 {:.1}ms · 重建/上传 {}/{}",
                status.vertex_count, status.triangle_count, status.build_milliseconds,
                status.upload_wait_milliseconds, status.work.mesh_builds, status.work.mesh_uploads));
        }
    }

    pub fn report(&self) -> String {
        if self.count == 0 { return String::new(); }
        let mut report = format!("\n\nClipmap · {}个网格（当前完整显示）\n顶点 {} · 三角 {} · Mesh {:.2}MiB · 高度页 {}\n累计重建/丢弃 {}/{} · Mesh/页上传 {}/{} · 采用 {}\n累计登记 Mesh {:.2}MiB / 高度页 {:.2}MiB",
            self.count, self.vertices, self.triangles, self.mesh_bytes as f64 / 1048576.0, self.pages,
            self.work.mesh_builds, self.work.mesh_discarded, self.work.mesh_uploads,
            self.work.page_uploads, self.work.display_adoptions,
            self.work.mesh_upload_bytes as f64 / 1048576.0, self.work.page_upload_bytes as f64 / 1048576.0);
        for row in &self.rows { report.push('\n'); report.push_str(row); }
        if self.count > self.rows.len() as u64 {
            report.push_str(&format!("\n另{}个网格已计入总数", self.count - self.rows.len() as u64));
        }
        report.push_str("\n预热等待为墙钟时间；登记字节不代表显存占用。");
        report
    }
}

#[derive(Resource, Default)]
pub(in crate::resource_monitor) struct ClipmapMonitorState {
    pub(super) identity: Option<(MapId, Option<Entity>)>,
    pub(super) cursor: Option<Entity>,
    pub(super) sampling: bool,
    pub(super) building: Snapshot,
    pub(in crate::resource_monitor) report: String,
    pub(super) next_sample: f64,
}
