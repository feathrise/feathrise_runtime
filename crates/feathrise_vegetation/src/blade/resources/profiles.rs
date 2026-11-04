use bevy::prelude::{Handle, Image, Resource, Vec2, Vec4}; // 动态批次除四槽外观外还持有自己的综合色纹理页。
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::ShaderType;

use crate::types::{GPU_GRASS_PROFILE_CAPACITY, GpuBladeProfileSlot};

/// 一种GPU叶片植被的高度、宽度、形变、图集和综合色配置。
///
/// 【功能设计】所有字段都是内容级常量，数百万实例只在紧凑记录中保存两位profile槽号；
/// Draw按该槽号读取固定数组，禁止为每株叶片复制材质、颜色或风摆参数。
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct GpuBladeProfile {
    pub geometry: Vec4,          // xy为最小/最大株高，z为根部下扎，w为最大静态倾斜。
    pub width_flexibility: Vec4, // xy为最小/最大水平宽度倍率，zw为最硬/最软风摆响应。
    pub bending: Vec4,           // xy为交互/总草尖弯曲上限，zw为风与局部交互响应倍率。
    pub atlas_uv: Vec4,          // xy为综合色图集UV缩放，zw为UV偏移。
    pub root_color: Vec4,        // xyz为线性根部综合色，w保留给未来透光参数。
    pub tip_color: Vec4,         // xyz为线性尖端综合色，w保留给未来粗糙度参数。
    pub cool_tint: Vec4,         // xyz为单株冷色扰动倍率，w表示粗级原型是否按LOD格距扩大XZ覆盖。
    pub warm_tint: Vec4, // xyz为单株暖色扰动倍率，w表示LOD3是否复用只覆盖四乘四母格的远景网格。
    pub motion: Vec4,    // x为驱动类型（零风、一水流），yz为水流方向，w为水流动画速度。
    pub motion_params: Vec4, // x为水流草尖摆幅，y为世界波形空间倍率，zw保留。
}

impl GpuBladeProfile {
    /// 无外部纹理即可工作的中性绿色 profile。
    pub const DEFAULT: Self = Self {
        geometry: Vec4::new(0.50, 0.90, 0.03, 0.085), // 半米至九十厘米覆盖常见短叶植被示例。
        width_flexibility: Vec4::new(0.82, 1.48, 0.64, 1.0), // 保留自然宽窄与软硬变化。
        bending: Vec4::new(0.40, 0.48, 1.0, 1.0),     // 默认完整响应世界风与局部交互。
        atlas_uv: Vec4::new(1.0, 1.0, 0.0, 0.0),      // 默认采样消费者纹理的完整 UV 区域。
        root_color: Vec4::new(0.008, 0.018, 0.004, 0.0), // 白色回退纹理下形成深绿色根部。
        tip_color: Vec4::new(0.060, 0.160, 0.025, 0.0), // 白色回退纹理下形成可读亮绿色叶尖。
        cool_tint: Vec4::new(0.91, 1.01, 1.08, 0.0),  // 阴影侧提供轻微冷色个体变化。
        warm_tint: Vec4::new(1.12, 0.98, 0.80, 0.0),  // 日晒侧提供轻微暖色个体变化。
        motion: Vec4::ZERO, // 默认驱动类型零读取世界统一风场，保持全部旧内容行为。
        motion_params: Vec4::new(0.0, 1.0, 0.0, 0.0), // 风驱动不读取摆幅，空间倍率保持安全非零默认。
    };

    /// 使用完整 GPU 字段构造消费者自定义 profile。
    #[allow(clippy::too_many_arguments)] // 八个 vec4 与 WGSL 固定 uniform 字段一一对应，合并会隐藏 ABI。
    pub const fn new(
        geometry: Vec4,          // xy株高、z根部下扎、w最大静态倾斜。
        width_flexibility: Vec4, // xy宽度范围、zw柔韧度范围。
        bending: Vec4,           // xy交互与总弯曲上限、zw风和交互响应倍率。
        atlas_uv: Vec4,          // xy图集缩放、zw图集偏移。
        root_color: Vec4,        // 线性根部综合色及预留参数。
        tip_color: Vec4,         // 线性叶尖综合色及预留参数。
        cool_tint: Vec4,         // 冷色扰动及粗级XZ覆盖开关。
        warm_tint: Vec4,         // 暖色扰动及LOD3远模补偿开关。
    ) -> Self {
        Self {
            geometry,                                     // 保留消费者提供的株高和静态形变范围。
            width_flexibility,                            // 保留消费者提供的宽度和柔韧度范围。
            bending,                                      // 保留消费者提供的风与交互响应。
            atlas_uv,                                     // 保留消费者提供的共享图集切片。
            root_color,                                   // 保留消费者提供的线性根色。
            tip_color,                                    // 保留消费者提供的线性尖色。
            cool_tint,                                    // 保留消费者提供的冷色扰动和覆盖标志。
            warm_tint,                                    // 保留消费者提供的暖色扰动和LOD3标志。
            motion: Vec4::ZERO, // 既有八参数构造器继续明确创建世界风驱动内容。
            motion_params: Vec4::new(0.0, 1.0, 0.0, 0.0), // 预留水流参数保持确定安全值。
        } // 完成与 WGSL 固定 profile 布局一一对应的纯值构造。
    }

