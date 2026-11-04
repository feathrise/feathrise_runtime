// lib.rs — feathrise_billboard_text：通用「离屏烘焙文字 Billboard」插件
//
// 把任意文字烘焙到离屏纹理，再以世界空间 3D Billboard 显示：
//   Text2d → RenderTarget::Image → 3D Billboard（朝向相机、近大远小、距离裁剪、可选 TTL）。
//
// 用法：
//   1. app.add_plugins(BillboardTextPlugin);
//   2. 给主 3D 相机加上 `BillboardCamera` 标记组件；
//   3. 在任意实体上插入 `BillboardText { text, font, ..default() }` 即可。
//      （follow=true 时跟随该实体头顶；ttl=Some(t) 时 t 秒后自动销毁。）
//
// 本 crate 完全解耦，不依赖任何游戏业务 crate，可独立开源。

mod bake; // 固定Camera2d池：排队、布局、最终渲染和槽位复用。
mod components; // 组件：BillboardCamera、BillboardText、BillboardTextLink
mod constants; // 默认参数常量
mod helpers; // 纯函数：投影、网格、UV 计算
mod materials; // 最终透明文字与时间重建引导材质。
mod resources; // 资源：固定烘焙相机池和FIFO等待队列。
mod shaders; // 内置PBR兼容顶点Shader，按普通/独立WorldUI选择投影。
mod systems; // ECS 系统：spawn / sync / update
mod types; // 非ECS类型：固定相机槽位。

pub use bake::sync_billboard_layouts;
pub use components::{
    BillboardCamera,
    BillboardDecoration,
    BillboardTemporalGuide,
    BillboardTemporalGuideSuppressed, // 独立 UI 合成可显式暂停相机字形时间引导。
    BillboardText,
    BillboardTextHandle,
    BillboardTextLink,
    BillboardTextOutline,
};
pub use constants::{
    DEFAULT_BAKE_POOL_SIZE, DEFAULT_BASE_LAYER, DEFAULT_FONT_SIZE, DEFAULT_HORIZONTAL_PADDING_PX,
    DEFAULT_MAX_DISTANCE, DEFAULT_REF_DISTANCE, DEFAULT_TEXTURE_HEIGHT, DEFAULT_TEXTURE_WIDTH,
    DEFAULT_WORLD_HEIGHT,
};
pub use helpers::{
    billboard_projection, build_billboard_mesh, centered_uv_window, mesh_width_for_pixel_width,
};
pub use materials::{
    BILLBOARD_GUIDE_GATE, // RenderWorld按实际分流结果驱动引导，不能提前按请求隐藏。
    BILLBOARD_PROJECTION_JITTERED,
    BILLBOARD_PROJECTION_UNJITTERED,
    BILLBOARD_TEMPORAL_GUIDE_ALPHA_CUTOFF,
    BillboardColorExtension,
    BillboardColorMaterial,
    BillboardTemporalGuideExtension,
    BillboardTemporalGuideMaterial,
};
pub use resources::BillboardBakePool; // 公开系统参数类型，但槽位、队列和相机实体仍由插件内部维护。
pub use systems::{
    BillboardAssetStores, BillboardTextSystems, spawn_billboard_texts, update_billboard_texts,
};

use bevy::camera::CameraUpdateSystems; // 动态文字纹理回收后必须先修复固定相机目标，再进入Bevy严格目标校验。
use bevy::prelude::*;

/// 文字清理的延迟销毁落地后再归还固定槽位，且必须早于Bevy相机目标校验。
fn configure_bake_target_retirement(app: &mut App) {
    app.add_systems(
        PostUpdate,
        bake::release_orphaned_bake_slots
            .after(systems::update_billboard_texts) // 普通after自动应用文字清理的Commands，防止退休门先看到旧Link、后续却回收其Image。
            .before(CameraUpdateSystems) // `Assets<Image>`允许业务层立即回收；同帧先切回占位图，不能让camera_system观察到悬空句柄。
            .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility), // 相机停用后才允许本帧可见性收集读取视图集合。
    );
}

