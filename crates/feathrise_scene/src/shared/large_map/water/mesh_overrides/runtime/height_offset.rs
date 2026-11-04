//! 将整网格水体作者高度从世界Y米制有界换算到primitive父坐标。

use std::collections::HashSet; // 分帧父链游标以O(1)实体集合拒绝循环。

use bevy::math::Mat3A; // 只反算位移向量所需的线性部分，父层级平移不会影响世界Y方向和米制长度。

use super::super::*; // 复用placement根、恢复记录、父链硬上限与Bevy层级类型。

/// 父链游标先逐层收集原始Transform，再逐项组合线性矩阵；两部分都消费同一明确操作预算。
pub(super) struct TransformChainCursor {
    root: Entity,                        // 完整层级必须经过当前placement根。
    current: Entity,                     // 收集阶段下一实体。
    visited: HashSet<Entity>,            // 固定深度上限内O(1)拒绝ChildOf循环。
    reversed_transforms: Vec<Transform>, // 从起点向世界根收集的原始位姿。
    crossed_placement_root: bool,        // 防止跨World实例使用另一根的坐标系。
    collecting: bool,                    // false后按世界根到起点顺序组合矩阵。
    compose_cursor: usize,               // 下一项待组合Transform。
    world_from_start: Mat3A,             // 已完成前缀的父坐标到世界线性映射。
} // 结束分帧父链游标。

impl TransformChainCursor {
    pub(super) fn new(root: Entity, start: Entity) -> Self {
        Self {
            root,
            current: start,
            visited: HashSet::with_capacity(MAX_MESH_WATER_ANCESTORS_TO_WORLD),
            reversed_transforms: Vec::with_capacity(MAX_MESH_WATER_ANCESTORS_TO_WORLD),
            crossed_placement_root: false,
            collecting: true,
            compose_cursor: 0,
            world_from_start: Mat3A::IDENTITY,
        }
    }

    /// 推进至多`budget`次父级读取或矩阵组合，完整完成时返回世界线性变换。
    pub(super) fn advance(
        &mut self,
        budget: usize,
        local_hierarchy: &Query<(&Transform, Option<&ChildOf>)>,
        overridden: &Query<&DynamicWaterMeshOverridePrimitive>,
    ) -> Result<Option<Mat3A>, String> {
        let mut remaining = budget; // 两个阶段共享同一帧明确上限。
        while remaining > 0 {
            remaining -= 1; // 一次父级读取或一次矩阵组合都计一个操作。
            if self.collecting {
                if self.visited.len() >= MAX_MESH_WATER_ANCESTORS_TO_WORLD {
                    return Err(format!(
                        "primitive父链超过{MAX_MESH_WATER_ANCESTORS_TO_WORLD}层硬上限"
                    ));
                }
                if !self.visited.insert(self.current) {
                    return Err(format!("primitive父链在实体{:?}形成循环", self.current));
                }
                let (current_transform, parent) = local_hierarchy
                    .get(self.current)
                    .map_err(|_| format!("primitive父链实体{:?}缺少Transform", self.current))?;
                let original_transform = if let Ok(previous) = overridden.get(self.current) {
                    if previous.owner_root != self.root {
                        return Err(format!(
                            "primitive父链实体{:?}由其他placement根{:?}接管",
                            self.current, previous.owner_root
                        ));
                    }
                    previous.original_transform // 热改时排除上轮世界高度偏移。
                } else {
                    *current_transform // 普通节点直接使用当前局部位姿。
                };
                self.reversed_transforms.push(original_transform); // Copy值跨帧稳定持有。
                if self.current == self.root {
                    self.crossed_placement_root = true; // placement自身及其外层变换都必须纳入世界米制。
                }
                if let Some(parent) = parent {
                    self.current = parent.parent(); // 下一预算操作继续向真实世界根追溯。
                } else {
                    if !self.crossed_placement_root {
                        return Err(format!(
                            "primitive父链没有经过当前placement根{:?}",
                            self.root
                        ));
                    }
                    self.reversed_transforms.reverse(); // 固定最多128项，转为父乘子顺序。
                    self.collecting = false; // 后续操作逐项组合，不在本次收集操作内隐藏完整循环。
                }
                continue;
            }
            if self.compose_cursor >= self.reversed_transforms.len() {
                return Ok(Some(self.world_from_start)); // 上一操作已完成最后一项组合。
            }
            let affine = self.reversed_transforms[self.compose_cursor].compute_affine();
            if !affine.is_finite() {
                return Err("primitive父链包含非有限Transform".to_owned());
            }
            self.world_from_start *= affine.matrix3; // 与Bevy GlobalTransform保持父乘子顺序。
            if !self.world_from_start.is_finite() {
                return Err("primitive父链组合后超出有限数值范围".to_owned());
            }
            self.compose_cursor += 1; // 精确保存已组合前缀。
        }
        if !self.collecting && self.compose_cursor >= self.reversed_transforms.len() {
            Ok(Some(self.world_from_start)) // 恰好在预算末完成时无需额外空转一帧。
        } else {
            Ok(None) // 游标完整保留到下一Update。
        }
    }
} // 结束分帧父链游标实现。

