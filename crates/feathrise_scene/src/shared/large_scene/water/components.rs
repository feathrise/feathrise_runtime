//! 正式大场景动态水面的ECS组件。

use bevy::prelude::*; // Component和Vec2用于保存GPU水面的轻量玩家接触状态。
use crate::shared::authoring_version::AuthoringVersion; // 覆盖总量不设门，保存只浅克隆版本票据。
use feathrise_scene_content::ScenePlacementWaterSurface; // 根组件直接持有经公共Serde格式校验的整网格赋水值。

use crate::scenes::dynamic_scene::{ClipmapGroundSnapshot, ClipmapWaterSurface}; // Clipmap静水沿用相同固定八槽互动状态。

use super::footprint::WaterInteractionFootprint; // GLB、R8静水、斜河与禁用瀑布共用一个受检互动足迹枚举。
use super::types::PlayerWaterRipple; // 组件保存固定容量的统一玩家波源。

/// 只有需要CPU玩家互动与波纹历史的水面才带此标记。
///
/// 【功能设计】Blender整网格水和瀑布第一阶段均为纯视觉水面，动画时间直接读取
/// RenderWorld全局uniform；不把它们放进随内容规模增长的逐帧CPU材质更新查询。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct DynamicWaterCpuInteraction; // ECS筛选在查询入口排除纯视觉水面，而不是逐项进入循环后再判断。

/// 安装在单个`WorldAssetRoot`上的Blender整网格水体覆盖。
///
/// 【功能设计】数组为空表示当前根不接管任何网格；结构变化必须整组件
/// `insert(DynamicWaterMeshOverrides::new(...))`，让生命周期observer定向唤醒该根；
/// 禁止通过普通`&mut`依赖稳定帧`Changed<T>`扫描。拖动数值使用下方一次性补丁组件。
#[derive(Component, Clone, Debug, Default)] // 完整值快照可直接进入编辑器撤销栈，整组件Insert同时形成明确事件边界。
pub struct DynamicWaterMeshOverrides {
    surfaces: Vec<ScenePlacementWaterSurface>, // 字段保持私有，避免调用方绕过明确的整批替换语义。
    authoring_version: AuthoringVersion, // 原位数值更新也必须撤销旧保存的回收资格。
} // 结束整网格水体覆盖根组件。

impl PartialEq for DynamicWaterMeshOverrides {
    fn eq(&self, other: &Self) -> bool {
        self.surfaces == other.surfaces // 原公开值相等语义不变，不把进程内票据混入作者内容比较。
    }
}

impl DynamicWaterMeshOverrides {
    /// 从一份已由调用方构造的作者快照建立根组件。
    pub fn new(surfaces: Vec<ScenePlacementWaterSurface>) -> Self {
        Self { surfaces, authoring_version: AuthoringVersion::default() } // 整批新作者快照拥有新票据。
    } // 完成根组件构造。

    /// 保存和回收只比较O(1)身份，不能因绑定数组增长而逐项复制或比较。
    pub fn authoring_version(&self) -> AuthoringVersion {
        self.authoring_version.clone()
    }

    /// 只读当前整批覆盖，供属性面板和诊断展示。
    pub fn surfaces(&self) -> &[ScenePlacementWaterSurface] {
        &self.surfaces // 不暴露可变Vec，调用方不会留下半项更新。
    } // 完成当前覆盖读取。

    /// 一次只修改冻结结构中的一个有限数值；失败时把旧值原样写回，保持组件快照原子。
    pub(super) fn apply_numeric_patch(
        &mut self,
        patch: DynamicWaterMeshOverrideNumericPatch,
    ) -> Result<(), String> {
        let surface_count = self.surfaces.len();
        let Some(surface) = self.surfaces.get_mut(patch.surface_index) else {
            return Err(format!(
                "整网格水数值补丁索引{}超出当前{}项覆盖",
                patch.surface_index, surface_count,
            ));
        };
        let previous = match patch.field {
            DynamicWaterMeshOverrideNumericField::FlowSpeed => {
                let previous = surface.flow_speed;
                surface.flow_speed = patch.value;
                previous
            }
            DynamicWaterMeshOverrideNumericField::HeightOffset => {
                let previous = surface.height_offset;
                surface.height_offset = patch.value;
                previous
            }
        };
        if let Err(detail) = surface.validate() {
            match patch.field {
                DynamicWaterMeshOverrideNumericField::FlowSpeed => surface.flow_speed = previous,
                DynamicWaterMeshOverrideNumericField::HeightOffset => {
                    surface.height_offset = previous;
                }
            }
            return Err(detail);
        }
        if previous != patch.value {
            self.authoring_version = AuthoringVersion::default();
        } // 失败和同值补丁保留旧版本；只有真正采用的新数值撤销旧保存的回收拥有权。
        Ok(())
    }
} // 结束整网格水体覆盖公开操作。

