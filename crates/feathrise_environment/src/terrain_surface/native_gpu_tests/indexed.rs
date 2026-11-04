//! 索引地表GPU查询的2×2真实纹理夹具与数值断言；不复制生产候选聚合算法。
use wgpu::util::DeviceExt;

pub(super) struct Resources {
    pub(super) uniform: wgpu::Buffer,
    pub(super) weights: wgpu::TextureView,
    pub(super) indices: wgpu::TextureView,
    pub(super) duplicate_indices: wgpu::TextureView,
    _textures: [wgpu::Texture; 3],
}

pub(super) fn resources(device: &wgpu::Device, queue: &wgpu::Queue, source: &str) -> Resources {
    // 从正式WGSL布局取得buffer跨度及字段偏移，避免另造过期的Rust测试uniform镜像。
    let module = wgpu::naga::front::wgsl::parse_str(source).unwrap();
    let (_, uniform) = module
        .global_variables
        .iter()
        .find(|(_, variable)| variable.name.as_deref() == Some("terrain_surface"))
        .unwrap();
    let wgpu::naga::TypeInner::Struct { members, span } = &module.types[uniform.ty].inner else {
        panic!("正式地表uniform必须保持结构布局");
    };
    let bounds = members
        .iter()
        .find(|member| member.name.as_deref() == Some("world_bounds"))
        .unwrap();
    let mut uniform_bytes = vec![0; *span as usize];
    for (channel, value) in [0f32, 0., 1., 1.].into_iter().enumerate() {
        let offset = bounds.offset as usize + channel * 4;
        uniform_bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("native_indexed_test_bounds"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    // 行序为(0,0)、(1,0)、(0,1)、(1,1)，与正式先Y后X的双线性循环一致。
    let unique: [u32; 16] = [9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 11, 10, 15, 14, 13, 12];
    let duplicate: [u32; 16] = [1, 2, 3, 4, 2, 3, 4, 5, 3, 4, 5, 6, 4, 5, 6, 7];
    let index_bytes: Vec<_> = unique.into_iter().flat_map(u32::to_le_bytes).collect();
    let duplicate_bytes: Vec<_> = duplicate.into_iter().flat_map(u32::to_le_bytes).collect();
    let (index_texture, indices) = texture(
        device,
        queue,
        "native_indexed_sixteen_keys",
        wgpu::TextureFormat::Rgba32Uint,
        &index_bytes,
        16,
    );
    let (duplicate_texture, duplicate_indices) = texture(
        device,
        queue,
        "native_indexed_repeated_keys",
        wgpu::TextureFormat::Rgba32Uint,
        &duplicate_bytes,
        16,
    );
    // R8通道提供实际可过滤的权重图；四邻同字节值让平局裁决与采样顺序可精确检查。
    let weights_bytes = [16u8, 32, 48, 64].repeat(4);
    let (weight_texture, weights) = texture(
        device,
        queue,
        "native_indexed_four_weights",
        wgpu::TextureFormat::Rgba8Unorm,
        &weights_bytes,
        4,
    );
    Resources {
        uniform,
        weights,
        indices,
        duplicate_indices,
        _textures: [index_texture, duplicate_texture, weight_texture],
    }
}

fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    format: wgpu::TextureFormat,
    bytes: &[u8],
    bytes_per_pixel: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: 2,
        height: 2,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(2 * bytes_per_pixel),
            rows_per_image: Some(2),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

pub(super) fn assert_queries(values: &[f32]) {
    for (case, (indices, weights)) in [
        ([2., 6., 10., 12.], [0.25; 4]), // 十六个不同候选中的最高四项并列，仍按较小材质索引裁决。
        ([6., 7., 8., 9.], [0.4, 0.3, 0.2, 0.1]),
        ([2., 3., 4., 5.], [0.4, 0.3, 0.2, 0.1]),
        ([10., 11., 0., 1.], [0.4, 0.3, 0.2, 0.1]),
        ([12., 13., 14., 15.], [0.4, 0.3, 0.2, 0.1]),
        (
            [10., 11., 0., 6.],
            [12. / 31., 9. / 31., 6. / 31., 4. / 31.],
        ), // 非对称双线性权重锁定四邻纹素的坐标与顺序。
        ([0.; 4], [0.; 4]), // 范围外不能借用边缘纹素或第一层。
        ([4., 5., 6., 3.], [0.3125, 0.28125, 0.21875, 0.1875]), // 跨纹素去重后再截断四槽。
    ]
    .into_iter()
    .enumerate()
    {
        let offset = 72 + case * 8;
        assert_eq!(&values[offset..offset + 4], &indices, "十六候选案例{case}");
        for (slot, expected) in weights.into_iter().enumerate() {
            assert!(
                (values[offset + 4 + slot] - expected).abs() < 0.00001,
                "十六候选案例{case}的权重槽{slot}错误"
            );
        }
    }
}
