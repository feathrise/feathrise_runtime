//! v3持久作者层：历史只保留索引根；IO和计算由调用方分别放到有界任务池。
mod codec;
mod display;
mod edit;
mod fields;
mod field_stream; // 原生体素按观察/玩家附近稀疏目录读取，工作集与整对象总量无关。
pub use field_stream::{read_native_surface_fields,read_native_surface_fields_with_budget};
pub use fields::{VolumeFieldReadPlan,VolumeFieldBytes,plan_surface_fields,read_surface_fields,decode_surface_fields};
pub use fields::plan_surface_field_windows;
mod lod;
mod residency;
mod store;
mod types;
pub use codec::{decode_paged_volume, encode_paged_volume};
pub use display::{load_display_volume_at_foci, load_display_volume_at_foci_with_progress};
pub use edit::{load_edit_batch_window, load_edit_window, prepare_delta, write_delta};
pub use residency::{ResidentVolume, build_resident_volume, load_resident_volume};
pub use residency::{is_residency_budget_error, load_resident_volume_at_foci};
pub use store::validate_roots;
pub use types::{PagedVolume, VolumeDelta};
pub use lod::{VolumeDisplayLod, apply_display_lods};
pub use fields::{field_paint_response, VolumeFieldBrush, VolumeFieldChannel, VolumeFieldSample, VolumeFieldSnapshot,
    VolumeFieldWindow, VolumeFieldDelta, load_surface_fields, load_field_window, prepare_field_paint, write_field_paint};
#[cfg(test)]
mod tests;
