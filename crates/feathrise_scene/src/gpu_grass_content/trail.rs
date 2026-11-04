//! 通用动态植被的固定容量玩家压草轨迹。
//! 足迹只保存七个世界坐标样本；切图和玩家缺失时清零，稳定帧不分配。

use bevy::prelude::{Resource, Vec2};
use feathrise_environment::vegetation::blade::{
    GPU_GRASS_INTERACTION_SOURCE_CAPACITY, GpuBladeInteractionSource,
};

/// 玩家当前位置持续压弯脚边约一米范围内的可见草叶。
pub(crate) const PLAYER_GRASS_RADIUS: f32 = 1.05;

/// 玩家当前位置使用接近满值但不过度贴地的弯曲强度。
pub(crate) const PLAYER_GRASS_STRENGTH: f32 = 0.95;

/// 每移动六十五厘米记录一个短寿命足迹，快速奔跑时仍不会出现明显未压草断点。
pub(crate) const GRASS_TRAIL_SPACING: f32 = 0.65;

/// 历史足迹在一点四秒内二次衰减，让玩家离开后草叶平滑回弹。
pub(crate) const GRASS_TRAIL_LIFETIME: f32 = 1.40;

/// 历史足迹比当前玩家略弱，避免多个源重叠时形成完全贴地的大圆坑。
pub(crate) const GRASS_TRAIL_STRENGTH: f32 = 0.78;

/// 单帧水平位移超过十二米视为传送，清空旧轨迹而不在地图间连出压草直线。
pub(crate) const GRASS_TRAIL_TELEPORT_DISTANCE: f32 = 12.0;
const GRASS_TRAIL_CAPACITY: usize = GPU_GRASS_INTERACTION_SOURCE_CAPACITY - 1;

/// 通用草地交互系统的固定容量 CPU 采样状态。
#[derive(Resource, Default)]
pub(crate) struct BladeTrailState {
    pub(crate) samples: [BladeTrailSample; GRASS_TRAIL_CAPACITY], // 七个槽与 GPU 尾部源一一对应。
    pub(crate) next_slot: usize, // 环形覆盖最旧槽，运行时间再长也不会分配或增长内存。
    pub(crate) last_emission_xz: Option<Vec2>, // 以上次实际足迹位置累计移动距离，低帧率奔跑也能补齐中间采样。
    pub(crate) last_player_xz: Option<Vec2>,   // 只用于识别传送和场景重进的大距离跳变。
}

/// 一个不会生成 ECS 实体的短寿命玩家压草采样点。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BladeTrailSample {
    pub(crate) position_xz: Vec2, // 足迹生成时的玩家世界 X/Z 位置。
    pub(crate) age_seconds: f32,  // 从零增长到一点四秒并在 GPU 强度中执行二次衰减。
    pub(crate) active: bool,      // 固定数组槽是否保存尚未过期的有效足迹。
}

impl BladeTrailState {
    /// 清空固定足迹槽和位置历史，下一次玩家出现时从当前位置重新开始。
    pub(crate) fn clear(&mut self) {
        self.samples.fill(BladeTrailSample::default()); // 所有历史源立即失效。
        self.next_slot = 0; // 环形写指针恢复到首槽便于测试和调试复现。
        self.last_emission_xz = None; // 下一帧不会从旧地图位置补画足迹。
        self.last_player_xz = None; // 下一帧玩家位置作为新的连续移动起点。
    }

