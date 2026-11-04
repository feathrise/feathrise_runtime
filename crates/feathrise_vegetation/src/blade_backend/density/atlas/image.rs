//! 紧凑R8图集到正式RGBA8 Bevy图片的无歧义编码。

use bevy::asset::RenderAssetUsages; // 图集图片需要同时保留主世界资产并上传渲染世界。
use bevy::prelude::Image; // CPU逻辑回退与GPU候选都使用同一种Bevy图片资产。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages}; // 正式候选必须同时支持采样、存储写、上传和回读。

use super::{DensityMipLevel, ResolvedDensityAtlas}; // 子模块直接读取已经受检的紧凑Mip链。

impl ResolvedDensityAtlas {
    /// 编码CPU已经生成完整Mip的Bevy图片，供无RenderApp逻辑测试安全回退。
    pub(in crate::blade_backend::density) fn to_image(&self) -> Result<Image, String> {
        let pixels = self.rgba_mip_bytes()?; // CPU回退必须上传基础级至1×1的完整参考链。
        self.image_with_bytes(pixels, "vegetation_resolved_density_atlas") // 使用正式四用途描述符创建图片。
    }

    /// 编码只含有效基础级的GPU候选图片，所有子Mip等待正式WGSL覆盖。
    #[cfg(test)] // 仅保留给P5.2b2a回归烟雾，正式运行时已经改走全零竞争候选。
    pub(in crate::blade_backend::density) fn to_gpu_candidate_image(
        &self,
    ) -> Result<Image, String> {
        let base = self.base_level()?; // 合法图集始终至少包含一层。
        let mut pixels = rgba_level_bytes(base)?; // RenderAsset只允许CPU上传活动布局的基础级权威。
        pixels.resize(self.rgba_byte_len()?, 0); // 子Mip显式清零，防止候选误用CPU参考数据冒充GPU结果。
        self.image_with_bytes(pixels, "vegetation_gpu_mip_candidate") // 候选尚未通过回读前不会绑定到任何叶片窗口。
    }

    /// 编码基础级和全部子Mip均为零的GPU竞争候选图片。
    pub(in crate::blade_backend::density) fn to_gpu_competition_candidate_image(
        &self,
    ) -> Result<Image, String> {
        let pixels = vec![0; self.rgba_byte_len()?]; // 正式竞争结果不得由CPU参考基础级预填充。
        self.image_with_bytes(pixels, "vegetation_gpu_competition_candidate") // RenderGraph先复制旧基础级或只写提交脏块。
    }

    /// 返回CPU参考完整Mip按Bevy上传顺序排列的紧凑RGBA字节。
    pub(in crate::blade_backend::density) fn rgba_mip_bytes(&self) -> Result<Vec<u8>, String> {
        let mut pixels = Vec::with_capacity(self.rgba_byte_len()?); // 一次分配全部比较字节。
        for level in &self.levels {
            append_rgba_level(&mut pixels, level); // 每级依次紧密写入且不包含行填充。
        }
        Ok(pixels) // 主世界保留该值直到版本化GPU回读完成逐字节比较。
    }

    /// 返回基础级真实宽度。
    pub(in crate::blade_backend::density) fn base_width(&self) -> Result<u32, String> {
        Ok(self.base_level()?.width) // GPU作业尺寸与同一受检基础级同源。
    }

    /// 返回基础级真实高度。
    pub(in crate::blade_backend::density) fn base_height(&self) -> Result<u32, String> {
        Ok(self.base_level()?.height) // 非正方形图集保留独立高度。
    }

    /// 返回基础级至1×1的完整级数。
    pub(in crate::blade_backend::density) fn mip_level_count(&self) -> Result<u32, String> {
        u32::try_from(self.levels.len()).map_err(|_| {
            format!(
                "vegetation blade layer {:?} mip level count overflowed",
                self.layer
            ) // 极端损坏级数在进入GPU描述符前显式拒绝。
        })
    }

