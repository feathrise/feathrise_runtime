//! 复用标准ViewTransmissionTexture绑定；每合格视图一次GPU颜色副本，无额外场景绘制。
use super::{
    camera::WaterReflectionView,
    components::{OwnedReflectionTexture, ReflectionCaptureReady},
    types::{ReflectionTextureAction, ReflectionTextureSpec},
};
use bevy::core_pipeline::core_3d::Transparent3d; // 复用本帧已完成可见性筛选的透明队列。
use bevy::pbr::{RenderMaterialInstances, Transmissive3d, ViewTransmissionTexture}; // 标准材质索引证明透明队列中确有水面。
use bevy::prelude::*; // RenderApp实体查询与命令。
use bevy::render::{
    render_phase::ViewSortedRenderPhases,
    render_resource::*,
    renderer::{RenderContext, RenderDevice, ViewQuery},
    view::{ExtractedView, ViewTarget},
}; // 真实视图纹理和编码器。
use std::any::TypeId; // 材质具体类型身份不依赖名字猜测。 // 资格、所有权和纯规格。

fn spec(texture: &Texture) -> ReflectionTextureSpec {
    // O(1)读取GPU描述，不访问像素。
    ReflectionTextureSpec {
        size: texture.size(),
        format: texture.format(),
        sample_count: texture.sample_count(),
    } // 使用实际内部尺寸和解析后采样数。
} // 结束实际纹理规格读取。

fn copy_specs_match(source: ReflectionTextureSpec, destination: ReflectionTextureSpec) -> bool {
    // 完整颜色副本只支持合法二维解析目标。
    source == destination
        && source.sample_count == 1
        && source.size.depth_or_array_layers == 1
        && source.size.width > 1
        && source.size.height > 1 // 目标规格任何一项变化都不能拿旧图继续复制。
} // 结束复制前纯规格核验。

fn source_is_eligible(spec: ReflectionTextureSpec, usage: TextureUsages) -> bool {
    // 自定义相机不能绕过GPU复制能力要求。
    copy_specs_match(spec, spec)
        && spec.format == TextureFormat::Rgba16Float
        && usage.contains(TextureUsages::COPY_SRC) // 只接收8字节每像素线性HDR；非HDR已色调映射或缺COPY_SRC时撤销目标回退IBL。
} // 结束源能力核验。

fn is_water_material(asset: bevy::asset::UntypedAssetId) -> bool {
    // 只认项目真实动态水材质类型。
    let kind = asset.type_id();
    kind == TypeId::of::<super::super::material::DynamicWaterMaterial>()
        || kind == TypeId::of::<crate::ClipmapWaterMaterial>() // Clipmap 水使用嵌套材质扩展，类型身份独立。
} // 结束现有材质索引类型检查。

fn action(
    eligible: bool,
    owned: bool,
    same_owner: bool,
    standard_active: bool,
    compatible: bool,
) -> ReflectionTextureAction {
    // 资源生命周期纯决策。
    if !eligible {
        // 退出地图、关闭反射或切换作者相机时立即撤销。
        return if owned && same_owner {
            ReflectionTextureAction::RemoveOwned
        } else if owned {
            ReflectionTextureAction::ForgetOwnership
        } else {
            ReflectionTextureAction::Ignore
        }; // 不删除其他系统后来替换的纹理。
    } // 完成资格撤销分支。
    if compatible && (same_owner || standard_active) {
        // 活动标准折射已有纹理可直接借用。
        return if owned && !same_owner {
            ReflectionTextureAction::ForgetOwnership
        } else {
            ReflectionTextureAction::Keep
        }; // 稳定帧不新建纹理。
    } // 完成复用分支。
    ReflectionTextureAction::Allocate // 新视图或实际内部尺寸变化需要完整新目标。
} // 结束所有权决策。