    /// 推进足迹寿命，并按世界移动距离在固定数组中补齐采样点。
    pub(crate) fn update(&mut self, player_xz: Vec2, delta_seconds: f32) {
        let safe_delta_seconds = delta_seconds.clamp(0.0, 0.25); // 长停顿或断点调试不会在一帧产生异常时间跨度。
        for sample in &mut self.samples {
            if !sample.active {
                continue; // 空槽不参与寿命更新。
            }
            sample.age_seconds += safe_delta_seconds; // 历史足迹随游戏时间平滑衰减。
            if sample.age_seconds >= GRASS_TRAIL_LIFETIME {
                sample.active = false; // 到期槽立即可被后续移动环形复用。
            }
        }
        if self.last_player_xz.is_none() {
            self.last_player_xz = Some(player_xz); // 首帧只建立连续移动参考点。
            self.last_emission_xz = Some(player_xz); // 当前玩家源已经压草，无需额外生成同位置历史点。
            return; // 等待玩家真正移动超过采样间距。
        }
        let previous_player_xz = self.last_player_xz.unwrap_or(player_xz); // 已由前置分支保证存在，保留安全回退。
        if player_xz.distance(previous_player_xz) > GRASS_TRAIL_TELEPORT_DISTANCE {
            self.clear(); // 传送或切图时禁止沿大段直线补出七个错误足迹。
            self.last_player_xz = Some(player_xz); // 新位置成为连续移动起点。
            self.last_emission_xz = Some(player_xz); // 新位置由当前玩家源立即覆盖。
            return; // 本帧不生成历史源。
        }
        self.last_player_xz = Some(player_xz); // 保存本帧位置供下一帧传送判断。
        let mut emission_xz = self.last_emission_xz.unwrap_or(player_xz); // 取得上次真实足迹锚点。
        let to_player = player_xz - emission_xz; // 计算尚未被固定间距采样覆盖的水平位移。
        let travel_distance = to_player.length(); // 一次平方根仅发生在 CPU 唯一玩家上，与草株数量无关。
        if travel_distance < GRASS_TRAIL_SPACING {
            return; // 位移不足一个采样间距时保留当前玩家源即可。
        }
        let travel_direction = to_player / travel_distance; // 合法非零距离下得到连续路径方向。
        let emission_count =
            ((travel_distance / GRASS_TRAIL_SPACING).floor() as usize).min(GRASS_TRAIL_CAPACITY); // 单帧最多补满七槽，禁止极端高速造成无界循环。
        for _ in 0..emission_count {
            emission_xz += travel_direction * GRASS_TRAIL_SPACING; // 沿真实移动线每六十五厘米推进一个采样。
            self.samples[self.next_slot] = BladeTrailSample {
                position_xz: emission_xz, // 当前路径采样点成为新的历史压草中心。
                age_seconds: 0.0,         // 新足迹从满强度开始进入二次衰减。
                active: true,             // 该槽从本帧起上传 GPU。
            };
            self.next_slot = (self.next_slot + 1) % GRASS_TRAIL_CAPACITY; // 固定环形数组覆盖最旧足迹。
        }
        self.last_emission_xz = Some(emission_xz); // 保存最后实际采样位置而非玩家位置，避免小位移累计丢失。
    }

    /// 把当前玩家和有效历史足迹写成 GPU 固定数组的连续前缀。
    pub(crate) fn write_gpu_sources(
        &self,           // 只读取固定足迹状态，不在上传时修改寿命。
        player_xz: Vec2, // 第一槽始终使用玩家本帧最终世界位置。
        output: &mut [GpuBladeInteractionSource; GPU_GRASS_INTERACTION_SOURCE_CAPACITY], // 调用方栈上固定数组避免堆分配。
    ) -> usize {
        output[0] = GpuBladeInteractionSource::new(
            player_xz,             // 玩家脚边中心连续跟随而不按草格量化。
            PLAYER_GRASS_RADIUS,   // 当前玩家保持约一米压草范围。
            PLAYER_GRASS_STRENGTH, // 当前玩家源保持稳定强度。
        ); // 第一源保证静止玩家脚边草仍保持弯曲。
        let mut output_count = 1usize; // 当前玩家存在时至少上传一个有效源。
        for sample in self.samples.iter().filter(|sample| sample.active) {
            if output_count >= GPU_GRASS_INTERACTION_SOURCE_CAPACITY {
                break; // 固定数组容量由草地 crate 统一约束，尾部不得越界。
            }
            let remaining = (1.0 - sample.age_seconds / GRASS_TRAIL_LIFETIME).clamp(0.0, 1.0); // 把剩余寿命映射到零至一。
            let strength = remaining * remaining * GRASS_TRAIL_STRENGTH; // 二次曲线先保持形变再柔和收尾回弹。
            output[output_count] = GpuBladeInteractionSource::new(
                sample.position_xz,  // 历史采样点保持世界稳定不追随玩家。
                PLAYER_GRASS_RADIUS, // 足迹半径与当前玩家连续压草范围一致。
                strength,            // 上传已经包含寿命衰减的实际弯曲权重。
            ); // 完成一个连续历史源写入。
            output_count += 1; // 推进下一 GPU 槽。
        }
        output_count // 返回本帧需要上传的连续源切片长度。
    }
}
