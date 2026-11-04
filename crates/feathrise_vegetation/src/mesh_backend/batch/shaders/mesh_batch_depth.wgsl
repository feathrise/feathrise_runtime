// 【功能设计】纯深度预通道与阴影沿用逐株LOD裁切，颜色丢弃的叶片像素也不能写深度。
// 独立片元模块避免顶点与片元条件导入混用；不增加法线、运动矢量或颜色附件。
#import bevy_pbr::{
    prepass_io::VertexOutput,
    pbr_functions::visibility_range_dither,
}
#ifdef MAY_DISCARD
#import bevy_pbr::pbr_prepass_functions::prepass_alpha_discard
#endif

@fragment
fn fragment(in: VertexOutput) {
    visibility_range_dither(in.position, in.visibility_range_dither);
#ifdef MAY_DISCARD
    prepass_alpha_discard(in); // 保留叶片透明纹理及阈值；Opaque深度路径不要求材质绑定。
#endif
}
