// bake/systems.rs — 固定Camera2d池的创建、排队、烘焙与复用系统。

use bevy::camera::RenderTarget; // 固定相机按当前任务切换离屏目标纹理。
use bevy::camera::visibility::RenderLayers; // 每个固定槽位使用长期稳定的隔离层。
use bevy::prelude::*; // Bevy ECS、渲染组件和资产类型。
use bevy::render::render_resource::TextureFormat; // 创建透明占位渲染目标。
use bevy::sprite::Anchor; // 让文字和装饰以离屏纹理中心为锚点。
use bevy::text::TextLayoutInfo; // 读取实际排版宽度以裁切最终网格和UV。

use super::helpers::{advance_bake_retirement, first_idle_slot}; // 复用可测试的纯状态逻辑。
use crate::components::BillboardTextLink; // 读取任务规格并写回烘焙状态。
use crate::constants::{DEFAULT_BAKE_POOL_SIZE, DEFAULT_BASE_LAYER}; // 固定池容量与隔离层范围。
use crate::helpers::{
    // 复用原有投影、网格和UV计算逻辑。
    billboard_projection,       // 让Text2d像素与目标纹理一比一对应。
    build_billboard_mesh,       // 布局完成后重建最终3D四边形。
    centered_uv_window,         // 计算居中文字对应的纹理采样窗口。
    mesh_width_for_pixel_width, // 把文字像素宽度换算为世界宽度。
};
use crate::resources::BillboardBakePool; // 全局FIFO队列和固定相机槽位。
use crate::types::BillboardBakeSlot; // Startup阶段登记每台固定相机。

/// Startup系统：只创建固定数量的Camera2d，后续所有Billboard任务都复用这些实体。
pub(crate) fn setup_billboard_bake_pool(
    mut commands: Commands,              // 创建长期存活的固定离屏相机实体。
    mut images: ResMut<Assets<Image>>,   // 创建空闲相机共用的1×1占位纹理。
    mut pool: ResMut<BillboardBakePool>, // 保存固定槽位和占位纹理句柄。
) {
    if !pool.slots.is_empty() {
        // 防止测试或插件重复初始化时创建第二组相机。
        return; // 已建立固定池时保持原相机实体不变。
    }

    let placeholder = Image::new_target_texture(
        1,                                   // 空闲相机不渲染，只需最小宽度。
        1,                                   // 空闲相机不渲染，只需最小高度。
        TextureFormat::Rgba8Unorm,           // 与正式文字目标保持一致的基础格式。
        Some(TextureFormat::Rgba8UnormSrgb), // 与正式文字目标保持一致的采样视图。
    );
    pool.placeholder_image = images.add(placeholder); // 池长期持有占位纹理，避免空闲相机引用已释放任务纹理。

    for slot_index in 0..DEFAULT_BAKE_POOL_SIZE {
        // 固定循环保证Camera2d总数与配置容量完全一致。
        let layer = DEFAULT_BASE_LAYER + slot_index; // 每个槽位永久绑定一个不同RenderLayers编号。
        let camera_entity = commands
            .spawn((
                Name::new(format!("Billboard_Bake_Camera_{slot_index}")), // 调试层级中明确标识固定槽位编号。
                Camera2d,                         // 使用2D渲染管线把Text2d写入目标Image。
                billboard_projection(UVec2::ONE), // 空闲时使用与1×1占位纹理匹配的投影。
                Camera {
                    order: -2,        // 保持原离屏相机顺序，不覆盖玩法和UI相机。
                    is_active: false, // 空闲槽位不参与视图提取，也不会挤占VisibilityRange视图预算。
                    clear_color: ClearColorConfig::Custom(Color::srgba(0.0, 0.0, 0.0, 0.0)), // 每个任务写入透明背景。
                    ..default() // 其余相机参数采用Bevy默认值。
                },
                RenderTarget::Image(pool.placeholder_image.clone().into()), // 空闲时只引用池持有的安全占位纹理。
                RenderLayers::none().with(layer), // 相机只看见当前槽位临时生成的2D源实体。
                Visibility::default(), // Camera.is_active负责实际启停，保留默认可见性传播。
            ))
            .id(); // 保存固定相机实体供后续原地切换目标。
        pool.slots.push(BillboardBakeSlot {
            camera_entity,   // 固定槽位整个游戏运行期复用该相机。
            layer,           // 当前任务2D源必须使用相同隔离层。
            billboard: None, // Startup时所有槽位均为空闲。
        });
    }
}

