//! 三维作者格必须与正式前向、阴影、运动矢量、风和公告板顶点入口共同编译。
use std::collections::HashSet;

fn expand(source: &str, defs: &HashSet<&str>) -> String {
    let mut result = String::new();
    let mut stack = Vec::new();
    let mut enabled = true;
    let mut import = false;
    for line in source.lines() {
        let trim = line.trim();
        if import {
            if trim.starts_with('}') {
                import = false;
            }
            continue;
        }
        if trim.starts_with("#import ") {
            import = trim.contains('{');
            continue;
        }
        if let Some(name) = trim.strip_prefix("#ifdef ") {
            stack.push((enabled, defs.contains(name)));
            enabled = enabled && defs.contains(name);
            continue;
        }
        if let Some(name) = trim.strip_prefix("#ifndef ") {
            stack.push((enabled, !defs.contains(name)));
            enabled = enabled && !defs.contains(name);
            continue;
        }
        if trim == "#else" {
            let (parent, condition) = stack.last_mut().unwrap();
            *condition = !*condition;
            enabled = *parent && *condition;
            continue;
        }
        if trim == "#endif" {
            enabled = stack.pop().unwrap().0;
            continue;
        }
        if trim.starts_with('#') {
            continue;
        }
        if enabled {
            result.push_str(line);
            result.push('\n');
        }
    }
    assert!(stack.is_empty());
    result.replace("#{MATERIAL_BIND_GROUP}", "3")
}

#[test]
fn volume_mesh_vertex_variants_use_valid_production_wgsl() {
    const ENGINE: &str = r#"
struct View {lod_view_world_position:vec3<f32>}
@group(0) @binding(0) var<uniform> view:View;
struct Globals {time:f32,delta_time:f32,padding:vec2<f32>}
@group(0) @binding(1) var<uniform> globals:Globals;
fn position_world_to_clip(p:vec3<f32>)->vec4<f32>{return vec4<f32>(p,1.0);}
fn get_world_from_local(index:u32)->mat4x4<f32>{return mat4x4<f32>(vec4<f32>(1,0,0,0),vec4<f32>(0,1,0,0),vec4<f32>(0,0,1,0),vec4<f32>(0,0,0,1));}
"#;
    let mesh = include_str!("../../../crates/feathrise_vegetation/src/mesh_backend/batch/shaders/mesh_batch.wgsl");
    let wind = include_str!("../../../crates/feathrise_environment/src/mesh_wind/shaders/wind.wgsl");
    let billboard = include_str!("../../../crates/feathrise_vegetation/src/mesh_backend/billboard/shaders/billboard.wgsl");
    for prepass in [false, true] {
        for extension in ["", "VEGETATION_BATCH_WIND", "VEGETATION_BATCH_BILLBOARD"] {
            let mut defs = HashSet::from([
                "VERTEX_UVS_A",
                "VERTEX_UVS_B",
                "VERTEX_NORMALS",
                "VERTEX_TANGENTS",
                "VERTEX_COLORS",
                "VERTEX_OUTPUT_INSTANCE_INDEX",
                "VISIBILITY_RANGE_DITHER",
            ]);
            if prepass {
                defs.extend([
                    "VEGETATION_BATCH_PREPASS",
                    "NORMAL_PREPASS_OR_DEFERRED_PREPASS",
                    "MOTION_VECTOR_PREPASS",
                ]);
            }
            if !extension.is_empty() {
                defs.insert(extension);
            }
            let mut source = ENGINE.to_owned();
            if extension == "VEGETATION_BATCH_WIND" {
                source.push_str(&expand(wind, &defs));
            }
            if extension == "VEGETATION_BATCH_BILLBOARD" {
                source.push_str(&expand(billboard, &defs));
            }
            source.push_str(&expand(mesh, &defs));
            let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| {
                panic!(
                    "三维Mesh语法：{prepass} {extension}: {}",
                    e.emit_to_string(&source)
                )
            });
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("三维Mesh语义：{prepass} {extension}: {e:?}"));
        }
    }
}
