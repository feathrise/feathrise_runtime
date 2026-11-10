use super::*;

impl GpuBladePatch {
    /// 为密度图非零区域设置概率增益；零纹素乘任何增益仍严格禁草。
    pub fn with_density_gain(mut self, gain: f32) -> Self {
        self.set_density_gain(gain); // 复用可变接口的有限范围约束。
        self // 返回可继续链式配置的草块。
    }

    /// 更新场景密度概率增益，不改变密度图本身的空间形状和零值遮罩。
    pub fn set_density_gain(&mut self, gain: f32) {
        if !gain.is_finite() {
            return; // 非有限增益保持上一合法值，禁止GPU概率比较被NaN永久污染。
        } // 完成密度增益有限性保护。
        self.lod_density.w = gain.clamp(0.0, 64.0); // 有限上界防止错误配置产生无意义的极大uniform值。
    }

    /// 返回当前密度图概率增益。
    pub fn density_gain(self) -> f32 {
        self.lod_density.w // 场景测试可核对场景调优值而不直接访问uniform布局。
    }

    /// 返回当前LOD环带完全不可见的内半径。
    pub fn inner_radius(self) -> f32 {
        self.lod_density.x // 零表示该窗口负责玩家脚边最密的一圈草。
    }

    /// 返回该批次一次绘制需要提交的实例数量。
    pub fn instance_count(self) -> u32 {
        self.grid.y // 总数在 CPU 和 WGSL 之间只维护一份。
    }

    /// 返回正方形候选网格单边格数，供渲染世界建立粗瓦片列表。
    pub(crate) fn grid_side(self) -> u32 {
        self.grid.x // 字段仍保持私有，避免场景层绕过构造器破坏候选总数一致性。
    }

    /// 返回相邻候选草格的世界间距。
    pub fn grid_spacing(self) -> f32 {
        self.origin_extent.w / (self.grid.x.saturating_sub(1).max(1) as f32) // 单格窗口仍安全回退为完整边长。
    }

    /// 返回当前草场窗口的世界中心。
    pub fn origin(self) -> Vec3 {
        self.origin_extent.truncate() // 只暴露位置，不允许场景层直接修改 uniform 内部布局。
    }

    /// 同步草场窗口世界中心；调用方还必须同步实体 Transform 供 AABB 剔除使用。
    pub fn set_origin(&mut self, origin: Vec3) {
        if !origin.is_finite() {
            return; // 三个坐标作为一个原子位置拒绝异常值，避免Transform与GPU窗口只更新部分轴。
        } // 完成窗口中心有限性保护。
        self.origin_extent.x = origin.x; // 顶点着色器使用新的世界 X 中心生成窗口草格。
        self.origin_extent.y = origin.y; // 阶段1仍保留调用方提供的统一高度参考面。
        self.origin_extent.z = origin.z; // 顶点着色器使用新的世界 Z 中心生成窗口草格。
    }

    /// 更新连续观察中心；X/Z影响常驻距离，Y为屏幕LOD提供局部地表参考，不移动稳定世界源草格。
    pub fn set_focus(&mut self, focus: Vec3) {
        if !focus.is_finite() {
            return; // 非有限观察中心保持上一帧合法玩家位置，避免三个LOD圈同时被NaN剔除。
        } // 完成连续观察中心有限性保护。
        self.focus_radius.x = focus.x; // 玩家连续 X 位置驱动圆形可见范围。
        self.focus_radius.y = focus.y; // 玩家根Y近似局部地表高度，降低河槽与堤岸投影LOD误差。
        self.focus_radius.z = focus.z; // 玩家连续 Z 位置驱动圆形可见范围。
    }

    /// 返回当前连续观察中心，供动态窗口验证和调试遥测读取。
    pub fn focus(self) -> Vec3 {
        self.focus_radius.truncate() // 不暴露同一Vec4内的可见半径写权限。
    }

    /// 返回当前LOD环带逐株草完全消失的外半径。
    pub fn visible_radius(self) -> f32 {
        self.focus_radius.w // 场景和测试可以核对窗口边缘是否保留安全带。
    }

