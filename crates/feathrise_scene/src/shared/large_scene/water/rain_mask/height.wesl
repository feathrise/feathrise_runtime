// 数值捕获保留实际水网格的顶点Alpha，空白像素clear=0即没有水。
import bevy_pbr::render::forward_io::VertexOutput;

@group(constants::MATERIAL_BIND_GROUP) @binding(105) var<uniform> coverage: vec4<f32>; // 真实基础材质透明度。

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> { // 输入布局与原水顶点shader逐位置一致。
    if coverage.x <= 0.0 { discard; } // 原材质完全透明时不制造看不见的挡雨水面。
    @if(VERTEX_COLORS) if in.color.a <= 0.0 { discard; } // 透明岸外不写深度或高度，不能拿包围盒代替真实覆盖。
    let encoded = in.world_position.y / 16.0; // 分段存储降低半浮点在高海拔的米级量化。
    return vec4<f32>(floor(encoded), fract(encoded), 1.0, 1.0); // RG解码为(R+G)*16，B明确表示实际水面覆盖。
} // 不做曝光、雾、PBR或色调映射。
