//! 通过数据预设的正式层身份解析共享F3/F4内容，GPU局部槽不承担草种语义。

use super::lod_controls::{LodControlContent, LodControlPresetBindings};
use feathrise_environment::vegetation::VegetationBladeLayer;
use feathrise_map_content::VegetationPresetRegistry;
pub(super) fn content_for_blade_layer(
    layer: &VegetationBladeLayer,
    presets: Option<&VegetationPresetRegistry>,
    bindings: Option<&LodControlPresetBindings>,
) -> Option<LodControlContent> {
    let presets = presets?;
    // 只查询工程显式绑定的固定诊断槽；不猜测 GPU 局部槽或素材名字。
    bindings?.iter()
    .find_map(|(content, id)| {
        presets
            .resolve(id)
            .filter(|preset| preset.resolved_runtime_layer_id() == layer.0.0)
            .map(|_| content)
    })
}