    /// 为当前草场启用世界坐标高度单元纹理。
    pub fn with_height_field(
        mut self,        // 按值接收窗口，支持在spawn元组中链式配置。
        world_min: Vec2, // 高度纹理覆盖的最小世界X/Z坐标。
        world_max: Vec2, // 高度纹理覆盖的最大世界X/Z坐标。
        height_min: f32, // RGBA16零值解码后的最低米制高度。
        height_max: f32, // RGBA16一值解码后的最高米制高度。
    ) -> Self {
        self.set_height_field(world_min, world_max, height_min, height_max); // 复用可变接口的范围校验与打包规则。
        self // 返回启用真实地表采样的窗口。
    }

    /// 更新世界高度纹理的坐标与解码契约，不生成任何逐草CPU数据。
    pub fn set_height_field(
        &mut self,       // 只修改当前窗口的两组vec4参数。
        world_min: Vec2, // 高度纹理覆盖的最小世界X/Z坐标。
        world_max: Vec2, // 高度纹理覆盖的最大世界X/Z坐标。
        height_min: f32, // RGBA16零值解码后的最低米制高度。
        height_max: f32, // RGBA16一值解码后的最高米制高度。
    ) {
        if !world_min.is_finite()
            || !world_max.is_finite()
            || !height_min.is_finite()
            || !height_max.is_finite()
        {
            return; // 高度契约必须整体有效才启用，避免世界范围或解码跨度向GPU传播非有限值。
        } // 完成高度纹理四项输入的原子有限性校验。
        let safe_world_max = world_max.max(world_min + Vec2::splat(0.01)); // 防止零范围导致shader格距除零。
        let safe_height_max = height_max.max(height_min + 0.001); // 保证十六位归一化值能够还原为有效高度跨度。
        self.height_world = Vec4::new(
            world_min.x,      // shader按世界X定位512列地形单元。
            world_min.y,      // shader按世界Z定位512行地形单元。
            safe_world_max.x, // 世界X上界同时用于场景外候选裁剪。
            safe_world_max.y, // 世界Z上界同时用于场景外候选裁剪。
        ); // 完成高度纹理的世界矩形打包。
        self.height_decode = Vec4::new(
            height_min,                   // R/G/B/A通道零值对应的米制高度。
            safe_height_max - height_min, // 乘以UNORM采样值恢复完整米制跨度。
            1.0,                          // 非零标记要求顶点shader启用真实地表采样。
            self.height_decode.w,         // 更换高度资产不得打断阶段5正在进行的全局淡入覆盖率。
        ); // 完成高度解码参数打包。
    }

    /// 返回当前窗口是否已经启用真实地表高度采样。
    pub fn uses_height_field(self) -> bool {
        self.height_decode.z > 0.5 // 测试与场景装配可核对固定平面和高度纹理路径。
    }

    /// 链式设置整个草窗的稳定抖动激活覆盖率，用于首次驻留数据就绪后的无块状淡入。
    pub fn with_activation_coverage(mut self, coverage: f32) -> Self {
        self.set_activation_coverage(coverage); // 复用可变接口的有限值保护与零到一钳制。
        self // 返回可继续配置高度、密度与LOD范围的轻量窗口。
    }

    /// 更新整个草窗的激活覆盖率；Compute用它执行世界稳定整株淡入且不再增加额外uniform字段。
    pub fn set_activation_coverage(&mut self, coverage: f32) {
        if !coverage.is_finite() {
            return; // 非有限流送状态保持上一合法覆盖率，禁止NaN传播到实例压缩与整株概率选择。
        } // 完成阶段5淡入参数的有限性保护。
        self.height_decode.w = coverage.clamp(0.0, 1.0); // 零表示全部实例隐藏，一表示恢复完整世界稳定候选集合。
    }

    /// 返回当前草窗的全局激活覆盖率。
    pub fn activation_coverage(self) -> f32 {
        self.height_decode.w // 场景状态机与测试使用同一已钳制GPU参数。
    }

    /// 为当前草场启用世界坐标密度概率纹理。
    pub fn with_density_field(mut self, world_min: Vec2, world_max: Vec2) -> Self {
        self.set_density_field(world_min, world_max); // 复用可变接口的范围校验、打包和启用位规则。
        self // 返回可继续链式装配高度与密度输入的轻量窗口。
    }

