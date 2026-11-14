use super::*;

impl GpuBladePatch {
    /// 三维作者格独立于窗口XZ格；GPU依据源样本的真实XYZ与法线生成实例。
    pub fn with_volume_source(mut self, count: u32) -> Self {
        self.height_decode.z = 0.;
        if !self.uses_volume_source() && self.grid.w & DENSITY_FIELD_ENABLED_FLAG != 0 {
            // 三维路径不读取高度单元图；借用其矩形保存室外R8映射，供连续顶面的跨界父格取样。
            // density_world另存候选规格，不能再把它误当成原密度世界范围。
            self.height_world = self.density_world;
        }
        self.grid.x = (count as f64).sqrt().ceil().max(1.) as u32;
        self.grid.y = count;
        self.grid.w |= 1 << 30;
        self.density_world = Vec4::new(1.0, 0.0, 0.0, 0.0); // 三维候选规格；跨界R8范围保存在height_world。
        self
    }
    /// GPU沿当前LOD的世界母格采样；粗级只派发父格，避免为宽草簇仍生成全部近叶数量。
    pub fn with_volume_cell_source(self, count: u32, cell_size: f32) -> Self {
        let side = (1u32 << self.lod_level().min(7)) as f32;
        let spacing = if self.view_lod_metadata.w > 0.0 {
            self.view_lod_metadata.w
        } else {
            self.grid_spacing() / side
        };
        let divisions = if cell_size.is_finite() {
            ((cell_size / (spacing.max(0.01) * side)).ceil() + 2.0).clamp(3.0, 64.0) as u32
        } else {
            1
        };
        let mut patch = self.with_volume_source(count.saturating_mul(divisions * divisions));
        patch.density_world.x = divisions as f32;
        patch.density_world.y = spacing.max(0.01);
        patch
    }
    /// 模型候选已在后台沿原三角面细分，GPU保留精确根点；不能再次投到轴向格或借邻楼层取密度。
    pub fn with_surface_sample_source(self, count: u32, finest_spacing: f32) -> Self {
        let mut patch = self.with_volume_source(count);
        patch.grid.w |= 1 << 29;
        patch.density_world.y = finest_spacing.max(0.01);
        patch
    }
    pub(crate) fn uses_volume_source(&self) -> bool {
        self.grid.w & (1 << 30) != 0
    }
    pub fn focus_radius_xyz(&self) -> Vec3 {
        self.focus_radius.truncate()
    }
    pub fn with_volume_seed(mut self, seed: u32) -> Self {
        self.grid.z = seed;
        self
    }
    /// 构建第一阶段固定网格草块；CPU 只保存草块级参数，不生成逐草位置。
    pub fn square(origin: Vec3, extent: f32, grid_side: u32, seed: u32) -> Self {
        let safe_origin = if origin.is_finite() {
            origin
        } else {
            Vec3::ZERO
        }; // 非有限世界坐标统一回退原点，禁止NaN或Inf进入全部后续GPU距离计算。
        let safe_grid_side = grid_side.max(1); // 避免零边长导致着色器除零。
        let safe_extent = if extent.is_finite() {
            extent.max(0.01) // 有限边长仍保留原有最小一厘米保护。
        } else {
            0.01 // 非有限边长回退最小合法窗口，避免候选格距和AABB继续传播异常值。
        }; // 完成草窗边长输入净化。
        let default_visible_radius = safe_extent * 0.5; // 静态草块默认允许显示到正方形窗口内切圆边缘。
        let default_outer_fade = (default_visible_radius * 0.15).clamp(0.01, 40.0); // 保持旧单圈窗口百分之十五且最多四十米的远端淡出。
        Self {
            source_to_world: Mat4::IDENTITY,
            world_to_source: Mat4::IDENTITY,
            origin_extent: safe_origin.extend(safe_extent), // 保证草块世界中心和边长全部为有限值。
            grid: UVec4::new(
                safe_grid_side, // 着色器用该值把实例编号映射为二维网格。
                safe_grid_side.saturating_mul(safe_grid_side), // 直接作为一次 draw 的实例范围。
                seed,           // 同一草块保持稳定外观，不随帧变化。
                0,              // 默认不启用密度或后续LOD等绘制标志。
            ),
            focus_radius: safe_origin.extend(default_visible_radius), // 静态草块默认以净化后的自身中心和半边长作为可见范围。
            height_world: Vec4::ZERO, // 默认没有地表高度纹理，最小样例继续使用origin.y固定平面。
            height_decode: Vec4::new(safe_origin.y, 0.0, 0.0, 1.0), // 无图时关闭高度采样，但默认完整显示以兼容既有场景与样例。
            density_world: Vec4::ZERO, // 默认不读取密度图，最小样例继续保留全部候选。
            lod_density: Vec4::new(0.0, 0.01, default_outer_fade, 1.0), // 默认无内圈空洞、沿用旧外圈淡出且不放大密度。
            vegetation_bounds: Vec4::new(
                GpuBladeConfig::DEFAULT.max_blade_height, // 旧调用方继续使用零点九米普通草保守高度。
                GpuBladeConfig::DEFAULT.max_horizontal_sweep, // 旧调用方继续覆盖原有最大风摆和草簇宽度。
                GpuBladeConfig::DEFAULT.reference_height, // 旧调用方屏幕LOD继续使用七十厘米参考草高。
                GpuBladeConfig::DEFAULT.root_penetration, // 旧调用方根部继续下扎三厘米。
            ), // 完成默认普通草内容和GPU包围参数初始化。
            density_control_world: Vec4::ZERO, // 旧场景默认只按密度图全图范围裁剪，不被新增受控区改变。
            view_lod_pixel_thresholds: Vec4::ZERO, // 默认继承GpuBladeView，保持所有既有场景和最小样例行为。
            view_lod_ultra_fade: Vec4::ZERO, // 未启用内容级配置时不单独覆盖超远阈值或近镜头避让。
            view_lod_residency_radii: Vec4::ZERO, // 未配置局部规格时前四档半径均为零，档数元数据决定是否继承。
            view_lod_residency_transitions: Vec4::ZERO, // 未启用时全部过渡宽度和安全边距由玩法视图提供。
            view_lod_pixel_thresholds_ext: [Vec4::ZERO; 2],
            view_lod_residency_radii_ext: Vec4::ZERO,
            view_lod_residency_transitions_ext: Vec4::ZERO,
            view_lod_metadata: Vec4::ZERO, // 零档数保留旧三/四档调用方的显式标志语义。
        } // 完成轻量草块参数构建。
    }

