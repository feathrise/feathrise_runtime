//! 表面天气缓冲只创建一次，变化时使用Bevy原位更新GPU缓冲。

use bevy::asset::uuid_handle; // UUID句柄可在材质构造时直接取得，无需修改所有描述符。
use bevy::prelude::*; // 插件初始化、资源访问和调度声明。
use bevy::render::storage::ShaderBuffer; // Bevy相同大小缓冲以write_buffer更新而不重分配GPU存储。

use super::super::material::TerrainSurfaceMaterial;
use super::resources::{SurfacePuddleMask, SurfaceWeatherGpu, SurfaceWeatherUpload}; // 固定状态、完整遮罩和公开上传顺序。 // 遮罩换版时更新材质绑定，稳定天气帧不遍历材质。

/// 所有地表及水面材质引用唯一固定的天气缓冲。
pub fn surface_weather_buffer_handle() -> Handle<ShaderBuffer> {
    // 返回弱UUID身份，资产由插件主动保持。
    uuid_handle!("da04cb15-a9df-46a0-9f72-efb98dab63c2") // 与项目其他内置资源隔离。
} // 完成固定身份入口。

/// 地表插件安装时创建64字节干燥回退；更新不遍历材质或地图。
pub(in crate::terrain_surface) fn install_surface_weather(app: &mut App) {
    // 无RenderApp测试也能建立相同主世界契约。
    crate::rain_shelter::install_rain_shelter(app); // 同一环境插件安装共享无碰撞遮雨缓冲。
    app.init_resource::<SurfaceWeatherGpu>(); // 缺少宿主天气时始终保持原外观。
    app.init_resource::<SurfacePuddleMask>(); // 无作者数据时保持没有积水纹理。
    app.init_resource::<Assets<ShaderBuffer>>(); // 完整Bevy宿主已登记；纯数据宿主允许初始化空表。
    app.world_mut()
        .resource_mut::<Assets<ShaderBuffer>>() // 只在插件安装创建一次资产。
        .insert(
            surface_weather_buffer_handle().id(),
            ShaderBuffer::from(vec![SurfaceWeatherGpu::default()]),
        ) // 固定大小且默认COPY_DST。
        .expect("表面天气UUID资产必须可以安装"); // UUID不是可回收索引，正常安装不会失败。
    app.add_systems(
        PostUpdate,
        upload_surface_weather.in_set(SurfaceWeatherUpload),
    ); // 宿主更新后、渲染提取前最多一次上传。
    app.add_systems(PostUpdate, bind_puddle_mask.in_set(SurfaceWeatherUpload)); // 仅换图换版或新材质时更新遮罩绑定。
} // 完成共享资源安装。

fn bind_puddle_mask(
    // 材质新增走事件，遮罩修订才访问完整材质集合。
    mask: Res<SurfacePuddleMask>, // 当前完整强句柄。
    mut events: MessageReader<AssetEvent<TerrainSurfaceMaterial>>, // 只消费当前帧实际新增或重建的材质事件。
    mut materials: ResMut<Assets<TerrainSurfaceMaterial>>, // 只在绑定确实不同时标记材质修改。
) {
    // 稳定运行不产生逐材质天气工作。
    if mask.is_changed() {
        // 遮罩完整换版为低频作者/切图事件。
        for (_, material) in materials.iter_mut() {
            // 当前地表材质数量取决于真实承接/预热，不创建每水洼材质。
            if material.extension.follows_global_puddle_mask() { material.extension.set_puddle_mask(mask.image.clone()); } // 子地图保持自己的受检遮罩。
        } // 完成遮罩换版。
        events.clear(); // 本次整表处理已覆盖新增和修改事件。
        return; // 不重复处理刚处理过的材质。
    } // 稳定纹理只处理事件。
    for event in events.read() {
        // 事件量与真正的材质更新相关，而非地图像素。
        let id = match event {
            // Modified也覆盖热重建的同ID材质，避免丢失遮罩。
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id, // 两类事件都可能产生没有遮罩的新扩展。
            _ => continue, // 删除或依赖完成不需要写回材质。
        }; // 提取稳定资产身份。
        if materials
            .get(id)
            .is_some_and(|material| material.extension.follows_global_puddle_mask() && material.extension.puddle_mask() != &mask.image)
        {
            // 自己写回产生的Modified在下一帧立即跳过。
            if let Some(mut material) = materials.get_mut(id) {
                // 资产可能已在本帧被回收。
                material.extension.set_puddle_mask(mask.image.clone()); // 只更新一个实际变更的材质。
            } // 完成单材质接入。
        } // 一致时不再发出修改事件。
    } // 完成增量材质绑定。
} // 完成遮罩独立生命周期桥。

fn upload_surface_weather(
    // 使用变更检测避免暂停和干燥稳定帧重复编码。
    state: Res<SurfaceWeatherGpu>, // 宿主已原子解析的固定状态。
    mut buffers: ResMut<Assets<ShaderBuffer>>, // 唯一64字节资产，不扫描资产集合。
) {
    // 系统热路径只有固定字段和一次句柄查找。
    if !state.is_changed() {
        return;
    } // 无变化时不触发RenderAsset提取或上传。
    if let Some(mut buffer) = buffers.get_mut(surface_weather_buffer_handle().id()) {
        // 固定句柄直接定位。
        if let Some([weather]) = buffer.cast_slice_mut::<SurfaceWeatherGpu>() {
            *weather = *state; // 固定64字节原位更新，GPU缓冲由Bevy复用。
        }
    } // 资源被宿主卸载时安全跳过。
} // 完成共享天气上传。

