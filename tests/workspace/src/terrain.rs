//! 完整地表与真实 Clipmap 高度库的跨包 Shader 契约，复用原预处理与全部管线变体。

use super::terrain_support::*;

#[test] // Forward、普通Deferred和携带光追三角身份的Deferred都必须保留合法完整雨天地表片元。
fn wet_terrain_full_fragment_variants_are_valid_wgsl() {
    // 无GPU无窗口测试全部生产接线。
    let variants: &[&[&str]] = &[
        &[],
        &["VERTEX_COLORS", "VERTEX_UVS_B"],
        &["VERTEX_COLORS", "VERTEX_UVS_B", "PREPASS_PIPELINE"],
        &["PREPASS_PIPELINE"],
        &["PREPASS_PIPELINE", "TERRAIN_SURFACE_PRIMITIVE_INDEX"],
        &["SPARSE_CLIPMAP"],
        &["SPARSE_CLIPMAP", "PREPASS_PIPELINE"],
        &[
            "SPARSE_CLIPMAP",
            "PREPASS_PIPELINE",
            "TERRAIN_SURFACE_PRIMITIVE_INDEX",
        ],
    ]; // 同时验证实际嵌套 Clipmap 的 Forward、普通 Deferred 与光追元数据。
    for defines in variants {
        // 每个变体包含固定层和索引层两个运行时分支。
        let directive = if defines.contains(&"TERRAIN_SURFACE_PRIMITIVE_INDEX") {
            "enable primitive_index;\n"
        } else {
            ""
        }; // 与实际Deferred资产使用相同扩展指令。
        let clipmap = if defines.contains(&"SPARSE_CLIPMAP") {
            // 与正式load_height_shader相同，先拼接共享参数/拼接函数，再验证高度库；不能用缺少宿主ABI的半份源码。
            format!(
                "{}\n{}",
                feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false).replace("#{MATERIAL_BIND_GROUP}", "2"),
                preprocess(include_str!("../../../crates/feathrise_scene/src/maps/dynamic_map/clipmap/shaders/height.wesl"), defines),
            )
        } else {
            String::new()
        }; // 只替换外部 Bevy 类型，世界格映射必须验证真实生产高度库。
        let source = format!(
            "{directive}{clipmap}\n{}\n{}\n{}\n{}\n{}",
            preprocess(PBR_STUB, defines),
            format!(
                "{}\n{}",
                include_str!("../../../crates/feathrise_environment/src/terrain_surface/shaders/rain_shelter.wesl"),
                preprocess(WEATHER, defines)
            ),
            preprocess(RAIN, defines),
            preprocess(WET, defines),
            preprocess(TERRAIN, defines).replace("clipmap_height::", "")
        ); // 生产库、共享ABI和完整片元依次组合。
        validate(&source, &format!("完整雨天地表{defines:?}")); // 错误必须报告具体通道身份。
    } // 完成六种生产管线校验。
} // 结束完整Forward/Deferred集成回归。
