//! F3总览分页的通用资源报告格式化。
//! 本文件只接收冻结后的世界快照，不持有ECS查询或任何草、麦、树木功能域数据。

use super::queries::ResourceMonitorSnapshot; // 读取同一半秒周期冻结的通用世界资源计数。

/// 格式化总览页，其他分页不会调用本函数或生成该字符串。
pub(super) fn format_overview_report(
    fps: f64,                           // Bevy平滑FPS诊断值。
    frame_ms: f64,                      // Bevy平滑帧耗时诊断值。
    application: &str, // 当前应用已经采样的附加文字；空工程不包含RPG行。
    snapshot: &ResourceMonitorSnapshot, // 当前低频刷新周期的通用资源快照。
) -> String {
    format!(
        "总览\n\
         FPS {fps:>5.1} | 帧耗时 {frame_ms:>5.2} ms\n\
         实体 {entities} | 命名 {named}\n\
         {application}Billboard {billboards} T:{billboard_temporary} P:{billboard_persistent} | 朝向相机 {billboard_cameras}\n\
         渲染 Mesh3d {mesh_instances} | SceneRoot {scene_roots}\n\
         相机 {cameras} | 光源 {light_count}\n\
         UI Node {ui_nodes} | Text {ui_texts}\n\
         物理 Body {rigid_bodies} | Collider {colliders}\n\
         资产 Mesh {asset_meshes} | Material {asset_materials}\n\
         资产 Image {asset_images} | AnimClip {asset_animations}",
        entities = snapshot.entities,                 // 世界存活实体数。
        named = snapshot.named,                       // 带Name组件实体数。
        billboards = snapshot.billboards,             // Billboard总数。
        billboard_temporary = snapshot.billboard_temporary, // 临时Billboard数。
        billboard_persistent = snapshot.billboard_persistent, // 常驻Billboard数。
        billboard_cameras = snapshot.billboard_cameras, // Billboard朝向相机数。
        mesh_instances = snapshot.mesh_instances,     // Mesh3d实例数。
        scene_roots = snapshot.scene_roots,           // SceneRoot数。
        cameras = snapshot.cameras,                   // 相机总数。
        light_count = snapshot.light_count,           // 点光、方向光与聚光总数。
        ui_nodes = snapshot.ui_nodes,                 // UINode数。
        ui_texts = snapshot.ui_texts,                 // UIText数。
        rigid_bodies = snapshot.rigid_bodies,         // 物理刚体数。
        colliders = snapshot.colliders,               // 物理碰撞体数。
        asset_meshes = snapshot.asset_meshes,         // 驻留Mesh资产数。
        asset_materials = snapshot.asset_materials,   // 驻留标准材质资产数。
        asset_images = snapshot.asset_images,         // 驻留图像资产数。
        asset_animations = snapshot.asset_animations, // 驻留动画片段资产数。
    ) // 返回总览页最终字符串。
} // 完成总览报告格式化。

#[cfg(test)] // 纯格式化测试无需ECS世界。
mod tests {
    use super::*; // 复用生产格式器和快照类型。

    #[test] // 验证总览只含通用资源且保持关键字段。
    fn overview_keeps_general_world_and_asset_counts() {
        let snapshot = ResourceMonitorSnapshot {
            entities: 42,         // 使用显眼数值验证实体统计。
            asset_meshes: 7,      // 使用显眼数值验证资产统计。
            ..Default::default()  // 未关心字段保持零。
        }; // 完成通用资源测试快照。
        let report = format_overview_report(60.0, 16.67, "", &snapshot); // 生成与运行时同口径文本。
        assert!(report.starts_with("总览\n")); // 当前页必须有明确页名。
        assert!(report.contains("实体 42")); // 实体统计必须保留。
        assert!(report.contains("资产 Mesh 7")); // 资产统计必须保留。
        assert!(!report.contains("草验收")); // 总览不得重新混入植被长报告。
        assert!(!report.contains("树LOD")); // 总览不得重新混入树木技术报告。
    } // 完成总览字段归属验证。
} // 结束总览格式化测试。
