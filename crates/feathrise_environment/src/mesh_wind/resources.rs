//! 共享 Mesh 风运行时资源类型。
//! 【功能设计】场景层只把噪声资源存在视为材质安装门，不读取或改写程序纹理像素。

use bevy::prelude::*; // 引入Resource、Handle和Image等资源声明所需类型。

/// 树木与灌木全部风材质共享的程序噪声纹理。
/// Startup完成固定尺寸图片登记后才插入该资源，场景材质因此不会观察到半成品句柄。
#[derive(Resource, Clone, Debug)] // 资源保存强Image句柄并允许测试与装配层轻量克隆。
pub struct TreeWindNoiseTexture(pub Handle<Image>); // 保留原公开元组形状，现由项目自有类型承担资源身份。