/// 将等待队列中的Billboard分配给空闲固定相机，并创建本次任务的临时2D源实体。
pub(crate) fn assign_billboard_bake_jobs(
    mut commands: Commands,              // 创建文字、描边和装饰等短命2D源实体。
    mut pool: ResMut<BillboardBakePool>, // 从FIFO队列取任务并占用固定槽位。
    mut cameras: Query<(&mut Camera, &mut Projection, &mut RenderTarget)>, // 原地启用固定相机并切换目标与投影。
    mut links: Query<&mut BillboardTextLink>, // 读取任务规格并记录临时源实体和动态资产。
    mut meshes: ResMut<Assets<Mesh>>,         // 创建装饰圆盘的短命2D网格。
    mut color_materials: ResMut<Assets<ColorMaterial>>, // 创建装饰圆盘的短命2D材质。
) {
    while let Some(slot_index) = first_idle_slot(&pool.slots) {
        // 一帧内填满所有空闲槽位，但绝不超过固定池容量。
        let Some(billboard) = pop_next_live_billboard(&mut pool.waiting, &links) else {
            // 跳过排队期间已被销毁的旧实体。
            break; // 队列已空时结束本帧分配。
        };
        let camera_entity = pool.slots[slot_index].camera_entity; // 复制实体后释放对slots的临时借用。
        let layer = pool.slots[slot_index].layer; // 当前任务所有2D源都使用槽位固定隔离层。
        let Ok(mut link) = links.get_mut(billboard) else {
            // 延迟命令可能让任务在同帧失效。
            continue; // 失效任务不占槽位，继续尝试下一个排队项。
        };
        let Some(spec) = link.bake_spec.clone() else {
            // 已完成或被取消的任务没有烘焙规格。
            continue; // 跳过重复队列项并保持槽位空闲。
        };
        let Ok((mut camera, mut projection, mut target)) = cameras.get_mut(camera_entity) else {
            // 固定相机异常缺失时不能执行任务。
            pool.waiting.push_front(billboard); // 把任务放回队首，避免静默丢失名字。
            break; // 停止分配，等待下一帧或诊断固定池状态。
        };

        *target = RenderTarget::Image(link.image.clone().into()); // Bevy 0.18将目标作为独立组件，原地切到当前Billboard纹理。
        camera.is_active = true; // 只有占用槽位时相机才进入渲染视图集合。
        *projection = billboard_projection(spec.texture_size); // 投影尺寸匹配当前任务纹理，保持像素一比一。

        let mut decoration_entities = Vec::new(); // 记录短命装饰实体以便完成或取消时清理。
        let mut decoration_meshes = Vec::new(); // 记录短命网格句柄以便烘焙后立即释放资产。
        let mut decoration_materials = Vec::new(); // 记录短命材质句柄以便烘焙后立即释放资产。
        for decoration in spec.decorations.iter() {
            // 按规格顺序创建文字背后的圆盘装饰。
            let decoration_mesh = meshes.add(Circle::new(decoration.radius_px)); // 圆盘半径直接使用离屏像素坐标。
            let decoration_material = color_materials.add(ColorMaterial::from(decoration.color)); // 无光照2D材质写入指定颜色。
            let decoration_entity = commands
                .spawn((
                    Name::new("Billboard_Decoration"), // 调试层级标识临时装饰源。
                    Mesh2d(decoration_mesh.clone()),   // 2D圆盘网格只在本次烘焙期间存在。
                    MeshMaterial2d(decoration_material.clone()), // 2D颜色材质只在本次烘焙期间存在。
                    Transform::from_xyz(0.0, 0.0, decoration.z), // z越大越靠近文字但始终低于文字层级。
                    RenderLayers::none().with(layer),            // 只允许当前固定槽位相机看见。
                    Visibility::default(),                       // 活跃相机负责将其写入目标纹理。
                ))
                .id(); // 保存实体供完成或取消时幂等销毁。
            decoration_entities.push(decoration_entity); // 登记当前任务装饰实体。
            decoration_meshes.push(decoration_mesh); // 登记当前任务动态网格资产。
            decoration_materials.push(decoration_material); // 登记当前任务动态材质资产。
        }

        let mut outline_entities = Vec::new(); // 记录八方向描边文字源实体。
        if let Some(outline) = &spec.outline {
            // 只有调用方配置描边时才创建额外文字源。
            for &(dx, dy) in &[
                (-outline.width_px, 0.0),               // 左侧描边偏移。
                (outline.width_px, 0.0),                // 右侧描边偏移。
                (0.0, -outline.width_px),               // 下侧描边偏移。
                (0.0, outline.width_px),                // 上侧描边偏移。
                (-outline.width_px, -outline.width_px), // 左下描边偏移。
                (-outline.width_px, outline.width_px),  // 左上描边偏移。
                (outline.width_px, -outline.width_px),  // 右下描边偏移。
                (outline.width_px, outline.width_px),   // 右上描边偏移。
            ] {
                let outline_entity = commands
                    .spawn((
                        Name::new("Billboard_Text_Outline"), // 调试层级标识临时描边文字。
                        Text2d::new(spec.text.clone()),      // 描边复用主文字内容。
                        TextFont {
                            font: spec.font.clone().into(),   // 描边必须使用与主文字相同字体。
                            font_size: spec.font_size.into(), // 描边必须使用与主文字相同字号。
                            ..default()                       // 其余字体参数保持默认。
                        },
                        TextColor(outline.color), // 描边使用调用方配置颜色。
                        Anchor::CENTER,           // 所有描边围绕纹理中心偏移。
                        Transform::from_xyz(dx, dy, 90.0), // 描边在装饰之上、主文字之下。
                        RenderLayers::none().with(layer), // 只允许当前固定槽位相机看见。
                        Visibility::default(),    // 活跃相机负责将其写入目标纹理。
                    ))
                    .id(); // 保存实体供烘焙完成时销毁。
                outline_entities.push(outline_entity); // 登记当前任务描边源。
            }
        }

        let mut text_commands = commands.spawn((
            Name::new("Billboard_Text"),    // 调试层级标识主文字源。
            Text2d::new(spec.text.clone()), // 写入本次Billboard文字内容。
            TextFont {
                font: spec.font.clone().into(),   // 使用调用方字体资产。
                font_size: spec.font_size.into(), // 使用调用方烘焙字号。
                ..default()                       // 其余字体参数采用Bevy默认值。
            },
            TextColor(spec.color),                // 主文字使用调用方颜色。
            Anchor::CENTER,                       // 主文字居中写入目标纹理。
            Transform::from_xyz(0.0, 0.0, 100.0), // 主文字位于所有描边和装饰之上。
            RenderLayers::none().with(layer),     // 只允许当前固定槽位相机看见。
            Visibility::default(),                // 活跃相机负责将其写入目标纹理。
        ));
        if let Some(shadow) = spec.shadow {
            // 可选阴影与旧实现保持一致。
            text_commands.insert(shadow); // 阴影随主文字在同一固定槽位中烘焙。
        }
        let text_entity = text_commands.id(); // 保存主文字实体以读取TextLayoutInfo并清理。

        link.text_entity = Some(text_entity); // 标记任务已经获得相机槽位并创建主文字源。
        link.outline_entities = outline_entities; // 保存当前任务所有描边实体。
        link.decoration_entities = decoration_entities; // 保存当前任务所有装饰实体。
        link.decoration_meshes = decoration_meshes; // 保存当前任务动态装饰网格。
        link.decoration_materials = decoration_materials; // 保存当前任务动态装饰材质。
        link.bake_frames_remaining = 2; // 为文字布局和最终纹理写入保留完整帧。
        pool.slots[slot_index].billboard = Some(billboard); // 最后占用槽位，保证前面失败不会留下半任务。
    }
}