/// 把一组从世界根到primitive父级的局部变换组合成父级到世界的线性变换。
#[cfg(test)] // 生产路径由TransformChainCursor跨帧组合；纯数组版本只验证矩阵语义。
fn compose_world_from_parent_linear(
    transforms: &[Transform], // 调用方必须按最外层到最内层父级顺序提供完整局部链。
) -> Result<Mat3A, String> {
    if transforms.is_empty() {
        return Err("primitive父级没有可解析的世界变换链".to_owned()); // 空链无法证明作者米制高度属于哪个世界坐标系。
    } // 完成空父链失败门。
    let mut world_from_parent = Mat3A::IDENTITY; // 从世界单位基开始依次乘入placement和Blender层级。
    for transform in transforms {
        let affine = transform.compute_affine(); // 使用与Bevy Transform传播相同的局部TRS仿射表示。
        if !affine.is_finite() {
            return Err("primitive父链包含非有限Transform".to_owned()); // 非有限平移也说明当前场景层级无有效世界语义。
        } // 完成单层有限值检查。
        world_from_parent *= affine.matrix3; // 父乘子保持与GlobalTransform传播相同的组合顺序，但不读取GlobalTransform。
        if !world_from_parent.is_finite() {
            return Err("primitive父链组合后超出有限数值范围".to_owned()); // 极端缩放或旋转累积溢出时拒绝不确定结果。
        } // 完成累计线性变换有限值检查。
    } // 完成全部受限父级TRS组合。
    Ok(world_from_parent) // 返回只影响方向与长度的父坐标到世界坐标线性映射。
} // 结束父级到世界线性变换组合。

/// 把世界Y米制作者值反算成primitive局部Transform.translation应增加的父坐标位移。
#[cfg(test)] // 生产路径使用下方已完成矩阵版本；数组入口仅服务旋转缩放回归。
fn parent_translation_for_world_height(
    transforms: &[Transform], // 从最外层世界根到primitive直接父级的完整局部变换链。
    height_offset: f32,       // 正数上移、负数下移的世界Y轴米数。
) -> Result<Vec3, String> {
    if !height_offset.is_finite() {
        return Err("世界高度偏移不是有限数".to_owned()); // 即使绕过公共作者校验也不允许NaN进入Transform。
    } // 完成作者高度有限值检查。
    let world_from_parent = compose_world_from_parent_linear(transforms)?; // 先得到父坐标向量到世界向量的完整线性映射。
    let determinant = world_from_parent.determinant(); // 可逆性决定是否存在唯一的父坐标位移。
    if !determinant.is_finite() || determinant == 0.0 {
        return Err("primitive父链包含不可逆缩放".to_owned()); // 任一零缩放导致世界Y无法可靠反算时失败关闭。
    } // 完成组合线性变换可逆性检查。
    let parent_from_world = world_from_parent.inverse(); // 只求线性逆，不让无关的世界大坐标平移放大误差。
    if !parent_from_world.is_finite() {
        return Err("primitive父链逆变换超出有限数值范围".to_owned()); // 极端病态缩放不会写入巨大或NaN局部位移。
    } // 完成逆变换有限值检查。
    let world_offset = Vec3::Y * height_offset; // 作者高度始终沿真正的世界竖直轴解释。
    let local_offset = parent_from_world * world_offset; // 反算到primitive Transform.translation所在的父坐标空间。
    if !local_offset.is_finite() {
        return Err("世界高度偏移换算出非有限局部位移".to_owned()); // 数值异常时不进入材质切换与ECS应用阶段。
    } // 完成局部位移有限值检查。
    let reconstructed_world_offset = world_from_parent * local_offset; // 重新前向映射，证明旋转、缩放和剪切组合下仍保持世界米制。
    let tolerance = height_offset.abs().max(1.0) * 1.0e-4; // 允许单精度深层矩阵的有限舍入，但最多按目标距离万分之一放宽。
    if !reconstructed_world_offset.is_finite()
        || (reconstructed_world_offset - world_offset).length() > tolerance
    {
        return Err("primitive父链数值条件不足以稳定保持世界高度".to_owned()); // 病态但形式可逆的矩阵同样失败关闭。
    } // 完成世界Y米制往返稳定性检查。
    Ok(local_offset) // 应用期只需加到原primitive局部translation，不再执行可失败计算。
} // 结束世界高度到父坐标位移换算。

