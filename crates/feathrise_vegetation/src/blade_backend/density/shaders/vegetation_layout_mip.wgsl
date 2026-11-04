// GPU完整Mip生成只读取相邻父级，并逐级写入同一RGBA8图集的非重叠子资源。
struct MipParams {
    source_size: vec2<u32>, // 当前输入Mip的真实尺寸。
    target_size: vec2<u32>, // 当前输出Mip的真实尺寸。
};

@group(0) @binding(0)
var source_density: texture_2d<f32>; // 源视图只暴露当前父Mip。

@group(0) @binding(1)
var target_density: texture_storage_2d<rgba8unorm, write>; // 目标视图只暴露相邻子Mip。

@group(0) @binding(2)
var<uniform> params: MipParams; // 尺寸独立于纹理视图，保证单像素轴能显式钳制。

// 把UNORM采样恢复为CPU参考链使用的八位整数。
fn load_density(coordinate: vec2<u32>) -> u32 {
    let normalized = clamp(
        textureLoad(source_density, vec2<i32>(coordinate), 0).r,
        0.0,
        1.0,
    ); // 输入纹理只允许0到1的线性概率。
    return u32(round(normalized * 255.0)); // 基础级和每个父Mip都恢复到同一R8域。
}

@compute @workgroup_size(8, 8, 1)
fn downsample_density(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let target_coordinate = global_id.xy; // Z轴固定为一层二维纹理。
    if any(target_coordinate >= params.target_size) {
        return; // 尾部工作组线程不得写到目标Mip范围之外。
    }

    let source_max = params.source_size - vec2<u32>(1u); // 合法纹理尺寸至少为一，不会下溢。
    let source_00 = min(target_coordinate * 2u, source_max); // 左上样本命中偶数源坐标。
    let source_10 = min(source_00 + vec2<u32>(1u, 0u), source_max); // 单像素X轴重复边缘。
    let source_01 = min(source_00 + vec2<u32>(0u, 1u), source_max); // 单像素Y轴重复边缘。
    let source_11 = min(source_00 + vec2<u32>(1u, 1u), source_max); // 右下样本双轴钳制。
    let total = load_density(source_00)
        + load_density(source_10)
        + load_density(source_01)
        + load_density(source_11); // 四个八位整数之和最多1020。
    let density = (total + 2u) / 4u; // 与CPU和离线KTX2完全相同的最近整数box平均。
    let normalized = f32(density) / 255.0; // 写回RGBA8 UNORM时恢复精确八位端点。
    textureStore(
        target_density,
        vec2<i32>(target_coordinate),
        vec4<f32>(normalized, normalized, normalized, 1.0),
    ); // RGB保持同值、Alpha固定255，匹配既有密度图片字节契约。
}