/// 相机和宿主在Update完成运动后再同步文字，且必须早于父子变换传播和引导显隐。
fn configure_billboard_pose_update(app: &mut App) {
    // 普通场景与独立WorldUI共用同一份本帧姿态。
    app.add_systems(
        PostUpdate,
        systems::update_billboard_texts // 不与Update中的相机旋转争抢执行顺序。
            .before(bevy::transform::TransformSystems::Propagate) // 保证最终网格和引导子实体同帧取得新的GlobalTransform。
            .before(systems::BillboardTextSystems::SyncTemporalGuides) // 清理和显隐先于引导同步。
            .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
    ); // 裁剪读取更新后的文字显隐。
} // 不增加遍历次数，仅调整现有系统的执行阶段。

/// 通用 Billboard 文字插件。
pub struct BillboardTextPlugin;

impl Plugin for BillboardTextPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<Assets<bevy::render::storage::ShaderBuffer>>()
            .insert(
                BILLBOARD_GUIDE_GATE.id(),
                bevy::render::storage::ShaderBuffer::from(Vec4::ZERO),
            )
            .expect("固定Billboard引导缓冲登记失败"); // 固定共享GPU信号初值为普通引导，不随实体增长。
        shaders::register_billboard_shaders(app); // 先登记顶点入口，再让材质插件建立异步管线。
        app.add_plugins(MaterialPlugin::<BillboardColorMaterial>::default());
        app.add_plugins(MaterialPlugin::<BillboardTemporalGuideMaterial>::default());
        app.init_resource::<resources::BillboardBakePool>(); // 初始化等待队列，Startup再创建固定相机实体。
        app.init_resource::<systems::BillboardTemporalGuideState>(); // 只在时间链状态切换时批量更新引导层。
        app.add_systems(Startup, bake::setup_billboard_bake_pool); // 全程只保留固定数量的Camera2d烘焙槽位。
        app.add_systems(
            Update,
            (
                systems::spawn_billboard_texts,
                bake::sync_billboard_layouts, // 先推进上一帧活跃任务的真实文字布局。
                bake::finish_billboard_bakes, // 完成任务先归还槽位，使其可在同帧复用。
                bake::assign_billboard_bake_jobs, // FIFO队列只填充固定数量的空闲相机。
            )
                .chain(),
        );
        app.add_systems(
            PostUpdate,
            systems::sync_billboard_temporal_guides
                .in_set(systems::BillboardTextSystems::SyncTemporalGuides)
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        );
        configure_bake_target_retirement(app); // 覆盖Update业务回收及PostUpdate文字清理，在相机校验前统一接管。
        configure_billboard_pose_update(app); // 文字不再偶发读取上一帧相机朝向。
    }
}

#[cfg(test)] // 独立验证生产调度的同帧旋转契约。
mod pose_tests; // 不启动窗口或GPU。

#[cfg(test)] // 用真实Bevy相机系统验证PostUpdate文字清理与固定槽位退休的顺序。
mod camera_retirement_tests;

#[cfg(test)] // 轻量调度回归不启动窗口或GPU，只验证悬空任务目标在Bevy相机阶段前已经恢复。
mod tests {
    use bevy::camera::{CameraUpdateSystems, RenderTarget}; // 使用正式相机更新集合模拟camera_system的严格目标检查。
    use bevy::prelude::*; // App、Assets、Image、Camera和PostUpdate测试所需类型。

    use super::configure_bake_target_retirement; // 测试与生产插件复用完全相同的调度注册入口。
    use crate::resources::BillboardBakePool; // 构造一个仍记录已销毁任务的固定相机槽位。
    use crate::types::BillboardBakeSlot; // 槽位身份必须与生产资源完全一致。

    #[derive(Resource, Default)] // 验证系统确实运行，并且没有因断言失败提前退出。
    struct CameraTargetValidationProbe(bool); // true表示模拟Bevy相机阶段已经看到全部有效Image目标。

