//! 地表和水材质共用的64字节天气布局；各扩展保留binding119。
#define_import_path feathrise_environment::surface_weather
#import feathrise_environment::rain_shelter::rain_shelter_volume_blocks

struct SurfaceWeatherGpu { // 必须与Rust四个Vec4保持顺序一致。
    weather: vec4<f32>, // 湿润、积水、降雨、暂停视觉秒。
    wind_quality: vec4<f32>, // 世界风向X/Z、风强、质量0..3。
    bounds: vec4<f32>, // 当前积水图最小X/Z和最大X/Z。
    options: vec4<f32>, // 积水图启用、作者潜力预览、雨纹距离米、雨纹密度倍率。
} // 固定64字节，不随地图或水面数量增长。

@group(#{MATERIAL_BIND_GROUP}) @binding(119) // 不占用StandardMaterial官方绑定。
var<storage, read> surface_weather: SurfaceWeatherGpu; // 所有材质读取同一个资产缓冲。

struct RainShelterVolume { // 显式作者遮雨盒，无需物理碰撞或可见网格。
    world_to_local: mat4x4<f32>, // 保留父节点旋转与非均匀缩放。
    half_extents: vec4<f32>, // xyz局部半径，w世界最高点快速出口。
    world_bounds: vec4<f32>, // 世界XZ包围范围，屋外在矩阵运算前拒绝。
}
struct RainShelterGpu { // 与环境侧固定32体ABI一致。
    header: vec4<f32>, // x有效数量，y全部体最高点。
    shelters: array<RainShelterVolume, 32>, // 作者超限由发布侧整批诊断，不截断。
}
@group(#{MATERIAL_BIND_GROUP}) @binding(118)
var<storage, read> rain_shelter: RainShelterGpu; // 地表和水面共享独立固定小缓冲。

fn rain_shelter_exposure(world: vec3<f32>) -> f32 { // 只改变局部视觉，不写回区域天气历史。
    let origin = world + vec3<f32>(0.0, 0.03, 0.0); // 避免屋顶自身表面被错误遮挡。
    if rain_shelter.header.x < 0.5 || origin.y >= rain_shelter.header.y { return 1.0; } // 无代理和最高屋顶上方零矩阵开销。
    for (var index = 0u; index < min(u32(rain_shelter.header.x), 32u); index += 1u) { // 每片元固定最坏32体。
        let volume = rain_shelter.shelters[index]; // 当前完整代理。
        if rain_shelter_volume_blocks(world, volume.world_to_local, volume.half_extents, volume.world_bounds) { return 0.0; } // 与雨丝使用相同相交函数。
    }
    return 1.0; // 开放区域保留原天气效果。
}