/// 等待活跃任务的文字布局完成，并按真实内容宽度更新最终3D网格与UV。
pub fn sync_billboard_layouts(
    pool: Res<BillboardBakePool>, // 只扫描固定数量的活跃任务，不遍历全部已完成Billboard。
    mut links: Query<(&mut BillboardTextLink, &Mesh3d)>, // 更新布局状态并取得最终网格句柄。
    text_layouts: Query<&TextLayoutInfo>, // 读取Bevy排版系统生成的实际文字尺寸。
    mut meshes: ResMut<Assets<Mesh>>, // 原地替换最终3D网格的宽度和UV。
) {
    for slot in pool.slots.iter() {
        // 活跃任务数永远不超过固定相机池容量。
        let Some(billboard) = slot.billboard else {
            // 空闲槽位没有需要同步的布局。
            continue; // 继续检查下一个固定槽位。
        };
        let Ok((mut link, mesh_handle)) = links.get_mut(billboard) else {
            // 任务可能被业务层提前销毁。
            continue; // 槽位回收系统会在同帧安全释放该相机。
        };
        if !link.needs_layout_sync {
            // 非裁切模式或已经完成布局的任务无需重复处理。
            continue; // 保留当前最终网格。
        }
        let Some(text_entity) = link.text_entity else {
            // 尚未创建主文字源时没有布局可读。
            continue; // 等待任务分配系统完成源实体创建。
        };
        let Ok(layout_info) = text_layouts.get(text_entity) else {
            // TextLayoutInfo通常在下一帧才生成。
            continue; // 保持隐藏并等待下一轮Update。
        };
        if layout_info.size.x <= 0.0 || layout_info.size.y <= 0.0 {
            // 无效尺寸不能用于裁切。
            continue; // 等字体资产或排版结果稳定。
        }

        let effective_pixel_width = // 加上水平留白并限制在目标纹理范围内。
            (layout_info.size.x + link.padding_px * 2.0).clamp(1.0, link.texture_width);
        let mesh_width = mesh_width_for_pixel_width(
            effective_pixel_width, // 当前文字实际占用的像素宽度。
            link.texture_height,   // 纹理高度用于换算每像素世界尺寸。
            link.world_height,     // 调用方指定的最终世界高度。
        );
        let (u_min, u_max) = centered_uv_window(effective_pixel_width, link.texture_width); // 只采样纹理中间有效文字区域。
        if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
            // 最终动态网格仍存在时原地更新。
            *mesh = build_billboard_mesh(mesh_width, link.world_height, u_min, u_max); // 保持文字比例并移除多余透明边缘。
            link.needs_layout_sync = false; // 下一帧开始进入最终纹理保留计数。
        }
    }
}