    /// 模拟Bevy 0.19 camera_system对每个Image目标执行的主世界资产存在性校验。
    fn validate_camera_image_targets(
        images: Res<Assets<Image>>,    // 目标必须仍存在于主世界Assets<Image>。
        targets: Query<&RenderTarget>, // 固定烘焙相机已经由退休门原地改写目标。
        mut probe: ResMut<CameraTargetValidationProbe>, // 记录严格校验系统完成。
    ) {
        for target in &targets {
            if let Some(image) = target.as_image() {
                assert!(
                    images.get(image).is_some(),
                    "CameraUpdateSystems不得观察到已从Assets<Image>移除的渲染目标"
                ); // 旧顺序会在这里稳定复现用户删除地图后的同类崩溃。
            }
        }
        probe.0 = true; // 全部相机目标通过后发布测试证据。
    }

    #[test] // 地图切换销毁正在烘焙的名牌时，相机必须先脱离已回收的任务纹理。
    fn orphaned_bake_target_retires_before_bevy_camera_validation() {
        let mut app = App::new(); // 最小ECS App足以验证PostUpdate调度，不初始化RenderApp或窗口。
        app.init_resource::<Assets<Image>>() // 运行时占位图和待回收任务图共用正式资产表。
            .init_resource::<BillboardBakePool>() // 退休系统读取固定槽位和长期占位图。
            .init_resource::<CameraTargetValidationProbe>(); // 模拟相机阶段写入完成证据。
        configure_bake_target_retirement(&mut app); // 使用生产插件实际采用的相机校验前排序。
        app.add_systems(
            PostUpdate,
            validate_camera_image_targets.in_set(CameraUpdateSystems), // 若退休门排序丢失，本系统会先看到悬空任务图并失败。
        );

        let (placeholder, retired_target) = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>(); // 一次性登记两张轻量测试图。
            (images.add(Image::default()), images.add(Image::default())) // 占位图保持有效，任务图随后模拟业务即时回收。
        };
        let camera = app
            .world_mut()
            .spawn((
                Camera::default(), // 固定池相机停用与否不影响camera_system对变化目标的完整性要求。
                RenderTarget::from(retired_target.clone()), // 初始仍指向即将被删除的名牌纹理。
            ))
            .id(); // 保存槽位长期绑定的相机身份。
        let retired_billboard = app.world_mut().spawn_empty().id(); // 先取得一个真实但即将失效的任务实体身份。
        app.world_mut().despawn(retired_billboard); // 模拟地图离场已经销毁NPC及其Billboard任务。
        {
            let mut pool = app.world_mut().resource_mut::<BillboardBakePool>(); // 填入旧Update留下的跨阶段槽位记录。
            pool.placeholder_image = placeholder.clone(); // 空闲目标由池长期强持有。
            pool.slots.push(BillboardBakeSlot {
                camera_entity: camera,              // 退休门应原地修复这一固定相机。
                layer: 0,                           // 渲染层与本次目标生命周期验证无关。
                billboard: Some(retired_billboard), // 槽位尚未来得及消费业务实体销毁。
            });
        }
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .remove(retired_target.id()); // 复现业务清理先回收动态纹理、固定相机仍引用旧句柄的危险窗口。

        app.world_mut().run_schedule(PostUpdate); // 退休门必须先于模拟CameraUpdateSystems执行。

        assert!(app.world().resource::<CameraTargetValidationProbe>().0); // 确认严格相机阶段确实完成。
        assert_eq!(
            app.world()
                .get::<RenderTarget>(camera)
                .and_then(RenderTarget::as_image),
            Some(&placeholder)
        ); // 固定相机已转交到仍存在的长期占位图，而非只靠停用掩盖悬空目标。
        assert!(
            app.world()
                .resource::<BillboardBakePool>()
                .slots
                .iter()
                .all(|slot| slot.billboard.is_none())
        ); // 旧任务身份同步释放，下一张地图的名牌可以复用同一槽位。
    }
}