    /// 取得必然非空的基础级。
    fn base_level(&self) -> Result<&DensityMipLevel, String> {
        self.levels.first().ok_or_else(|| {
            format!("vegetation blade layer {:?} has no mip levels", self.layer) // 损坏图集不得创建零尺寸图片。
        })
    }

    /// 计算完整RGBA链字节数并检查平台整数溢出。
    fn rgba_byte_len(&self) -> Result<usize, String> {
        let total_pixels = self.levels.iter().try_fold(0_usize, |total, level| {
            total.checked_add(level.pixels.len()) // 每级紧凑R8长度就是该级像素数量。
        });
        total_pixels
            .and_then(|pixels| pixels.checked_mul(4)) // 每个正式像素固定RGBA四字节。
            .ok_or_else(|| {
                format!(
                    "vegetation blade layer {:?} RGBA mip bytes overflowed",
                    self.layer
                ) // 失败时旧活动图片继续托底。
            })
    }

    /// 用统一正式描述符承载指定完整Mip字节。
    fn image_with_bytes(&self, pixels: Vec<u8>, label: &'static str) -> Result<Image, String> {
        if pixels.len() != self.rgba_byte_len()? {
            return Err(format!(
                "vegetation blade layer {:?} image bytes {} do not match expected {}",
                self.layer,
                pixels.len(),
                self.rgba_byte_len()?
            )); // 候选与CPU回退都禁止部分Mip数据进入RenderAsset。
        }
        let base = self.base_level()?; // 描述符基础尺寸与字节首级完全一致。
        let mut image = Image::new_uninit(
            Extent3d {
                width: base.width,        // 纹理描述符只声明基础级尺寸。
                height: base.height,      // 所有后续尺寸由WebGPU标准Mip规则推导。
                depth_or_array_layers: 1, // 每个无限层使用一张普通二维图片。
            },
            TextureDimension::D2,         // 叶片Compute绑定固定为二维纹理。
            TextureFormat::Rgba8Unorm,    // 保持既有不可过滤float采样兼容格式。
            RenderAssetUsages::default(), // 主世界句柄和渲染世界GPU资产都需要保留。
        );
        image.texture_descriptor.mip_level_count = self.mip_level_count()?; // 分配基础级至1×1完整链。
        image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::STORAGE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC; // 同一正式图集可上传基础级、逐级生成Mip并异步复制回读。
        image.texture_descriptor.label = Some(label); // 图形调试器区分CPU回退和未公开GPU候选。
        image.data = Some(pixels); // Bevy按基础级到最小级连续上传，Mip顺序没有歧义。
        Ok(image) // 返回尚未写入Assets表的完整图片值。
    }
}

/// 把一个紧凑R8级扩展为RGB同值且Alpha固定255的正式字节。
fn append_rgba_level(destination: &mut Vec<u8>, level: &DensityMipLevel) {
    for density in &level.pixels {
        destination.extend_from_slice(&[*density, *density, *density, 255]); // 与叶片红通道采样和离线KTX2严格一致。
    }
}

/// 单独编码基础级并检查容量溢出。
#[cfg(test)] // 仅供旧基础级上传回归验证，生产候选不再接收CPU解析结果。
fn rgba_level_bytes(level: &DensityMipLevel) -> Result<Vec<u8>, String> {
    let capacity = level.pixels.len().checked_mul(4).ok_or_else(|| {
        format!(
            "vegetation density base {}x{} RGBA bytes overflowed",
            level.width, level.height
        )
    })?; // 候选分配前验证基础级容量。
    let mut pixels = Vec::with_capacity(capacity); // 基础级只执行一次确定性扩展。
    append_rgba_level(&mut pixels, level); // 保持CPU参考和GPU输入使用同一编码函数。
    Ok(pixels) // 后续调用方补齐零子Mip以满足Bevy图片长度契约。
}