    /// 设置逐株裁剪的完全消失半径；半径必须留在正方形窗口内部。
    pub fn with_visible_radius(mut self, radius: f32) -> Self {
        if !radius.is_finite() {
            return self; // 非有限半径保持构造器的安全默认值，禁止破坏环带和淡出参数。
        } // 完成可见半径输入保护。
        self.focus_radius.w = radius.clamp(0.01, self.origin_extent.w * 0.5); // 防止可见圆越过窗口边缘暴露方形边界。
        self.lod_density.x = 0.0; // 单圈兼容接口明确清除内圈空洞。
        self.lod_density.z = (self.focus_radius.w * 0.15).clamp(0.01, 40.0); // 继续使用旧版自动远端淡出宽度。
        self // 返回可继续链式配置的轻量窗口。
    }

    /// 设置一个带内外交叉过渡的圆形LOD环带。
    pub fn with_lod_range(
        mut self,              // 按值接收窗口，支持场景层连续链式配置。
        inner_radius: f32,     // 该半径以内完全交给更密一级LOD。
        outer_radius: f32,     // 该半径以外完全交给更疏一级LOD或地表材质。
        inner_fade_width: f32, // 从内半径开始逐株增加的交叉过渡宽度。
        outer_fade_width: f32, // 到外半径结束逐株减少的交叉过渡宽度。
    ) -> Self {
        self.set_lod_range(
            // 复用可变接口的窗口边界和非零过渡校验。
            inner_radius,     // 传递调用方要求的内圈空洞半径。
            outer_radius,     // 传递调用方要求的外圈完全消失半径。
            inner_fade_width, // 传递与更密一级草圈的交叉宽度。
            outer_fade_width, // 传递与更疏一级草圈的交叉宽度。
        ); // 完成LOD环带参数写入。
        self // 返回可继续配置高度、密度和场景资产的窗口。
    }

