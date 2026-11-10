//! 场景细节质量的纯范围派生规则。

use std::ops::Range; // Bevy 可见范围用两个浮点区间表达宽域交叉淡化。

use bevy::camera::visibility::VisibilityRange; // 输入输出都复用 Bevy 原生距离 LOD 组件。
use feathrise_settings::SceneDetailQuality; // 档位提供 LOD 边界与最大范围两个受限倍率。

/// 按固定倍率缩放一段作者距离区间。
pub(super) fn scaled_margin(authored: &Range<f32>, multiplier: f32) -> Range<f32> {
    authored.start * multiplier..authored.end * multiplier // 同时缩放淡入或淡出宽域的两端。
} // 完成单个宽域区间派生。

/// 从不可变作者范围派生目标档位，最远档的退出边界单独服从最大显示距离倍率。
pub(super) fn visibility_range_for_quality(
    authored: &VisibilityRange,  // 首次接管时保存的场景或原型原始范围。
    farthest_lod: bool,          // 树公告板、景观超低档或通用第 4 档才为 true。
    quality: SceneDetailQuality, // 当前持久场景细节档。
) -> VisibilityRange {
    if quality == SceneDetailQuality::High {
        return authored.clone(); // 高档严格恢复作者值，避免单位浮点运算引入任何基线差异。
    } // 完成高档精确恢复快速路径。
    let (lod_multiplier, view_distance_multiplier) = quality.runtime_multipliers(); // 取得受设计表约束的有限倍率。
    VisibilityRange {
        start_margin: scaled_margin(&authored.start_margin, lod_multiplier), // 所有 LOD 交接边界随精细度档位变化。
        end_margin: scaled_margin(
            &authored.end_margin,
            if farthest_lod {
                view_distance_multiplier // 最远档只在低/中缩短消失距离，极高不得突破作者上限。
            } else {
                lod_multiplier // 其他档的淡出与下一档淡入保持完全相同倍率。
            },
        ),
        use_aabb: authored.use_aabb, // 测距原点所有权继续归内容作者，不由画质系统改变。
    } // 完成不会累计缩放的目标范围构造。
} // 完成场景细节档位范围派生。

#[cfg(test)]
mod tests {
    use super::*; // 直接验证纯倍率与相邻 LOD 连续性契约。

    fn range(start: (f32, f32), end: (f32, f32)) -> VisibilityRange {
        VisibilityRange {
            start_margin: start.0..start.1, // 构造可辨识淡入宽域。
            end_margin: end.0..end.1,       // 构造可辨识淡出宽域。
            use_aabb: false,                // HN-01 固定景观全部按根位置测距。
        } // 完成最小作者范围 fixture。
    } // 完成测试范围构造纯函数。

    fn assert_margin_close(actual: &Range<f32>, expected: Range<f32>) {
        assert!((actual.start - expected.start).abs() <= 0.0001); // 容纳十进制倍率的单精度末位误差。
        assert!((actual.end - expected.end).abs() <= 0.0001); // 设计米制边界仍保持万分之一米精度。
    } // 完成测试专用浮点区间近似断言。

    #[test]
    fn low_and_medium_scale_lod_and_farthest_distance_together() {
        let authored = range((300.0, 360.0), (480.0, 560.0)); // 使用岩石最远档代表范围。
        let low = visibility_range_for_quality(&authored, true, SceneDetailQuality::Low);
        assert_margin_close(&low.start_margin, 180.0..216.0); // 最远档进入边界缩到作者值六成。
        assert_margin_close(&low.end_margin, 288.0..336.0); // 低档最远显示距离同样缩到六成。
        let medium = visibility_range_for_quality(&authored, true, SceneDetailQuality::Medium);
        assert_margin_close(&medium.start_margin, 240.0..288.0); // 中档 LOD 交接使用八成边界。
        assert_margin_close(&medium.end_margin, 384.0..448.0); // 中档最远距离同步使用八成。
    } // 完成低中档双倍率一致性覆盖。

    #[test]
    fn ultra_extends_lod_transitions_without_exceeding_authored_maximum() {
        let previous = range((110.0, 140.0), (240.0, 280.0)); // 灌木低档退出边界。
        let farthest = range((240.0, 280.0), (320.0, 380.0)); // 灌木超低档进入与最远退出边界。
        let previous_ultra =
            visibility_range_for_quality(&previous, false, SceneDetailQuality::Ultra);
        let farthest_ultra =
            visibility_range_for_quality(&farthest, true, SceneDetailQuality::Ultra);
        assert_eq!(previous_ultra.end_margin, farthest_ultra.start_margin); // 相邻交叉淡化区必须继续严格连续。
        assert_eq!(farthest_ultra.end_margin, farthest.end_margin); // 极高不得增加实例或突破作者最远范围。
    } // 完成极高 LOD 延伸与最大范围边界覆盖。

    #[test]
    fn high_restores_the_exact_authored_range() {
        let authored = VisibilityRange {
            start_margin: 75.25..90.5,   // 使用非整数确保不是近似重建。
            end_margin: 170.75..195.125, // 使用非整数锁定精确 clone 语义。
            use_aabb: true,              // 作者测距模式也必须原样保留。
        }; // 完成带小数和 AABB 标记的作者范围。
        assert!(
            visibility_range_for_quality(&authored, false, SceneDetailQuality::High) == authored,
            "高档必须逐字段恢复作者基线",
        ); // VisibilityRange 没有 Debug，使用显式相等断言保持同一契约。
    } // 完成高档精确恢复覆盖。
} // 完成场景细节纯函数测试。
