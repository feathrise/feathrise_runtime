//! 固定遮雨缓冲仅在完整代理批次变化时编码和上传。

use super::{RainShelterGpu, RainShelterState, RainShelterUpload};
use bevy::asset::uuid_handle; // 固定资产身份不绑定具体地图。
use bevy::prelude::*; // Bevy资源与调度。
use bevy::render::storage::ShaderBuffer; // 与当前Bevy0.19一致。 // 完整批次和发布顺序。

pub fn rain_shelter_buffer_handle() -> Handle<ShaderBuffer> {
    // 所有地表与水面绑定同一缓冲。
    uuid_handle!("6b01d1be-c8d6-46a8-b5c6-2f53eb704fb3") // 独立固定UUID避免与天气缓冲冲突。
} // 完成公共句柄。

pub(crate) fn install_rain_shelter(app: &mut App) {
    // 由环境地表插件只安装一次。
    app.init_resource::<RainShelterState>(); // 没有作者代理时默认全部受雨。
    app.init_resource::<Assets<ShaderBuffer>>(); // 支持无窗口测试。
    app.world_mut()
        .resource_mut::<Assets<ShaderBuffer>>()
        .insert(
            rain_shelter_buffer_handle().id(),
            ShaderBuffer::from(vec![RainShelterGpu::default()]),
        )
        .expect("遮雨UUID缓冲必须可以安装"); // 固定3088字节资产。
    app.add_systems(PostUpdate, upload.in_set(RainShelterUpload)); // 宿主完整发布后上传。
} // 完成插件安装。

fn upload(state: Res<RainShelterState>, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    // 不遍历材质或像素。
    if !state.is_changed() {
        return;
    } // 暂停与稳定帧零编码。
    if let Some(mut buffer) = buffers.get_mut(rain_shelter_buffer_handle().id()) {
        if let Some([gpu]) = buffer.cast_slice_mut::<RainShelterGpu>() {
            *gpu = state.gpu;
        }
    } // 同大小缓冲原位更新。
} // 完成固定上传。