#[cfg(test)] // 检查公开ABI和真实变更上传，而非重复天气计算。
mod tests {
    // 保持无窗口测试与生产系统同源。
    use super::*; // 直接调用安装与上传入口。
    #[test]
    fn root_mask_updates_preserve_source_masks_and_explicit_empty_sources() {
        use crate::terrain_surface::{TerrainSurfaceExtension, TerrainSurfaceFallbackImages};
        let mut app = App::new();
        app.init_resource::<Assets<TerrainSurfaceMaterial>>()
            .add_message::<AssetEvent<TerrainSurfaceMaterial>>()
            .init_resource::<SurfacePuddleMask>()
            .add_systems(Update, bind_puddle_mask);
        let fallback = TerrainSurfaceFallbackImages { zero_weight: default(), white_base_color_array: default(),
            neutral_arm_array: default(), neutral_normal_array: default(), zero_index: default(), neutral_layer_table: default() };
        let material = || TerrainSurfaceMaterial { base: StandardMaterial::default(),
            extension: TerrainSurfaceExtension::new_passthrough(&fallback) };
        let mut images = Assets::<Image>::default();
        let root_mask = images.add(Image::default());
        let source_mask = images.add(Image::default());
        let root = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(material());
        let bounds = Vec4::new(-100.0, -50.0, 100.0, 50.0);
        let mut source_material = material();
        source_material.extension.set_source_puddle_mask(Some(source_mask.clone()), bounds);
        let source = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(source_material);
        let mut empty_material = material();
        empty_material.extension.set_source_puddle_mask(None, bounds);
        let empty = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(empty_material);
        app.world_mut().resource_mut::<SurfacePuddleMask>().image = Some(root_mask.clone());
        app.update();
        let materials = app.world().resource::<Assets<TerrainSurfaceMaterial>>();
        assert_eq!(materials.get(&root).unwrap().extension.puddle_mask(), &Some(root_mask));
        assert_eq!(materials.get(&source).unwrap().extension.puddle_mask(), &Some(source_mask.clone()));
        assert_eq!(materials.get(&empty).unwrap().extension.puddle_mask(), &None);
        assert_eq!(materials.get(&source).unwrap().extension.source_puddle_bounds(), bounds);
        app.world_mut().resource_mut::<SurfacePuddleMask>().image = None;
        app.update();
        // 父图撤销积水和源材质Modified事件都不能夺回子图的独立绑定。
        app.world_mut().write_message(AssetEvent::<TerrainSurfaceMaterial>::Modified { id: source.id() });
        app.world_mut().write_message(AssetEvent::<TerrainSurfaceMaterial>::Modified { id: empty.id() });
        app.update();
        let materials = app.world().resource::<Assets<TerrainSurfaceMaterial>>();
        assert_eq!(materials.get(&root).unwrap().extension.puddle_mask(), &None);
        assert_eq!(materials.get(&source).unwrap().extension.puddle_mask(), &Some(source_mask));
        assert_eq!(materials.get(&empty).unwrap().extension.puddle_mask(), &None);
        assert_eq!(materials.get(&empty).unwrap().extension.source_puddle_bounds(), bounds);
    }

    #[test] // 不同材质共享的布局必须与WGSL四vec4一致。
    fn weather_buffer_has_fixed_layout_and_paused_upload_is_idle() {
        // 无渲染设备即可验证主世界写侧。
        assert_eq!(std::mem::size_of::<SurfaceWeatherGpu>(), 64); // CPU/GPU ABI固定64字节。
        let mut app = App::new(); // 最小宿主避免真实GPU启动。
        app.init_resource::<Assets<TerrainSurfaceMaterial>>()
            .add_message::<AssetEvent<TerrainSurfaceMaterial>>(); // 满足正式遮罩绑定系统的资产事件依赖。
        install_surface_weather(&mut app); // 安装正式系统和资产。
        app.update(); // 消费初始化变更。
        <bevy::render::storage::GpuShaderBuffer as bevy::render::render_asset::RenderAsset>::take_gpu_data(
            &mut app.world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .get_mut(surface_weather_buffer_handle().id())
            .unwrap(),
            None,
        ).unwrap();
        app.update(); // 暂停稳定状态没有任何资源写入。
        assert!(
            app.world()
                .resource::<Assets<ShaderBuffer>>()
                .get(surface_weather_buffer_handle().id())
                .unwrap()
                .cast_slice::<u8>()
            .is_none()
        ); // 不重复制造上传包。
        app.world_mut()
            .resource_mut::<SurfaceWeatherGpu>()
            .weather
            .x = 0.5; // 模拟宿主收到真实湿润变化。
        app.update(); // 正式上传系统应消费变化。
        assert_eq!(
            app.world()
                .resource::<Assets<ShaderBuffer>>()
                .get(surface_weather_buffer_handle().id())
                .unwrap()
                .cast_slice::<u8>()
                .unwrap()
                .len(),
            64
        ); // 只产生一次固定64字节包。
    } // 完成暂停上传回归。
} // 完成共享资源测试。