/// 拖动期间对稳定水面索引执行的一次性O(1)数值补丁。
///
/// 【功能设计】作者事务在开始拖动前冻结覆盖结构，因此`surface_index`只解释同一结构；
/// 调用方每档通过`commands.entity(root).insert(patch)`提交，运行时observer校验并修改一个
/// `f32`后立即移除本组件，再沿整组件Insert相同的Pending/原子重建链推进。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct DynamicWaterMeshOverrideNumericPatch {
    pub(crate) surface_index: usize, // 只允许定位已冻结数组的一项，绝不复制完整Vec。
    pub(crate) field: DynamicWaterMeshOverrideNumericField, // 明确字段避免两个Option形成非法空补丁。
    pub(crate) value: f32, // 运行时仍执行公共有限值与正式范围校验，不能信任UI钳制。
}

/// 一次性整网格水数值补丁的受控字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DynamicWaterMeshOverrideNumericField {
    FlowSpeed,    // 沿UV0.U的米每秒流速。
    HeightOffset, // 相对Blender原始高度的世界Y米制偏移。
}

impl DynamicWaterMeshOverrideNumericPatch {
    /// 构造一档流速拖动补丁。
    pub fn flow_speed(surface_index: usize, value: f32) -> Self {
        Self {
            surface_index,
            field: DynamicWaterMeshOverrideNumericField::FlowSpeed,
            value,
        }
    }

    /// 构造一档水面高度拖动补丁。
    pub fn height_offset(surface_index: usize, value: f32) -> Self {
        Self {
            surface_index,
            field: DynamicWaterMeshOverrideNumericField::HeightOffset,
            value,
        }
    }
}

/// 记录一次水中起跳从离地到重新落水或着底的完整周期。
#[derive(Clone, Copy, Default)] // 小型枚举随水面组件逐帧原地更新，不产生实体、资产或临时分配。
pub(super) enum WaterJumpCycle {
    #[default]
    Idle, // 当前没有等待结算的水中跳跃，普通入水和行走仍可生成各自波纹。
    Armed {
        left_surface: bool,     // 脚底是否已经穿出静止水位；再次入水时据此结算浅水落水波。
        was_falling_fast: bool, // 深水跳未穿出水面时，记录是否进入过明确下落段以识别着底。
    },
} // 结束水中跳跃周期状态定义。

/// 保存水面轮廓和玩家接触状态，逐顶点波形不再驻留或更新于CPU。
#[derive(Component)] // PostUpdate系统只查询已切换到GPU扩展材质的实际水面子实体。
pub(crate) struct DynamicWaterSurface {
    // 组件只保存玩家接触检测所需的静态轮廓与固定容量事件状态。
    pub(super) interaction_footprint: WaterInteractionFootprint, // 统一足迹返回水平静水或斜河实际表面，瀑布明确禁用。
    pub(super) rest_water_level: f32, // 保存静止水位，用玩家脚底深度判断是否正在接触这片无碰撞水域。
    pub(super) flow_direction: Vec2, // 湖面使用作者XZ流向；整网格水用x高位哨兵强制UV0流场，样条仍读UV1。
    pub(super) flow_speed: f32,      // 湖面基础流速与样条回退流速共同进入当前/历史GPU参数。
    pub(super) player_ripples: Vec<PlayerWaterRipple>, // 固定容量运行时池保存最近八个入水、走动、起跳或落水大波。
    pub(super) last_player_takeoff_sequence: Option<u64>, // 每张水面独立消费Tnua实际起跳序号，None表示玩家首次出现只建立基线。
    pub(super) previous_player_local_xz: Option<Vec2>, // 记录上一帧玩家局部位置，按真实移动距离发射新波圈。
    pub(super) previous_player_local_y: Option<f32>, // 记录上一帧脚底高度，只用于确认深水跳的下降与着底阶段。
    pub(super) distance_since_ripple: f32, // 累积上一次发波后的水中行走距离，避免依赖动画状态或按键。
    pub(super) player_was_in_water: bool,  // 记录接触状态边沿，使首次踏入水面只发射一圈入水波。
    pub(super) jump_cycle: WaterJumpCycle, // 起跳立即发强波并武装周期，落水或深水着底再追加同种强波。
} // 结束正式场景动态水面运行时数据定义。