    /// 更新当前窗口的LOD环带，所有距离仍以连续观察中心为圆心。
    pub fn set_lod_range(
        &mut self,             // 只更新当前草块的两个vec4距离参数。
        inner_radius: f32,     // 内圈完全不可见半径，零表示从玩家脚边开始显示。
        outer_radius: f32,     // 外圈完全不可见半径，必须位于候选正方形内。
        inner_fade_width: f32, // 内圈逐株淡入宽度。
        outer_fade_width: f32, // 外圈逐株淡出宽度。
    ) {
        if !inner_radius.is_finite()
            || !outer_radius.is_finite()
            || !inner_fade_width.is_finite()
            || !outer_fade_width.is_finite()
        {
            return; // 任一距离异常时完整保留旧环带，避免只更新部分字段形成不一致smoothstep。
        } // 完成四项LOD输入的原子有限性校验。
        let safe_outer_radius = outer_radius.clamp(0.01, self.origin_extent.w * 0.5); // 防止环带越过候选窗口的内切圆。
        let safe_inner_radius = inner_radius
            .max(0.0)
            .min((safe_outer_radius - 0.01).max(0.0)); // 保证内半径严格小于外半径。
        let available_ring_width = (safe_outer_radius - safe_inner_radius).max(0.01); // 给两个smoothstep保留合法正宽度。
        self.focus_radius.w = safe_outer_radius; // 既有字段继续保存当前草圈完全消失半径。
        self.lod_density.x = safe_inner_radius; // 顶点shader在该半径以内严格剔除本圈候选。
        self.lod_density.y = inner_fade_width.clamp(0.01, available_ring_width); // 内圈交叉不得超过整个环带宽度。
        self.lod_density.z = outer_fade_width.clamp(0.01, available_ring_width); // 外圈交叉同样保持有效smoothstep范围。
    }

    /// 为当前草圈选择近、中、远材质级别；级别随紧凑实例进入GPU，不增加逐草CPU数据。
    pub fn with_lod_level(mut self, lod_level: u32) -> Self {
        self.set_lod_level(lod_level); // 复用可变接口的两位标志保护和合法范围钳制。
        self // 返回可继续链式配置高度、密度和场景输入的草块。
    }

    /// 更新当前草圈的材质LOD级别，同时严格保留`grid.w`中的密度与未来标志位。
    pub fn set_lod_level(&mut self, lod_level: u32) {
        let safe_lod_level = lod_level.min(MAX_LOD_LEVEL); // 未定义的更高级别统一钳到超远档，防止着色器身份越界。
        self.grid.w = (self.grid.w & !LOD_LEVEL_MASK) // 只清除原有两个LOD位。
            | (safe_lod_level << LOD_LEVEL_SHIFT); // 把合法零至三级写回原位而不覆盖密度启用位。
    }

    /// 返回当前草圈进入GPU紧凑实例的近中远材质级别。
    pub fn lod_level(self) -> u32 {
        (self.grid.w & LOD_LEVEL_MASK) >> LOD_LEVEL_SHIFT // 解码后的零至三分别对应近、中、远、超远。
    }

    /// 声明当前窗口属于近中远同源父簇层级。
    ///
    /// 【功能设计】启用后，Compute使用统一最细世界格生成父簇身份、占用与有界根位置；
    /// 当屏幕空间视图暂时无效时仅绘制最远祖先级，避免三个零内圈窗口同时叠加。
    pub fn with_hierarchical_lod(mut self, enabled: bool) -> Self {
        self.set_hierarchical_lod(enabled); // 复用可变接口并严格保留密度、LOD和驻留输出标志。
        self
    }

    /// 更新同源父簇层级标志，不改变任何候选数量或窗口半径。
    pub fn set_hierarchical_lod(&mut self, enabled: bool) {
        if enabled {
            self.grid.w |= HIERARCHICAL_LOD_FLAG; // 第四个持久位只改变同源LOD身份与安全回退策略。
        } else {
            self.grid.w &= !HIERARCHICAL_LOD_FLAG; // 单窗口与旧径向环继续使用各自独立世界格。
        }
    }

    /// 返回当前窗口是否属于正式同源父簇层级。
    pub fn uses_hierarchical_lod(self) -> bool {
        self.grid.w & HIERARCHICAL_LOD_FLAG != 0
    }
    /// 为当前同源层级启用第四级超远代理，并编码世界稳定固定预算档。
    pub fn with_ultra_lod_budget(mut self, budget: GpuBladeUltraLodBudget) -> Self {
        self.grid.w |= ULTRA_LOD_CHAIN_FLAG; // 普通草四窗必须共同进入八乘八顶层父簇选择。
        let encoded = match budget {
            GpuBladeUltraLodBudget::High => 0,   // 二乘二超远父格块编码为零。
            GpuBladeUltraLodBudget::Dense => 1,  // 三乘三增密块编码为一。
            GpuBladeUltraLodBudget::Medium => 2, // 四乘四平衡块编码为二。
            GpuBladeUltraLodBudget::Low => 3,    // 八乘八低档块编码为三。
        }; // 完成不依赖浮点或逐帧状态的固定档位编码。
        self.grid.w = (self.grid.w & !ULTRA_LOD_BUDGET_MASK) | (encoded << ULTRA_LOD_BUDGET_SHIFT); // 只覆盖超远预算位，保留密度、LOD、层级和内容profile。
        self // 返回可继续配置地表数值输入的轻量窗口。
    }