/// 最终纹理稳定后释放短命2D源，并让同一固定相机槽位立即处理下一个任务。
pub(crate) fn finish_billboard_bakes(
    mut commands: Commands, // 幂等销毁当前任务的文字、描边和装饰源实体。
    mut pool: ResMut<BillboardBakePool>, // 释放已完成或失效任务占用的固定槽位。
    mut cameras: Query<(&mut Camera, &mut RenderTarget)>, // 空闲时停用相机并恢复安全占位目标。
    mut links: Query<&mut BillboardTextLink>, // 标记最终纹理就绪并清空烘焙期引用。
    mut meshes: ResMut<Assets<Mesh>>, // 完成后立即释放装饰短命网格资产。
    mut color_materials: ResMut<Assets<ColorMaterial>>, // 完成后立即释放装饰短命材质资产。
) {
    let placeholder_image = pool.placeholder_image.clone(); // 循环前克隆句柄，避免同时借用pool字段。
    for slot in pool.slots.iter_mut() {
        // 固定数量循环保证完成阶段开销有明确上限。
        let Some(billboard) = slot.billboard else {
            // 空闲槽位无需处理。
            continue; // 检查下一固定槽位。
        };
        let Ok(mut link) = links.get_mut(billboard) else {
            // 业务层可能在烘焙中途删除Billboard。
            release_camera_slot(slot, &placeholder_image, &mut cameras); // 立即停用固定相机，避免继续引用已释放纹理。
            continue; // 失效任务没有Link可继续清理。
        };
        if !advance_bake_retirement(link.needs_layout_sync, &mut link.bake_frames_remaining) {
            // 未完成布局或最终帧时继续占用槽位。
            continue; // 当前帧末尾固定相机仍需写入目标纹理。
        }

        retire_bake_sources(
            &mut commands,        // 删除本任务短命2D源实体。
            &mut meshes,          // 删除本任务短命装饰网格。
            &mut color_materials, // 删除本任务短命装饰材质。
            &mut link,            // 清空源实体和规格引用并标记就绪。
        );
        release_camera_slot(slot, &placeholder_image, &mut cameras); // 相机实体不销毁，只停用并归还固定槽位。
    }
}

