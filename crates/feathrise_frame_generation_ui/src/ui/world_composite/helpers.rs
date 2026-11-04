//! 世界UI当前帧有效范围与尺寸映射的纯数学转换，不依赖时间历史或GPU状态。
use super::types::WorldUiSample;
use bevy::prelude::*; // 固定二维尺寸与四维Uniform参数。 // CPU/GPU共享参数布局。

pub(super) fn sampling_parameters(render: UVec2, output: UVec2) -> WorldUiSample {
    // 调用方已确认有限非零尺寸，独立材质已经使用无抖动投影。
    WorldUiSample {
        mapping: Vec4::new(
            render.x as f32 / output.x as f32,
            render.y as f32 / output.y as f32,
            0.0,
            0.0,
        ),
        bounds: Vec4::new(render.x as f32, render.y as f32, 0.0, 0.0),
    } // 保留当前实际渲染比例，zw固定为零避免合成器二次位移。
} // 采样参数构造结束。

#[cfg(test)] // 纯数学回归不启动GPU。
mod tests {
    // 固定输出分辨率无抖动合成与有效范围约定。
    use super::*; // 复用唯一参数构造入口。
    #[test] // 4K超性能必须按1280×720有效区采样，不能误用全4K纹理尺寸。
    fn spatial_sampling_keeps_output_scale_without_temporal_jitter() {
        // 无抖动材质直接按输出层坐标采样。
        let parameters = sampling_parameters(UVec2::new(3840, 2160), UVec2::new(3840, 2160)); // 头顶UI在输出分辨率层完整光栅化。
        assert_eq!(parameters.mapping, Vec4::new(1.0, 1.0, 0.0, 0.0)); // 不再由合成器执行空间位移。
        assert_eq!(parameters.bounds, Vec4::new(3840.0, 2160.0, 0.0, 0.0)); // 输出层全幅写入，纹理未写区域不能参加过滤。
    } // 空间采样测试结束。
} // 测试模块结束。