pub(super) fn prepare_reflection_textures(
    mut commands: Commands,    // 标准绑定组准备前原子插入或撤销完整目标。
    device: Res<RenderDevice>, // 只在规格变化时分配GPU纹理，不映射CPU。
    phases: Res<ViewSortedRenderPhases<Transmissive3d>>, // 判断本帧标准折射是否有真实消费者。
    transparent: Res<ViewSortedRenderPhases<Transparent3d>>, // 当前视图透明队列直接提供已经剔除的可见水候选。
    materials: Res<RenderMaterialInstances>, // O(1)定位候选真正材质类型，不扫描主世界实体或资产表。
    views: Query<
        (
            Entity,
            Option<&ExtractedView>,
            Option<&ViewTarget>,
            Has<WaterReflectionView>,
            Option<&ViewTransmissionTexture>,
            Option<&OwnedReflectionTexture>,
            Has<ReflectionCaptureReady>,
        ),
        Or<(
            With<WaterReflectionView>,
            With<OwnedReflectionTexture>,
            With<ReflectionCaptureReady>,
        )>,
    >, // 只访问反射租约；目标消失后仍能释放旧GPU纹理。
) {
    // 运行在标准传输资源准备之后。
    for (entity, view, target, eligible, transmission, owned, was_ready) in &views {
        // 每视图固定查询与至多一次分配。
        let same_owner = transmission
            .zip(owned)
            .is_some_and(|(current, owner)| current.texture.id() == owner.texture.id()); // ID比较避免误删标准系统的新对象。
        let (Some(view), Some(target)) = (view, target) else {
            // 窗口最小化、相机目标撤销和提取空窗都不能保留旧租约。
            if same_owner {
                commands.entity(entity).remove::<ViewTransmissionTexture>();
            } // 只释放此功能仍拥有的图片。
            if owned.is_some() {
                commands.entity(entity).remove::<OwnedReflectionTexture>();
            } // 缺目标立即释放旧强GPU句柄。
            if was_ready {
                commands.entity(entity).remove::<ReflectionCaptureReady>();
            } // 目标缺失不允许Core3d消费旧捕获票据。
            continue; // 无目标不能创建或复制图片。
        }; // 完成当前视图与目标完整性检查。
        let source = target.main_texture(); // Bevy已经提供单采样解析目标，不能取sampled_main_texture。
        let source_spec = spec(source); // 保留真实内部规格。
        let eligible = eligible && source_is_eligible(source_spec, source.usage()); // 无COPY_SRC或无效目标在分配/绑定前就撤销租约。
        let has_visible_water = eligible
            && transparent
                .get(&view.retained_view_entity)
                .is_some_and(|phase| {
                    phase.items.len() > 64
                        || phase.items.iter().any(|(_, item)| {
                            materials
                                .instances
                                .get(&item.entity.1)
                                .is_some_and(|material| is_water_material(material.asset_id))
                        })
                }); // 最多64次材质查询；大队列保守捕获，避免无水判定退化为逐帧完整实体扫描。
        if was_ready && !has_visible_water {
            commands.entity(entity).remove::<ReflectionCaptureReady>();
        } // 转向无水地面当帧停止全屏颜色copy。
        if eligible && !has_visible_water {
            continue;
        } // 保留旧GPU缓存避免转头反复分配，新视图无水不创建副本。
        let compatible = transmission.is_some_and(|current| spec(&current.texture) == source_spec); // Resize与HDR切换均需重新匹配。
        let standard_active = phases
            .get(&view.retained_view_entity)
            .is_some_and(|phase| !phase.items.is_empty()); // 不伪造折射材质来强迫标准系统复制。
        match action(
            eligible,
            owned.is_some(),
            same_owner,
            standard_active,
            compatible,
        ) {
            // 一次确定完整资源动作。
            ReflectionTextureAction::RemoveOwned => {
                commands
                    .entity(entity)
                    .remove::<ViewTransmissionTexture>()
                    .remove::<OwnedReflectionTexture>();
            } // 资格关闭只释放自有目标。
            ReflectionTextureAction::ForgetOwnership => {
                commands.entity(entity).remove::<OwnedReflectionTexture>();
            } // 新标准目标继续保留。
            ReflectionTextureAction::Ignore | ReflectionTextureAction::Keep => {} // 非目标和稳定帧不写GPU组件。
            ReflectionTextureAction::Allocate => {
                // 首次或规格变化才创建新完整目标。
                let texture = device.create_texture(&TextureDescriptor {
                    label: Some("water_reflection_scene_color"),
                    size: source_spec.size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: source_spec.format,
                    usage: TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                }); // GPU专用副本与源同尺寸同格式。
                let texture_view = texture.create_view(&TextureViewDescriptor::default()); // 一层完整颜色采样视图。
                let sampler = device.create_sampler(&SamplerDescriptor {
                    label: Some("water_reflection_sampler"),
                    mag_filter: FilterMode::Linear,
                    min_filter: FilterMode::Linear,
                    ..default()
                }); // ClampToEdge默认采样器配合shader显式屏外拒绝。
                commands.entity(entity).insert((
                    ViewTransmissionTexture {
                        texture: texture.clone(),
                        view: texture_view,
                        sampler,
                    },
                    OwnedReflectionTexture { texture },
                )); // 标准绑定组一次接收完整纹理、采样器与所有权。
            } // 完成完整目标分配。
        } // 完成本视图资源决策。
        if has_visible_water && !was_ready {
            commands.entity(entity).insert(ReflectionCaptureReady);
        } // 完整目标与当前水队列共同授权本帧唯一copy。
    } // 完成有限视图集合处理。
} // 结束SSR纹理准备。

pub(super) fn capture_reflection_color(
    view: ViewQuery<(
        &WaterReflectionView,
        &ReflectionCaptureReady,
        &ViewTarget,
        &ViewTransmissionTexture,
    )>, // 逐视图且仅当前队列有水才执行，main与F12不会串图。
    mut context: RenderContext, // 使用当前视图编码器，源绘制完成后才提交copy。
) {
    // 标准折射后、透明水面前运行一次。
    let (_, _, target, destination) = view.into_inner(); // 读取当前这一个视图的完成目标。
    let source = target.main_texture(); // 取当前ping-pong实际活动纹理，而非提前缓存另一面。
    if !source_is_eligible(spec(source), source.usage())
        || source.id() == destination.texture.id()
        || !copy_specs_match(spec(source), spec(&destination.texture))
    {
        return;
    } // 编码前再次防御源能力或规格被其他渲染系统改变。
    context.command_encoder().copy_texture_to_texture(
        source.as_image_copy(),
        destination.texture.as_image_copy(),
        source.size(),
    ); // 一次同规格GPU颜色复制，不回读CPU、不重画角色/建筑/植被。
} // 结束当前视图颜色捕获。

#[cfg(test)] // 资源生命周期纯回归不需要游戏或GPU设备。
mod tests; // 锁定标准折射共享与资格撤销边界。
