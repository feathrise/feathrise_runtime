// 作者视图实体着色从正式深度/法线得到几何，保留Clipmap位移、裁切和所有原始材质资产。
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var normals: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
struct Settings { options:vec4<f32>, wire:vec4<f32> }
@group(0) @binding(3) var<uniform> settings:Settings;
@fragment
fn fragment(in:FullscreenVertexOutput)->@location(0) vec4<f32> {
    let size=vec2<i32>(textureDimensions(source));
    let p=clamp(vec2<i32>(in.uv*vec2<f32>(size)),vec2<i32>(0),size-1);
    let color=textureLoad(source,p,0);
    let d=textureLoad(depth,p,0);
    let normal_sample=textureLoad(normals,p,0);
    // 只处理作者线框的主色与背景，后续Gizmo仍保留橙色选中、悬停和变换手柄。
    // 不依赖原材质亮度或天空颜色，因此白色地形线不会盖过物体结构。
    if settings.options.x<0.5 {
        if d<=0.0000001 {return vec4<f32>(0.055,0.055,0.055,1.0);}
        return vec4<f32>(0.0025,0.0025,0.0025,1.0);
    }
    if d<=0.0000001 {
        if settings.options.w<0.5 {return vec4<f32>(0.055,0.065,0.08,1.0);}
        return color;
    }
    if settings.options.x<0.5 || settings.options.x>1.5 {return color;}
    if dot(normal_sample.xyz,normal_sample.xyz)<0.00001 {return color;}
    let n=normalize(normal_sample.xyz*2.0-1.0);
    var gray=0.18+0.55*max(dot(n,normalize(vec3<f32>(-0.35,0.8,0.6))),0.0);
    var surface=vec3<f32>(gray);
    if settings.options.z>0.5 {surface=n*0.5+0.5;}
    if settings.options.y>0.5 {
        let right=clamp(p+vec2<i32>(1,0),vec2<i32>(0),size-1);
        let down=clamp(p+vec2<i32>(0,1),vec2<i32>(0),size-1);
        let nr=textureLoad(normals,right,0).xyz;
        let nd=textureLoad(normals,down,0).xyz;
        let edge=min(length(normal_sample.xyz-nr)+length(normal_sample.xyz-nd),1.0);
        surface*=1.0-0.65*edge;
    }
    return vec4<f32>(surface,1.0);
}
