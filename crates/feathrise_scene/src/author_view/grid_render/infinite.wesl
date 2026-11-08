// 当前视图射线与工作平面解析求交；不依赖地图边界、有限窗口或实体深度。
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import bevy_render::view::View;

@group(0) @binding(0) var<uniform> view: View;
struct Grid {
    origin_stride: vec4<f32>,
    plane: vec4<f32>,
    axis_u: vec4<f32>,
    axis_v: vec4<f32>,
}
@group(0) @binding(1) var<uniform> grid: Grid;

fn lines(coordinate: vec2<f32>, footprint: vec2<f32>, spacing: f32) -> f32 {
    let cell = coordinate / spacing;
    let distance = abs(fract(cell + 0.5) - 0.5) * spacing;
    let coverage = clamp(vec2<f32>(1.0) - distance / footprint, vec2<f32>(0.0), vec2<f32>(1.0));
    return max(coverage.x, coverage.y);
}
fn axis_color(direction: vec3<f32>) -> vec3<f32> {
    // sRGB(0.60,0.21,0.20)、(0.24,0.52,0.28)、(0.25,0.39,0.67)转线性；轴色按延伸方向。
    return direction.x * vec3<f32>(0.318547,0.036306,0.033105)
         + direction.y * vec3<f32>(0.046665,0.233022,0.063724)
         + direction.z * vec3<f32>(0.050876,0.125967,0.406448);
}
fn over(base: vec4<f32>, color: vec3<f32>, alpha: f32) -> vec4<f32> {
    return vec4<f32>(color * alpha + base.rgb * (1.0-alpha), alpha + base.a * (1.0-alpha));
}
@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = (in.position.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = uv * vec2<f32>(2.0,-2.0) + vec2<f32>(-1.0,1.0);
    // z=0在透视无限反向投影下会落到无穷远，使用1与0.5兼容透视和正交。
    let near_h = view.view_from_clip * vec4<f32>(ndc,1.0,1.0);
    let mid_h = view.view_from_clip * vec4<f32>(ndc,0.5,1.0);
    let near_view = near_h.xyz / near_h.w;
    let mid_view = mid_h.xyz / mid_h.w;
    // 先在观察空间相减，避免大坐标抵消不足一米的射线；方向以w=0转世界，不叠加相机平移。
    let near = (view.world_from_view * vec4<f32>(near_view,1.0)).xyz;
    let ray = (view.world_from_view * vec4<f32>(mid_view-near_view,0.0)).xyz;
    let denominator = dot(ray,grid.plane.xyz);
    let safe_denominator = select(-1.0,1.0,denominator>=0.0) * max(abs(denominator),0.000001);
    let t = (grid.plane.w - dot(near,grid.plane.xyz)) / safe_denominator;
    let hit = near + ray*t;
    let offset = hit-grid.origin_stride.xyz;
    let coordinate = vec2<f32>(dot(offset,grid.axis_u.xyz),dot(offset,grid.axis_v.xyz)) / grid.origin_stride.w;
    let footprint = max(fwidth(coordinate),vec2<f32>(0.000001));
    // 固定三层十倍层级连续交接；远处保持可读密度，不随可见距离增加线段或CPU工作。
    let lod = max(log2(max(max(footprint.x,footprint.y)*12.0,1.0))/log2(10.0),0.0);
    let spacing = pow(10.0,floor(lod));
    let blend = fract(lod);
    let minor = lines(coordinate,footprint,spacing) * (1.0-blend);
    let major = lines(coordinate,footprint,spacing*10.0);
    let distant = lines(coordinate,footprint,spacing*100.0);
    var color = over(vec4<f32>(0.0),vec3<f32>(0.036306),minor);
    // 两档灰度同处工作背景较暗一侧，防止LOD交接的颜色穿过背景灰而让整档网格隐去。
    color = over(color,vec3<f32>(mix(0.046665,0.036306,blend)),major);
    color = over(color,vec3<f32>(0.046665),distant);
    let axis = clamp(vec2<f32>(1.0)-abs(coordinate)/footprint,vec2<f32>(0.0),vec2<f32>(1.0));
    color = over(color,axis_color(grid.axis_v.xyz),axis.x);
    color = over(color,axis_color(grid.axis_u.xyz),axis.y);
    // 先统一计算导数再裁无交点/近平行射线，避免非一致控制流的导数和地平线闪烁。
    let angle = abs(denominator) / max(length(ray),0.000001);
    let fade = smoothstep(0.0002,0.003,angle);
    let valid = t>=0.0 && all(uv>=vec2<f32>(0.0)) && all(uv<=vec2<f32>(1.0));
    return color * fade * select(0.0,1.0,valid);
}
