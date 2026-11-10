# feathrise_billboard_text

通用「离屏烘焙文字 Billboard」插件，面向 [Bevy](https://bevyengine.org) 0.19。

把任意文字（可叠加彩色底盘装饰）烘焙到离屏纹理，再以**世界空间 3D Billboard** 显示在某个实体头顶或固定世界坐标：

- **朝向相机**：始终正对摄像机，永远可读；
- **近大远小**：可选距离缩放，远处自动缩小；
- **距离裁剪**：超出最大距离自动隐藏；
- **可选 TTL**：到时自动销毁，适合临时提示；
- **复合徽标**：文字背后可叠放多层圆盘（底盘 + 描边圈 + 文字）；
- **固定相机池**：默认仅4台离屏 `Camera2d`，大量名字通过 FIFO 队列分批烘焙；
- **时间重建引导**：Deferred + Motion + TemporalJitter 主相机自动获得字形深度、GBuffer 与运动向量，抑制 DLSS RR 奔跑残影；
- **完全解耦**：仅依赖 `bevy`，可独立使用（MIT OR Apache-2.0）。

## 原理

```text
BillboardText 请求 ──► FIFO 等待队列
                            │ 每次最多4个活跃任务
                            ▼
                   固定 Camera2d 槽位池
                            │ RenderTarget::Image
Text2d (+ 装饰圆盘 Mesh2d) ─┤
                            ▼
                       离屏纹理 Image
                            ├─ BillboardColorMaterial(StandardMaterial + 投影切换) ─► 最终透明文字
                            │
                            └─ AlphaMask Deferred 子层 ───────► GBuffer / Depth / Motion 引导
```

相机槽位在插件启动时一次创建并长期复用；任务烘焙完成后只销毁短命 `Text2d`、描边和装饰源。排队与烘焙期间不计算 `ttl`，避免短命战斗文字尚未显示就过期。

宿主消失或 TTL 到期的清理在 `PostUpdate` 文字更新中执行；固定槽位退休必须排在它之后，并让延迟实体销毁先落地，再停用相机、切回长期占位图，最后进入 `CameraUpdateSystems`。Bevy 也会检查停用相机的目标元数据，不能仅靠 `is_active = false` 留下已回收的 Image 句柄。退休仍只检查固定4个槽位，不增加相机或纹理分配。

最终文字继续使用原有 Blend 软边。只有唯一活动 `BillboardCamera` 同时具备 `DeferredPrepass`、`MotionVectorPrepass` 和 `TemporalJitter` 时，插件才显示共享网格/纹理的 AlphaMask 引导子层；该层使用独立 RenderLayer，不进入小场景、角色预览或其他 Forward 相机。时间链关闭后，插件会隐藏全部引导并精确归还主相机原有 RenderLayers。工程启用外部 WorldUI 分流时可把 `BillboardColorExtension` 切到 `BILLBOARD_PROJECTION_UNJITTERED`，让输出分辨率合成避免再次叠加时间抖动。

## 快速开始

```toml
# Cargo.toml
[dependencies]
bevy = "0.19"
feathrise_billboard_text = "0.1"
```

```rust
use bevy::prelude::*;
use feathrise_billboard_text::{BillboardCamera, BillboardText, BillboardTextPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(BillboardTextPlugin) // 1. 注册插件
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    // 2. 给主 3D 相机加上 `BillboardCamera` 标记
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 3.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
        BillboardCamera,
    ));

    // 3. 在任意实体上插入 `BillboardText` 即可（处理后该组件被自动移除）
    commands.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        Visibility::default(),
        BillboardText {
            text: "Hello, Billboard!".to_string(),
            ..default() // font/字号/颜色等都有合理默认值
        },
    ));
}
```

## 常见用法

### 头顶名牌（跟随实体）

```rust
commands.entity(npc).insert(BillboardText {
    text: name.to_string(),
    follow: true,        // 跟随宿主头顶（默认）
    y_offset: 2.2,       // 头顶偏移（米）
    ..default()
});
```

### 用独立锚点承载请求，但 Billboard 跟随另一个对象

适合「锚点实体只负责生成 Billboard，实际跟随 NPC」的解耦写法：

```rust
let anchor = commands
    .spawn((Transform::default(), Visibility::default()))
    .id();

commands.entity(anchor).insert(BillboardText {
    text: "!".to_string(),
    follow_target: Some(npc), // 跟随 npc，而非锚点自身
    ..default()
});
```

### 临时飘字（TTL 自动销毁）

```rust
commands.spawn((
    Transform::from_translation(world_pos),
    Visibility::default(),
    BillboardText {
        text: "Quest Complete!".to_string(),
        follow: false,           // 固定在生成时的世界坐标
        ttl: Some(2.0),          // 2 秒后自动销毁
        ..default()
    },
));
```

### 复合徽标（底盘 + 描边圈 + 文字）

`crop_to_text = false` 时使用整张正方形纹理；`decorations` 按 `z` 由低到高叠放，文字始终在最上层：

```rust
use feathrise_billboard_text::BillboardDecoration;

commands.entity(anchor).insert(BillboardText {
    text: "!".to_string(),
    follow_target: Some(npc),
    texture_size: UVec2::splat(128),
    world_height: 0.42,
    crop_to_text: false, // 内容已填满纹理，按方形映射
    decorations: vec![
        BillboardDecoration { radius_px: 60.0, color: Color::srgb(0.1, 0.1, 0.1), z: 0.0 }, // 描边圈
        BillboardDecoration { radius_px: 52.0, color: Color::srgb(0.9, 0.8, 0.2), z: 1.0 }, // 底盘
    ],
    ..default()
});
```

### 获取生成出的 Billboard 实体（用于销毁/重建）

`BillboardText` 被消费后，插件会把 `BillboardTextHandle(billboard_entity)` 回写到承载请求的实体上：

```rust
use feathrise_billboard_text::{BillboardTextHandle, BillboardTextLink};

fn despawn_billboard(
    mut commands: Commands,
    anchors: Query<&BillboardTextHandle>,
    links: Query<&BillboardTextLink>,
    anchor: Entity,
) {
    if let Ok(handle) = anchors.get(anchor) {
        if let Ok(link) = links.get(handle.0) {
            // 固定烘焙相机属于插件全局池，不得随单块 Billboard 销毁。
            if let Some(text_entity) = link.text_entity {
                commands.entity(text_entity).despawn();
            }
            for outline in &link.outline_entities {
                commands.entity(*outline).despawn();
            }
            for d in &link.decoration_entities {
                commands.entity(*d).despawn();
            }
            commands.entity(handle.0).despawn();
        }
        commands.entity(anchor).despawn();
    }
}
```

## `BillboardText` 字段一览

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `text` | `""` | 显示文字 |
| `font` | 内置默认字体 | 字体句柄 |
| `font_size` | `80.0` | 烘焙字号（仅影响纹理清晰度） |
| `color` | 白色 | 文字颜色 |
| `shadow` | 黑色软阴影 | 可选文字阴影 |
| `texture_size` | `1024 × 128` | 离屏纹理尺寸（像素） |
| `world_height` | `0.24` | 世界高度（米），宽度按文字像素宽推导 |
| `y_offset` | `0.0` | 头顶/锚点 Y 偏移（米） |
| `scale` | `1.0` | 额外世界尺寸缩放 |
| `follow` | `true` | 跟随宿主头顶 / 固定世界坐标 |
| `follow_target` | `None` | 覆盖跟随目标（`None` = 跟随承载请求的实体） |
| `distance_scaling` | `true` | 近大远小 |
| `ref_distance` | `8.0` | 小于该距离不再放大 |
| `max_distance` | `50.0` | 超过则隐藏 |
| `ttl` | `None` | 存活秒数（`None` = 常驻） |
| `padding_px` | `24.0` | 水平内边距（像素），避免阴影被裁切 |
| `decorations` | `[]` | 文字背后的装饰圆盘 |
| `crop_to_text` | `true` | 按文字真实宽度裁切 / 整张方形纹理映射 |

## 运行示例

```bash
cargo run -p feathrise_billboard_text --example basic
```

## 许可

`MIT OR Apache-2.0`，任选其一。