/// 已由增量父链游标验证的世界线性变换反算作者世界Y米制位移。
pub(super) fn parent_translation_for_world_height_linear(
    world_from_parent: Mat3A,
    height_offset: f32,
) -> Result<Vec3, String> {
    if !height_offset.is_finite() {
        return Err("世界高度偏移不是有限数".to_owned());
    }
    if !world_from_parent.is_finite() {
        return Err("primitive父链组合后超出有限数值范围".to_owned());
    }
    let determinant = world_from_parent.determinant();
    if !determinant.is_finite() || determinant == 0.0 {
        return Err("primitive父链包含不可逆缩放".to_owned());
    }
    let parent_from_world = world_from_parent.inverse();
    if !parent_from_world.is_finite() {
        return Err("primitive父链逆变换超出有限数值范围".to_owned());
    }
    let world_offset = Vec3::Y * height_offset;
    let local_offset = parent_from_world * world_offset;
    if !local_offset.is_finite() {
        return Err("世界高度偏移换算出非有限局部位移".to_owned());
    }
    let reconstructed_world_offset = world_from_parent * local_offset;
    let tolerance = height_offset.abs().max(1.0) * 1.0e-4;
    if !reconstructed_world_offset.is_finite()
        || (reconstructed_world_offset - world_offset).length() > tolerance
    {
        return Err("primitive父链数值条件不足以稳定保持世界高度".to_owned());
    }
    Ok(local_offset)
}

#[cfg(test)] // 世界Y米制换算使用纯Transform数组回归，不启动App、World或Transform传播系统。
mod tests {
    use super::*; // 直接验证本文件私有纯函数和与Bevy一致的矩阵组合。

    /// 验证局部位移应用后实际世界位移严格等于目标世界Y。
    fn assert_world_height_preserved(transforms: &[Transform], height_offset: f32) {
        let local_offset = parent_translation_for_world_height(transforms, height_offset)
            .expect("有限可逆父链必须成功换算世界高度"); // 调用被测纯函数获得primitive父坐标位移。
        let world_from_parent =
            compose_world_from_parent_linear(transforms).expect("同一有限父链必须可重复组合"); // 独立前向组合验证最终世界结果。
        let actual_world_offset = world_from_parent * local_offset; // 模拟primitive局部translation增加后的世界位移。
        let expected_world_offset = Vec3::Y * height_offset; // 作者契约只允许世界竖直方向发生变化。
        assert!(
            (actual_world_offset - expected_world_offset).length() <= 1.0e-4,
            "局部位移{local_offset:?}产生世界位移{actual_world_offset:?}，期望{expected_world_offset:?}"
        ); // 同时覆盖方向和米制长度，不能只比较Y分量。
    } // 结束世界高度保持断言helper。

    #[test] // 非均匀缩放不能把一米作者高度放大成父级scale倍数。
    fn non_uniform_scale_preserves_world_meter_height() {
        let transforms = [Transform::from_scale(Vec3::new(2.0, 4.0, 0.5))]; // 世界Y由父级局部Y放大四倍。
        let local_offset =
            parent_translation_for_world_height(&transforms, 3.0).expect("非零缩放父链必须可逆"); // 三米世界高度应反算为四分之三局部单位。
        assert!((local_offset - Vec3::new(0.0, 0.75, 0.0)).length() <= 1.0e-6); // 直接锁定米制缩放语义。
        assert_world_height_preserved(&transforms, 3.0); // 再以前向矩阵验证真实世界位移。
    } // 完成非均匀缩放回归。

    #[test] // placement和Blender父节点旋转后，局部增量可以有XYZ分量但世界结果仍只沿Y。
    fn rotated_scaled_hierarchy_preserves_world_vertical_direction() {
        let transforms = [
            Transform::from_translation(Vec3::new(120.0, -7.0, 48.0))
                .with_rotation(Quat::from_rotation_z(0.63))
                .with_scale(Vec3::new(1.7, 0.8, 2.2)), // 模拟placement根的平移、旋转与非均匀缩放。
            Transform::from_translation(Vec3::new(-3.0, 5.0, 9.0))
                .with_rotation(Quat::from_rotation_x(-0.41))
                .with_scale(Vec3::new(0.6, 1.9, 1.3)), // 模拟Blender对象祖先，组合后可产生剪切线性映射。
            Transform::from_rotation(Quat::from_rotation_y(0.27))
                .with_scale(Vec3::new(1.2, 0.7, 1.5)), // 模拟primitive直接父节点的最后一层局部TRS。
        ]; // 完成多层旋转缩放父链构造。
        let local_offset =
            parent_translation_for_world_height(&transforms, -2.4).expect("旋转缩放组合必须可逆"); // 负值验证世界向下偏移同样成立。
        assert!(local_offset.x.abs() > 0.01 || local_offset.z.abs() > 0.01); // 证明实现不是继续偷改局部translation.y。
        assert_world_height_preserved(&transforms, -2.4); // 前向验证最终没有世界横向漂移且长度为2.4米。
    } // 完成旋转缩放层级回归。

    #[test] // 零缩放父链无法唯一反算世界位移，必须显式失败而非写入NaN。
    fn singular_parent_hierarchy_fails_closed() {
        let transforms = [Transform::from_scale(Vec3::new(1.0, 0.0, 1.0))]; // 世界Y维度被完全压扁，线性矩阵不可逆。
        let result = parent_translation_for_world_height(&transforms, 1.0); // 尝试换算一米世界高度。
        assert!(result.is_err()); // 不可逆父链必须在应用任何ECS命令前失败关闭。
    } // 完成不可逆父链回归。
} // 结束世界Y米制纯函数测试。