/// PostUpdate兜底：文字清理的延迟销毁落地后，在相机目标校验和可见性计算前归还孤儿槽位。
pub(crate) fn release_orphaned_bake_slots(
    mut pool: ResMut<BillboardBakePool>, // 扫描固定数量的活跃槽位并释放失效任务。
    links: Query<(), With<BillboardTextLink>>, // 只判断槽位记录的Billboard是否仍存在。
    mut cameras: Query<(&mut Camera, &mut RenderTarget)>, // 立即停用相机并切回安全占位纹理。
) {
    let placeholder_image = pool.placeholder_image.clone(); // 循环前克隆句柄，避免同时借用pool字段。
    for slot in pool.slots.iter_mut() {
        // 固定容量循环不会随Billboard总量增长。
        let Some(billboard) = slot.billboard else {
            // 空闲槽位没有孤儿任务。
            continue; // 检查下一固定槽位。
        };
        if links.get(billboard).is_err() {
            // Update业务清理或PostUpdate文字更新已经移除了活跃Billboard实体。
            release_camera_slot(slot, &placeholder_image, &mut cameras); // 渲染提取前停用相机，避免引用已释放目标纹理。
        }
    }
}

/// 从FIFO队列弹出下一个仍存在的Billboard，排队期间被删除的实体直接丢弃。
fn pop_next_live_billboard(
    waiting: &mut std::collections::VecDeque<Entity>, // 固定池共享等待队列。
    links: &Query<&mut BillboardTextLink>,            // 查询任务实体是否仍有运行时Link。
) -> Option<Entity> {
    while let Some(billboard) = waiting.pop_front() {
        // 严格按请求进入顺序检查。
        if links.get(billboard).is_ok() {
            // 只有仍存在的Billboard才能占用槽位。
            return Some(billboard); // 返回第一个有效任务。
        }
    }
    None // 队列中没有可执行任务。
}

/// 释放固定相机槽位；只切换状态和目标，绝不销毁Camera2d实体。
fn release_camera_slot(
    slot: &mut BillboardBakeSlot,      // 当前完成或失效任务占用的槽位。
    placeholder_image: &Handle<Image>, // 池长期持有的安全空闲目标。
    cameras: &mut Query<(&mut Camera, &mut RenderTarget)>, // 原地修改固定相机及其独立目标组件。
) {
    if let Ok((mut camera, mut target)) = cameras.get_mut(slot.camera_entity) {
        // 正常情况下固定相机始终存在。
        camera.is_active = false; // 空闲相机不参与视图提取和VisibilityRange计算。
        *target = RenderTarget::Image(placeholder_image.clone().into()); // 去掉对任务纹理的引用并恢复安全占位目标。
    }
    slot.billboard = None; // 无论相机查询是否成功，都释放逻辑槽位避免队列永久堵塞。
}