    /// 更新密度纹理的世界范围并显式启用GPU红通道概率采样。
    pub fn set_density_field(&mut self, world_min: Vec2, world_max: Vec2) {
        if !world_min.is_finite() || !world_max.is_finite() {
            return; // 非有限世界矩形保持旧密度契约，不设置启用位也不产生场景外错误采样。
        } // 完成密度世界范围有限性保护。
        let safe_world_max = world_max.max(world_min + Vec2::splat(0.01)); // 防止零范围导致shader归一化除零。
        self.density_world = Vec4::new(
            world_min.x,      // shader按世界X定位密度图西侧边界。
            world_min.y,      // shader按世界Z定位密度图北侧边界。
            safe_world_max.x, // 世界X上界同时用于场景外严格禁种。
            safe_world_max.y, // 世界Z上界同时用于场景外严格禁种。
        ); // 完成独立密度世界矩形打包。
        self.grid.w |= DENSITY_FIELD_ENABLED_FLAG; // 只占用最低标志位，不覆盖未来其他绘制模式。
    }

    /// 返回当前窗口是否已经启用密度概率采样。
    pub fn uses_density_field(self) -> bool {
        self.grid.w & DENSITY_FIELD_ENABLED_FLAG != 0 // 测试、场景装配和渲染等待共用同一位定义。
    }

    /// 为当前密度图追加一个世界坐标受控种植矩形。
    ///
    /// 【功能设计】密度图仍决定区域内部的逐纹素概率、水面和道路净空；该矩形只负责把首轮技术接入
    /// 限制在指定栖息地，后续扩大范围时无需重做或裁切原始密度资产。
    pub fn with_density_control_region(mut self, world_min: Vec2, world_max: Vec2) -> Self {
        self.set_density_control_region(world_min, world_max); // 复用可变接口的有限值和严格正范围校验。
        self // 返回可继续配置LOD、地形高度和内容profile的轻量窗口。
    }

    /// 更新当前密度图的额外受控种植矩形；无效输入完整保留上一份合法范围。
    pub fn set_density_control_region(&mut self, world_min: Vec2, world_max: Vec2) {
        if !world_min.is_finite()
            || !world_max.is_finite()
            || world_max.x <= world_min.x
            || world_max.y <= world_min.y
        {
            return; // 非有限、零宽或反向矩形均不能形成部分有效的GPU空间契约。
        } // 完成受控种植区的原子输入校验。
        self.density_control_world = Vec4::new(
            world_min.x, // 保存受控区世界X最小边。
            world_min.y, // 保存受控区世界Z最小边。
            world_max.x, // 保存受控区世界X最大边。
            world_max.y, // 保存受控区世界Z最大边。
        ); // 完成独立于密度纹理世界映射的受控区打包。
    }

    /// 返回当前窗口是否启用了额外受控种植矩形。
    pub fn uses_density_control_region(self) -> bool {
        self.density_control_world.z > self.density_control_world.x
            && self.density_control_world.w > self.density_control_world.y // 默认零范围保持禁用，合法矩形才追加GPU裁剪。
    }

    /// 仅供渲染世界在任一必需地表纹理尚未上传时跳过本帧绘制。
    pub(crate) fn suppress_draw_for_render_frame(&mut self) {
        self.grid.y = 0; // 下一帧主世界提取会恢复真实实例数，不让错误的全密度或固定平面草闪现。
    }

    /// 仅供渲染世界标记本帧复用上一份有效GPU输出，避免纹理热重载期间覆盖间接状态。
    pub(crate) fn retain_resident_output_for_render_frame(&mut self) {
        self.grid.w |= RETAIN_RESIDENT_OUTPUT_FLAG; // 标志位只存在于当前提取副本，下一帧会由主世界参数自然清除。
    }

    /// 返回当前渲染副本是否应跳过Compute并继续消费驻留输出。
    pub(crate) fn retains_resident_output_for_render_frame(self) -> bool {
        self.grid.w & RETAIN_RESIDENT_OUTPUT_FLAG != 0 // 高位不影响密度采样与两个LOD材质位。
    }

    /// 按若干草格为一步，把连续观察位置转换为不会让草场滑动的窗口中心。
    pub fn snapped_origin_xz(self, focus: Vec2, snap_cells: u32) -> Vec2 {
        if !focus.is_finite() {
            return self.origin().xz(); // 玩家坐标异常时维持当前窗口中心，禁止生成NaN Transform和AABB。
        } // 完成吸附输入有限性保护。
        let snap_step = self.grid_spacing() * snap_cells.max(1) as f32; // 至少按一个世界草格移动，绝不连续跟随玩家。
        (focus / snap_step).round() * snap_step // 跨边界时只替换窗口最外圈，重叠世界草格保持不变。
    }
}