    /// 把当前外观改为开发者声明的水平水流驱动；玩家局部交互仍由公共足迹叠加。
    pub const fn with_water_flow(
        mut self,           // 按值接收便于项目内容常量链式定义。
        direction: Vec2,    // 世界 X/Z 流向；内容加载器负责拒绝零向量和非有限值。
        speed: f32,         // 动画时间倍率。
        tip_strength: f32,  // 草尖最大水流位移，单位为米。
        spatial_scale: f32, // 复用三层连续波时的世界空间频率倍率。
    ) -> Self {
        self.motion = Vec4::new(1.0, direction.x, direction.y, speed); // 一明确选择水流，yz/w 打包流向与速度。
        self.motion_params = Vec4::new(tip_strength, spatial_scale, 0.0, 0.0); // 其余槽保持确定零值供未来 ABI 扩展。
        self // 返回仍可作为 const 项目外观使用的完整 profile。
    }

    /// 把当前外观恢复为读取世界统一风场的驱动。
    pub const fn with_wind(mut self) -> Self {
        self.motion = Vec4::ZERO; // 驱动零让 WGSL 读取当前与上一帧全局风向、强度和速度。
        self.motion_params = Vec4::new(0.0, 1.0, 0.0, 0.0); // 清除旧水流摆幅并保留安全空间倍率。
        self // 颜色、几何和局部交互响应保持不变。
    }

    /// 返回当前 profile 是否使用独立水流而非世界风场。
    pub fn uses_water_flow(self) -> bool {
        self.motion.x >= 0.5 // GPU 使用同一阈值，避免 CPU 与 WGSL 驱动判定分叉。
    }
}

impl Default for GpuBladeProfile {
    fn default() -> Self {
        Self::DEFAULT // 四个公共槽默认共享中性安全外观，消费者可逐槽覆盖。
    }
}

/// 主世界与RenderApp共享的固定四槽叶片植被profile表。
#[derive(Resource, ExtractResource, Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct GpuBladeProfiles {
    pub(crate) profiles: [GpuBladeProfile; GPU_GRASS_PROFILE_CAPACITY], // 两位内容编号可直接安全索引。
}

impl Default for GpuBladeProfiles {
    fn default() -> Self {
        Self {
            profiles: [GpuBladeProfile::DEFAULT; GPU_GRASS_PROFILE_CAPACITY], // 四槽默认无地图语义并共享中性安全外观。
        }
    }
}

impl GpuBladeProfiles {
    /// 使用完整四槽数组构造 profile 表。
    pub const fn new(profiles: [GpuBladeProfile; GPU_GRASS_PROFILE_CAPACITY]) -> Self {
        Self { profiles } // 数组长度在编译期锁定为 GPU 两位槽容量。
    }

    /// 读取一个受检槽的当前 profile。
    pub const fn get(&self, slot: GpuBladeProfileSlot) -> GpuBladeProfile {
        self.profiles[slot.index() as usize] // 私有槽构造保证数组索引始终合法。
    }

    /// 覆盖一个受检槽，不触碰其他并发外观。
    pub fn set(&mut self, slot: GpuBladeProfileSlot, profile: GpuBladeProfile) {
        self.profiles[slot.index() as usize] = profile; // RenderApp 下一帧只覆写固定小 uniform。
    }

    /// 将全部槽恢复为无消费者内容语义的公共默认外观。
    pub fn reset(&mut self) {
        *self = Self::default(); // 不分配、不改变槽容量或实例 ABI。
    }
}

/// 一个运行时 Blade 绘制批次的四个局部外观与共享纹理页。
///
/// 【功能设计】四槽只是单个绘制绑定组内的紧凑 ABI，不再代表全项目内容上限；
/// 任意数量预设按纹理页和每批四项拆成多个本结构，共用二十字节实例记录。
#[derive(Clone, Debug, PartialEq)]
pub struct GpuBladeProfileBank {
    /// 当前批次的四个局部 profile；不足四项使用中性默认值填充。
    pub profiles: GpuBladeProfiles,
    /// 当前批次共享的综合色纹理页；None 明确使用白色回退。
    pub blade_base_color: Option<Handle<Image>>,
}

impl GpuBladeProfileBank {
    /// 使用一张纹理页和完整四槽表构造运行时批次。
    pub fn new(
        profiles: GpuBladeProfiles,              // 已由内容层确定局部槽顺序。
        blade_base_color: Option<Handle<Image>>, // 同页预设共享一个 GPU 纹理绑定。
    ) -> Self {
        Self {
            profiles,         // 保留四槽纯值表供 RenderApp 创建小 uniform。
            blade_base_color, // 强句柄保证活动目录期间纹理资产保持可达。
        }
    }
}