/// 销毁已写入纹理的2D源资产，并把Billboard切换为可显示的Ready状态。
fn retire_bake_sources(
    commands: &mut Commands,   // 延迟销毁Text2d、描边和装饰实体。
    meshes: &mut Assets<Mesh>, // 立即移除装饰短命网格句柄。
    color_materials: &mut Assets<ColorMaterial>, // 立即移除装饰短命材质句柄。
    link: &mut BillboardTextLink, // 清空本次任务源引用并保留最终3D资产。
) {
    if let Some(text_entity) = link.text_entity.take() {
        // 取出后保证后续外部清理不会重复引用旧实体。
        commands.entity(text_entity).try_despawn(); // 主文字像素已进入最终纹理，可安全删除源实体。
    }
    for outline_entity in link.outline_entities.drain(..) {
        // 清空所有八方向描边源引用。
        commands.entity(outline_entity).try_despawn(); // 描边像素已进入最终纹理，可安全删除源实体。
    }
    for decoration_entity in link.decoration_entities.drain(..) {
        // 清空所有装饰源引用。
        commands.entity(decoration_entity).try_despawn(); // 装饰像素已进入最终纹理，可安全删除源实体。
    }
    for decoration_mesh in link.decoration_meshes.drain(..) {
        // 清空短命装饰网格句柄。
        meshes.remove(decoration_mesh.id()); // 完成后立即释放资产，避免等待Billboard最终销毁。
    }
    for decoration_material in link.decoration_materials.drain(..) {
        // 清空短命装饰材质句柄。
        color_materials.remove(decoration_material.id()); // 完成后立即释放资产，避免长期累计。
    }
    link.bake_spec = None; // 不再保留文字字符串、字体和装饰规格的额外引用。
    link.bake_ready = true; // 更新系统从本帧开始允许显示，并从下一次调用开始计算TTL。
}

#[cfg(test)] // ECS集成测试验证Startup确实只创建固定数量且默认停用的Camera2d实体。
mod tests {
    use bevy::prelude::*; // App、Assets、Camera和Camera2d测试所需类型。

    use super::setup_billboard_bake_pool; // 直接运行生产环境使用的固定池初始化系统。
    use crate::constants::DEFAULT_BAKE_POOL_SIZE; // 校验默认池容量没有被实现偏离。
    use crate::resources::BillboardBakePool; // 读取系统初始化后的槽位登记状态。

    #[test] // 大量Billboard出现前，插件只应预建四台空闲离屏相机。
    fn setup_creates_exactly_four_inactive_bake_cameras() {
        let mut app = App::new(); // 使用最小ECS App，不启动窗口和GPU渲染器。
        app.init_resource::<Assets<Image>>(); // Startup系统需要登记1×1占位纹理。
        app.init_resource::<BillboardBakePool>(); // Startup系统需要写入固定槽位资源。
        app.add_systems(Startup, setup_billboard_bake_pool); // 注册与正式插件相同的初始化系统。
        app.update(); // 执行一次Startup并应用相机生成命令。

        let slot_cameras: Vec<Entity> = app // 先复制槽位相机标识，避免后续可变World查询借用冲突。
            .world()
            .resource::<BillboardBakePool>() // 读取已经完成初始化的固定池。
            .slots
            .iter() // 遍历所有固定槽位。
            .map(|slot| slot.camera_entity) // 只保留每个槽位绑定的相机实体。
            .collect(); // 得到可与World查询结果比较的实体列表。
        assert_eq!(slot_cameras.len(), DEFAULT_BAKE_POOL_SIZE); // 槽位总数必须严格等于4。

        let world = app.world_mut(); // 取得可变World以创建Camera查询状态。
        let mut camera_query = world.query_filtered::<(Entity, &Camera), With<Camera2d>>(); // 只统计池创建的2D相机。
        let bake_cameras: Vec<(Entity, bool)> = camera_query // 收集实体与活动状态，避免持有查询借用。
            .iter(world) // 遍历Startup实际生成的全部Camera2d。
            .map(|(entity, camera)| (entity, camera.is_active)) // 保存相机标识和是否参与渲染。
            .collect(); // 完成独立快照便于断言。

        assert_eq!(bake_cameras.len(), DEFAULT_BAKE_POOL_SIZE); // ECS世界中不得额外生成逐Billboard相机。
        assert!(bake_cameras.iter().all(|(_, is_active)| !is_active)); // 空闲池相机默认不参与视图提取。
        assert!(slot_cameras.iter().all(
            |entity| // 每个资源槽位必须准确指向World中的固定相机。
            bake_cameras.iter().any(|(camera, _)| camera == entity) // 槽位引用不得指向其他实体。
        ));
    }
}
