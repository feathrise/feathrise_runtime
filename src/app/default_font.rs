//! SDK 的可选中文默认字体：唯一源随程序编译，应用可关闭特性或提供自己的默认字体。

use bevy::{asset::AssetId, prelude::*};

/// 编辑器和游戏共用的唯一字体源；运行时不读取 Cargo 缓存或仓库里的字体文件。
pub const DEFAULT_FONT_BYTES: &[u8] = feathrise_font::BYTES;
/// 字体原许可随二进制保留，分发索引仍应保留可读的 OFL 原文。
pub const DEFAULT_FONT_LICENSE: &str = feathrise_font::OFL;

/// 在 Bevy `DefaultPlugins` 后安装；未显式指定字体的文字默认支持中文。
///
/// 仅在插件装配阶段替换 Bevy 自带 Fira 或空默认值。工程提前提供的字体保持，
/// 之后设置的默认字体也不会被覆盖；显式 `TextFont.font` 句柄始终由工程提供。
pub struct DefaultFontPlugin;

impl Plugin for DefaultFontPlugin {
    fn build(&self, app: &mut App) {
        // 无 TextPlugin 的无窗口/纯逻辑应用无需加载字体，也不为默认字体装入完整文字系统。
        let Some(mut fonts) = app.world_mut().get_resource_mut::<Assets<Font>>() else {
            return;
        };
        install(&mut fonts);
    }
}

fn install(fonts: &mut Assets<Font>) {
    let id = AssetId::<Font>::default();
    if fonts.get(id).is_some_and(|font| {
        font.data.as_ref() != bevy::text::DEFAULT_FONT_DATA
    }) {
        return;
    }
    // Font::from_bytes 只接管字节；字形注册和缓存沿用 Bevy TextPlugin。
    // 只在首帧前复制一次，所有默认控件共享默认资产，避免按路径另载第二份中文字体。
    fonts.insert(id, Font::from_bytes(DEFAULT_FONT_BYTES.to_vec()))
        .expect("Bevy 默认字体资产 ID 必须可写");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_replaces_only_bevy_default_and_is_not_reinstalled() {
        let mut fonts = Assets::<Font>::default();
        let id = AssetId::<Font>::default();
        fonts.insert(id, Font::from_bytes(bevy::text::DEFAULT_FONT_DATA.to_vec())).unwrap();
        install(&mut fonts);
        assert_eq!(fonts.get(id).unwrap().data.as_ref(), DEFAULT_FONT_BYTES);
        let data = fonts.get(id).unwrap().data.clone();
        install(&mut fonts);
        assert_eq!(fonts.get(id).unwrap().data, data, "再次安装必须复用同一字体资产");
        assert_eq!(fonts.len(), 1);
    }

    #[test]
    fn explicitly_supplied_default_font_is_preserved() {
        let mut fonts = Assets::<Font>::default();
        let id = AssetId::<Font>::default();
        fonts.insert(id, Font::from_bytes(b"application font".to_vec())).unwrap();
        install(&mut fonts);
        assert_eq!(fonts.get(id).unwrap().data.as_ref(), b"application font");
    }

    #[test]
    fn fontless_application_stays_fontless() {
        let mut app = App::new();
        app.add_plugins(DefaultFontPlugin);
        assert!(!app.world().contains_resource::<Assets<Font>>());
    }
}