/// 主世界与 RenderApp 共享的动态 Blade 批次目录。
///
/// 下标零在渲染端保留给既有 `GpuBladeProfiles + GpuBladeRenderAssets`；本资源中的
/// 第零项对应公开批次 ID 一，后续项依次增长，因此旧场景无需迁移即可继续使用批次零。
#[derive(Resource, ExtractResource, Clone, Debug, Default, PartialEq)]
pub struct GpuBladeProfileBanks {
    banks: Vec<GpuBladeProfileBank>, // 数量由实际开发者预设派生，不设置产品硬上限。
    free: std::collections::BTreeSet<usize>, // 复用空槽而不移动其他根仍在使用的ID。
    references: Vec<usize>, // 室外草窗与三维继承根独立持有；最后一个根退役才回收槽位。
    epoch: std::sync::Arc<()>, // 整目录重置或地图驻留交换后，旧根不能归还另一来源的同号槽。
}

impl GpuBladeProfileBanks {
    /// 单一目录消费者的兼容入口；多根共存使用allocate，不能整体替换其他根的绘制槽。
    pub fn replace(&mut self, banks: Vec<GpuBladeProfileBank>) {
        self.references = vec![1; banks.len()];
        self.banks = banks; // Vec 只受实际内存和设备资源能力约束，不做静默截断。
        self.free.clear(); self.epoch = std::sync::Arc::new(());
    }

    /// 清除全部数据驱动批次；既有批次零仍由兼容资源继续提供。
    pub fn clear(&mut self) {
        self.banks.clear(); // 离开动态地图时释放目录持有的全部纹理强句柄。
        self.references.clear();
        self.free.clear(); self.epoch = std::sync::Arc::new(());
    }

    /// 只登记本次根的少量批次；其他可见/隐藏根的槽位及纹理强句柄保持不动。
    pub fn allocate(&mut self, banks: Vec<GpuBladeProfileBank>) -> Option<super::super::components::GpuBladeProfileBankAllocation> {
        use super::super::components::{GpuBladeProfileBankAllocation, GpuBladeProfileBankId};
        let end = self.banks.len().checked_add(banks.len().saturating_sub(self.free.len()))?;
        if end > u32::MAX as usize { return None; } // 分配前完整验证，失败不能留下半份目录。
        let mut ids = Vec::with_capacity(banks.len());
        for bank in banks {
            let index = self.free.pop_first().unwrap_or(self.banks.len());
            if index == self.banks.len() {
                self.banks.push(bank); self.references.push(1);
            } else { self.banks[index] = bank; self.references[index] = 1; }
            ids.push(GpuBladeProfileBankId::for_dynamic_index(index).expect("分配前已核对GPU下标范围"));
        }
        Some(GpuBladeProfileBankAllocation { epoch: self.epoch.clone(), ids: ids.into() })
    }

    /// 三维窗口继承室外模板的批次时显式续持，室外热更不能提前释放仍可见的洞壁外观。
    pub fn retain(&mut self, ids: Vec<super::super::components::GpuBladeProfileBankId>) -> Option<super::super::components::GpuBladeProfileBankAllocation> {
        for id in &ids {
            let index = id.index().checked_sub(1)? as usize;
            let count = *self.references.get(index)?;
            if count == 0 || count.checked_add(ids.len()).is_none() { return None; }
        } // 完整验证后才续持；无效模板不能留下部分信用。
        for id in &ids { self.references[id.index() as usize - 1] += 1; }
        Some(super::super::components::GpuBladeProfileBankAllocation { epoch: self.epoch.clone(), ids: ids.into() })
    }

    pub(crate) fn release(&mut self, epoch: &std::sync::Arc<()>, ids: &[super::super::components::GpuBladeProfileBankId]) {
        if !std::sync::Arc::ptr_eq(epoch, &self.epoch) { return; }
        for id in ids {
            let index = id.index() as usize - 1;
            if index >= self.references.len() || self.references[index] == 0 { continue; }
            self.references[index] -= 1;
            if self.references[index] == 0 && self.free.insert(index) {
                self.banks[index] = GpuBladeProfileBank::new(GpuBladeProfiles::default(), None);
            } // 释放本根纹理；内部空槽保留位置，不能重编号其他草窗。
        }
        while let Some(last) = self.banks.len().checked_sub(1) {
            if !self.free.remove(&last) { break; }
            self.banks.pop(); // 只收缩尾部连续空槽，长期热更不累积历史分配。
            self.references.pop();
        }
    }

    /// 返回稳定绘制槽目录，含内部可复用空槽；下标与各根的会话ID保持一致。
    pub fn banks(&self) -> &[GpuBladeProfileBank] {
        &self.banks // 渲染准备阶段只借用快照，不复制任意数量批次。
    }

    /// 返回动态目录槽位跨度，含内部空槽，不包含保留的兼容批次零。
    pub fn len(&self) -> usize {
        self.banks.len() // 诊断按真实批次数报告，不把四槽误写成内容上限。
    }

    /// 判断当前是否只有兼容批次零。
    pub fn is_empty(&self) -> bool {
        self.banks.is_empty() // 空动态目录仍是合法状态。
    }
}

#[cfg(test)]
#[path = "profile_bank_tests.rs"]
mod profile_bank_tests;
