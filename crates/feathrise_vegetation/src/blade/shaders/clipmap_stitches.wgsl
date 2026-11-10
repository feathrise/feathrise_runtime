// 共享边界约束与统一采样正文；直接读取uniform，避免把四KiB数组复制到每个GPU调用的函数局部。
#define_import_path feathrise_vegetation::clipmap_stitches
struct ClipmapStitches { edges:array<vec4<f32>,4>, heights:array<array<vec4<f32>,65>,4> }
#ifdef CLIPMAP_STITCH_COMPUTE
struct BladeClipmapParams {
    stitches:ClipmapStitches,
    holes:array<vec4<f32>,32>,hole_info:vec4<f32>,scene_bounds:vec4<f32>,grid:vec4<f32>,
    options:vec4<f32>,view:vec4<f32>,heights:vec4<f32>,missing_pages:array<vec4<u32>,16>,bands:array<vec4<u32>,16>,
}
@group(0) @binding(9) var<uniform> blade_clipmap:BladeClipmapParams;
#else
struct ClipmapHeightParams {
    stitches:ClipmapStitches,
    scene_bounds:vec4<f32>,grid:vec4<f32>,options:vec4<f32>,reserved:vec4<f32>,source_to_world:mat4x4<f32>,world_to_source:mat4x4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(120) var<uniform> clipmap_params:ClipmapHeightParams;
#endif
fn stitch_info(edge:u32)->vec4<f32> {
#ifdef CLIPMAP_STITCH_COMPUTE
    return blade_clipmap.stitches.edges[edge];
#else
    return clipmap_params.stitches.edges[edge];
#endif
}
fn stitch_value(edge:u32,index:u32)->f32 {
#ifdef CLIPMAP_STITCH_COMPUTE
    return blade_clipmap.stitches.heights[edge][index/4u][index%4u];
#else
    return clipmap_params.stitches.heights[edge][index/4u][index%4u];
#endif
}
fn stitches_active()->bool {
    for(var e=0u;e<4u;e+=1u){if stitch_info(e).x>=2.0{return true;}}return false;
}
fn stitch_sample(edge:u32,distance:f32)->f32 {
    let info=stitch_info(edge);let stride=max(u32(info.w),1u);let last=u32(info.x)-1u;
    let at=min(max(distance,0.0)/info.y,f32(last));let lo=min(u32(at)/stride*stride,last);let hi=min(lo+stride,last);
    var t=0.0;if hi>lo{t=(at-f32(lo))/f32(hi-lo);}return mix(stitch_value(edge,lo),stitch_value(edge,hi),t);
}
fn stitch_position(point:vec2<f32>,bounds:vec4<f32>)->vec2<f32> {
    if !stitches_active(){return point;}var p=clamp(point,bounds.xy,bounds.zw);
    for(var e=0u;e<4u;e+=1u){
        let info=stitch_info(e);if info.x<2.0{continue;}
        var axis=0u;if e<2u{axis=1u;}let normal=1u-axis;
        var bound=bounds[normal];if e%2u==1u{bound=bounds[normal+2u];}
        if abs(p[normal]-bound)<=0.00001 {
            let step=info.y*max(info.w,1.0);
            p[axis]=clamp(bounds[axis]+floor((p[axis]-bounds[axis])/step+0.5)*step,bounds[axis],bounds[axis+2u]);
        }
    }return p;
}
fn stitch_boundary_height(point:vec2<f32>,bounds:vec4<f32>)->vec2<f32> {
    for(var e=0u;e<4u;e+=1u){let info=stitch_info(e);if info.x<2.0{continue;}var axis=0u;if e<2u{axis=1u;}let normal=1u-axis;
        var bound=bounds[normal];if e%2u==1u{bound=bounds[normal+2u];}
        if abs(point[normal]-bound)<=0.00001{return vec2<f32>(stitch_sample(e,point[axis]-bounds[axis]),1.0);}
    }return vec2<f32>(0.0);
}
fn stitch_normal(point:vec2<f32>,original:vec3<f32>,bounds:vec4<f32>)->vec3<f32> {
    for(var e=0u;e<4u;e+=1u){let info=stitch_info(e);if info.x<2.0{continue;}var axis=0u;if e<2u{axis=1u;}let normal=1u-axis;
        var bound=bounds[normal];if e%2u==1u{bound=bounds[normal+2u];}
        if abs(point[normal]-bound)<=0.00001{
            let distance=point[axis]-bounds[axis];let step=info.y*max(info.w,1.0);
            let lo=max(0.0,distance-step);let hi=min(bounds[axis+2u]-bounds[axis],distance+step);
            let slope=(stitch_sample(e,hi)-stitch_sample(e,lo))/max(hi-lo,0.00001);
            if axis==0u{return normalize(vec3<f32>(-slope,1.0,0.0));}return normalize(vec3<f32>(0.0,1.0,-slope));
        }
    }return original;
}
fn stitch_height(point:vec2<f32>,base:f32,bounds:vec4<f32>)->f32 {
    var total=0.0;var weighted=0.0;var remain=1.0;
    for(var e=0u;e<4u;e+=1u){
        let info=stitch_info(e);if info.x<2.0{continue;}var axis=0u;if e<2u{axis=1u;}let normal=1u-axis;
        var bound=bounds[normal];if e%2u==1u{bound=bounds[normal+2u];}
        let distance=abs(point[normal]-bound);if distance>=info.z&&distance>0.00001{continue;}
        let value=stitch_sample(e,point[axis]-bounds[axis]);
        if distance<=0.00001{return value;}
        let t=clamp(distance/info.z,0.0,1.0);let weight=1.0-t*t*(3.0-2.0*t);
        total+=weight;weighted+=weight*value;remain*=1.0-weight;
    }if total==0.0{return base;}return base*remain+(1.0-remain)*weighted/total;
}
// 边点收敛会把相邻三角拉进当前名义格之外；逆映射只增加固定的四乘四候选。
fn stitch_candidate_axis(point:vec2<f32>,bounds:vec4<f32>,step:vec2<f32>,center:vec2<f32>,axis:u32,index:u32)->f32 {
    if index==0u{return point[axis];}let normal=1u-axis;
    var first=0u;if axis==0u{first=2u;}
    for(var e=first;e<first+2u;e+=1u){
        let info=stitch_info(e);if info.x<2.0{continue;}
        var bound=bounds[normal];if e%2u==1u{bound=bounds[normal+2u];}
        let grid=(bound-center[normal])/step[normal];var inner=floor(grid)+1.0;if e%2u==1u{inner=ceil(grid)-1.0;}
        let width=max(abs(center[normal]+inner*step[normal]-bound),0.00001);
        let t=abs(point[normal]-bound)/width;if t>1.00001{continue;}
        let shared_step=info.y*max(info.w,1.0);let lo=bounds[axis]+floor((point[axis]-bounds[axis])/shared_step)*shared_step;
        let hi=min(lo+shared_step,bounds[axis+2u]);var result=(lo+hi)*0.5;
        if index==1u{result=(point[axis]-(1.0-t)*lo)/max(t,0.000001);}
        if index==2u{result=(point[axis]-(1.0-t)*hi)/max(t,0.000001);}
        return clamp(result,bounds[axis],bounds[axis+2u]);
    }return point[axis];
}
fn stitch_candidate(point:vec2<f32>,bounds:vec4<f32>,step:vec2<f32>,center:vec2<f32>,index:u32)->vec2<f32> {
    return vec2<f32>(stitch_candidate_axis(point,bounds,step,center,0u,index%4u),stitch_candidate_axis(point,bounds,step,center,1u,index/4u));
}