    /// 返回当前窗口是否属于含第四级超远代理的同源层级。
    pub fn uses_ultra_lod_chain(self) -> bool {
        self.grid.w & ULTRA_LOD_CHAIN_FLAG != 0 && self.lod_count() >= 4
    }

    /// 仅最终粗档使用超远稀疏预算，中间扩展档必须保持完整候选容量。
    pub(crate) fn uses_ultra_lod_budget(self) -> bool {
        self.uses_ultra_lod_chain() && self.lod_level() + 1 == self.lod_count()
    }

    /// 显式局部档数优先；未写入局部规格的旧场景按原三/四档标志兼容。
    pub fn lod_count(self) -> u32 {
        if self.view_lod_metadata.x > 0.0 {
            self.view_lod_metadata.x as u32
        } else if self.grid.w & ULTRA_LOD_CHAIN_FLAG != 0 {
            4
        } else {
            3
        }
    }

    /// 返回超远世界稳定预算块的单边父格数。
    pub fn ultra_lod_budget_block_side(self) -> u32 {
        if !self.uses_ultra_lod_chain() {
            return 1; // 非超远窗口不执行结构降采样，容量保持既有完整圆形上界。
        } // 完成三级兼容路径。
        match (self.grid.w & ULTRA_LOD_BUDGET_MASK) >> ULTRA_LOD_BUDGET_SHIFT {
            0 => 2, // 高档每四个超远父格最多保留一个代理。
            1 => 3, // 增密档每九个超远父格最多保留一个代理。
            2 => 4, // 中档每十六个超远父格最多保留一个代理。
            _ => 8, // 低档每六十四个父格最多保留一个代理。
        } // 完成与WGSL完全一致的预算块解码。
    }

    /// 为当前窗口设置叶片植被内容身份以及GPU保守包围参数。
    ///
    /// 【功能设计】profile编号继续打包进既有实例最高字节；米制高度与扫掠半径只存在于窗口uniform，
    /// 不会随候选或最终实绘实例数量增加上传成本。
    pub fn with_vegetation_config(mut self, config: GpuBladeConfig) -> Self {
        self.set_vegetation_config(config); // 复用可变接口的有限值和保守下界保护。
        self // 返回可继续链式配置高度、密度和LOD窗口的轻量参数。
    }

    /// 更新当前窗口所属内容及其保守包围，不改变候选数量、密度图或LOD编号。
    pub fn set_vegetation_config(&mut self, config: GpuBladeConfig) {
        let safe_max_height = if config.max_blade_height.is_finite() {
            config.max_blade_height.clamp(0.05, 8.0) // 叶片类内容保持五厘米至八码的有限保守高度。
        } else {
            GpuBladeConfig::DEFAULT.max_blade_height // 非有限高度回退已验证的普通草上界。
        }; // 完成最高叶尖输入净化。
        let safe_horizontal_sweep = if config.max_horizontal_sweep.is_finite() {
            config.max_horizontal_sweep.clamp(0.01, 4.0) // 水平包围至少一厘米且不允许错误配置扩成整块地。
        } else {
            GpuBladeConfig::DEFAULT.max_horizontal_sweep // 非有限扫掠回退普通草安全值。
        }; // 完成水平包围输入净化。
        let safe_reference_height = if config.reference_height.is_finite() {
            config.reference_height.clamp(0.01, safe_max_height) // 典型投影高度不能超过当前内容保守最高叶尖。
        } else {
            safe_max_height // 非有限参考值保守使用最高叶尖，避免过早切换粗LOD。
        }; // 完成屏幕LOD参考高度输入净化。
        let safe_root_penetration = if config.root_penetration.is_finite() {
            config.root_penetration.clamp(0.0, 0.25) // 叶片根部最多下扎二十五厘米，防止地表下大范围误剔除。
        } else {
            GpuBladeConfig::DEFAULT.root_penetration // 非有限根深回退三厘米。
        }; // 完成根部下扎输入净化。
        self.grid.w = (self.grid.w & !VEGETATION_PROFILE_MASK) // 只清除旧两位内容编号，保留密度、LOD、层级和驻留标志。
            | (config.profile_slot.index() << VEGETATION_PROFILE_SHIFT); // 写入零至三稳定profile编号。
        self.vegetation_bounds = Vec4::new(
            safe_max_height,       // Compute视锥与近镜头避让读取最高叶尖。
            safe_horizontal_sweep, // Compute近镜头完整包围读取最大水平扫掠。
            safe_reference_height, // Compute屏幕LOD按当前内容典型高度估算像素尺寸。
            safe_root_penetration, // Compute包围和Draw根部位置共享同一米制下扎深度。
        ); // 完成不会扩大逐实例ABI的内容规格写入。
    }