impl DynamicWaterSurface {
    /// 洞内派生水面复用同源流向参数，但独立拥有固定八槽玩家接触历史。
    pub(crate) fn derived_for_surface(
        &self,
        surface: &ClipmapWaterSurface,
        ground: &ClipmapGroundSnapshot,
    ) -> Self {
        Self::from_clipmap(surface, ground, self.flow_direction, self.flow_speed)
    }
    /// Clipmap用世界坐标作为波源坐标；足迹同时冻结同代R8与已采用分页高度。
    pub(crate) fn from_clipmap(
        surface: &ClipmapWaterSurface,
        ground: &ClipmapGroundSnapshot,
        flow_direction: Vec2,
        flow_speed: f32,
    ) -> Self {
        Self {
            interaction_footprint: WaterInteractionFootprint::Clipmap {
                surface: surface.clone(),
                ground: ground.clone(),
            },
            rest_water_level: surface.water_level,
            flow_direction,
            flow_speed,
            player_ripples: Vec::with_capacity(super::constants::PLAYER_RIPPLE_CAPACITY),
            last_player_takeoff_sequence: None,
            previous_player_local_xz: None,
            previous_player_local_y: None,
            distance_since_ripple: 0.0,
            player_was_in_water: false,
            jump_cycle: WaterJumpCycle::Idle,
        }
    }

    /// 高度页或视图修订换代时只替换只读足迹绑定，既有水中接触和波源历史保持连续。
    pub(crate) fn sync_clipmap_footprint(
        &mut self,
        surface: &ClipmapWaterSurface,
        ground: &ClipmapGroundSnapshot,
    ) {
        self.rest_water_level = surface.water_level;
        self.interaction_footprint = WaterInteractionFootprint::Clipmap {
            surface: surface.clone(),
            ground: ground.clone(),
        };
    }
}

#[cfg(test)]
mod tests {
    use feathrise_scene_content::{ScenePlacementWaterKind, ScenePlacementWaterSurface};

    use super::*;

    fn surface() -> ScenePlacementWaterSurface {
        ScenePlacementWaterSurface {
            mesh_index: 3,
            node_path: vec!["河道".to_owned(), "水面".to_owned()],
            mesh_name: "河流水面".to_owned(),
            kind: ScenePlacementWaterKind::Sloped,
            flow_speed: 1.0,
            height_offset: 2.0,
        }
    }

    #[test]
    fn numeric_patch_changes_only_one_frozen_surface_field() {
        let mut overrides = DynamicWaterMeshOverrides::new(vec![surface(), surface()]);
        overrides
            .apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::flow_speed(1, 3.5))
            .unwrap();
        assert_eq!(overrides.surfaces()[0].flow_speed, 1.0);
        assert_eq!(overrides.surfaces()[1].flow_speed, 3.5);
        assert_eq!(overrides.surfaces()[1].height_offset, 2.0);
    }

    #[test]
    fn invalid_numeric_patch_restores_the_previous_author_value() {
        let mut overrides = DynamicWaterMeshOverrides::new(vec![surface()]);
        assert!(
            overrides
                .apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::height_offset(
                    0,
                    f32::INFINITY,
                ))
                .is_err()
        );
        assert_eq!(overrides.surfaces()[0].height_offset, 2.0);
        assert!(
            overrides
                .apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::flow_speed(9, 2.0))
                .is_err()
        );
    }

    #[test]
    fn authoring_version_tracks_real_patches_without_changing_value_equality() {
        let mut overrides = DynamicWaterMeshOverrides::new(vec![surface()]);
        let initial = overrides.authoring_version();
        let cloned = overrides.clone();
        let rebuilt = DynamicWaterMeshOverrides::new(vec![surface()]);
        assert_eq!(overrides, rebuilt, "内容相同仍保留原PartialEq语义");
        assert_eq!(initial, cloned.authoring_version());
        assert_ne!(initial, rebuilt.authoring_version());

        overrides.apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::flow_speed(0, 1.0)).unwrap();
        assert_eq!(initial, overrides.authoring_version(), "同值补丁不是新作者版本");
        assert!(overrides.apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::flow_speed(8, 2.0)).is_err());
        assert_eq!(initial, overrides.authoring_version());
        assert!(overrides.apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::height_offset(0, f32::INFINITY)).is_err());
        assert_eq!(initial, overrides.authoring_version(), "失败回滚不得撤销仍有效的保存证明");

        overrides.apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::flow_speed(0, 3.5)).unwrap();
        let changed = overrides.authoring_version();
        assert_ne!(initial, changed);
        assert_eq!(cloned.surfaces()[0].flow_speed, 1.0, "原克隆票据和内容仍代表保存时的旧版本");
        overrides.apply_numeric_patch(DynamicWaterMeshOverrideNumericPatch::height_offset(0, 4.0)).unwrap();
        assert_ne!(changed, overrides.authoring_version(), "第二类真实标量更新也推进版本");
        assert_eq!(overrides.authoring_version(), overrides.clone().authoring_version());
    }
}

/// 仅参与水面渲染和雨水高度捕获的子块，不创建第二个CPU互动波源。
#[derive(Component)]
pub(crate) struct WaterRenderChunk;