    /// 返回当前窗口的稳定受检profile槽。
    pub fn profile_slot(self) -> GpuBladeProfileSlot {
        GpuBladeProfileSlot::from_gpu_index(
            (self.grid.w & VEGETATION_PROFILE_MASK) >> VEGETATION_PROFILE_SHIFT,
        ) // 未知或损坏编号由类型构造器安全回退零号槽。
    }

    /// 返回当前窗口参与GPU保守剔除的最高叶尖米数。
    pub fn max_blade_height(self) -> f32 {
        self.vegetation_bounds.x // F3和场景测试无需访问uniform私有字段即可核对内容包围。
    }

    /// 返回当前窗口参与GPU视锥和近镜头剔除的保守水平扫掠半径。
    pub fn max_horizontal_sweep(self) -> f32 {
        self.vegetation_bounds.y // 消费者回归可核对远景原型覆盖已经进入保守包围。
    }

    /// 返回当前窗口用于屏幕空间LOD估算的典型株高。
    pub fn reference_blade_height(self) -> f32 {
        self.vegetation_bounds.z // 不同profile可保持独立细节距离而无需修改全局相机。
    }

    /// 为当前内容设置独立的屏幕LOD、近镜头避让和常驻边界规格。
    ///
    /// 【功能设计】同一视图可同时驻留不同高度与档数的叶片profile；配置使用固定大小的草窗uniform，
    /// 不复制逐草数据，也不会改写共享玩法相机从而影响其他内容。
    pub fn with_view_lod_config(mut self, config: GpuBladeViewLodConfig) -> Self {
        self.set_view_lod_config(config); // 复用可变接口的一致合法性保护和uniform打包。
        self // 返回可继续链式配置高度、密度和受控区域的轻量窗口。
    }

    /// 更新当前窗口的内容级LOD规格；非法配置保持上一份合法值不变。
    ///
    /// 全部有效档一同打包；非法候选不得留下新档数与旧阈值混用的半配置。
    pub fn set_view_lod_config(&mut self, config: GpuBladeViewLodConfig) {
        if !config.is_valid() {
            return; // 损坏场景规格保持上一份完整局部配置。
        }
        let thresholds = config.pixel_thresholds;
        // 细→粗 pixel_thresholds：[0]中近、[1]远中、[2]超远转远。
        self.view_lod_pixel_thresholds = Vec4::new(
            thresholds[1].x,
            thresholds[1].y,
            thresholds[0].x,
            thresholds[0].y,
        ); // 完成两段内容级屏幕阈值打包。
        self.view_lod_ultra_fade = Vec4::new(
            thresholds[2].x,
            thresholds[2].y,
            config.camera_fade_distances.x, // 镜头穿入当前内容后的整株隐藏距离。
            config.camera_fade_distances.y, // 当前内容完全恢复距离。
        ); // 完成超远阈值与近镜头避让打包。
        self.view_lod_residency_radii = Vec4::new(
            config.residency_radii[0],
            config.residency_radii[1],
            config.residency_radii[2],
            config.residency_radii[3],
        ); // 前四档独立常驻半径；不足四档的尾槽保持构造器安全默认值。
        self.view_lod_residency_transitions = Vec4::new(
            config.residency_transition_widths[0], // 近级向中级回退宽度。
            config.residency_transition_widths[1], // 中级向远级回退宽度。
            config.residency_transition_widths[2], // 远级向超远级回退宽度。
            config.residency_margin,               // 子级共同使用的外沿安全余量。
        ); // 完成不会改变玩法视图或其他草种的局部常驻规格打包。
        let view = GpuBladeView::screen_space_lod_with(config);
        self.view_lod_pixel_thresholds_ext = view.lod_pixel_thresholds_ext;
        self.view_lod_residency_radii_ext = view.lod_residency_radii_ext;
        self.view_lod_residency_transitions_ext = view.lod_residency_transitions_ext;
        self.view_lod_metadata = view.lod_metadata;
    }
}
