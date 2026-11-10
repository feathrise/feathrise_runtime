//! 正式大场景动态水面的背景波、互动波纹、接触深度和资源容量参数。

pub(super) const PRIMARY_WAVE_AMPLITUDE: f32 = 0.015; // 背景主波只保留一点五厘米轻微流动，避免宽缓反光覆盖互动同心波。
pub(super) const CROSSING_WAVE_AMPLITUDE: f32 = 0.008; // 斜向次波限制在八毫米，只用于打破完全静止感而不产生大片白面。
pub(super) const WATER_TRANSPARENT_SORT_DEPTH_BIAS: f32 = -10_000.0; // 把千米水面稳定排到透明队列前端，避免随相机视角覆盖名字与技能特效。
pub(super) const PLAYER_RIPPLE_CAPACITY: usize = 8; // 起跳与落水统一为长期大波后保留八个事件，约覆盖四个完整起落周期且仍是固定成本。
pub(super) const PLAYER_RIPPLE_SPACING: f32 = 1.8; // 玩家在水中每移动约一点八米发出一组波列，匹配高密度水面网格的有效分辨率。
pub(super) const PLAYER_RIPPLE_LIFETIME: f32 = 2.4; // 每组环形波包用二点四秒完成外扩，旧波离开脚下后再自然消失。
pub(super) const PLAYER_RIPPLE_SPEED: f32 = 2.0; // 主波峰以每秒两米传播，寿命内半径不超过四点八米。
pub(super) const PLAYER_RIPPLE_BAND_HALF_WIDTH: f32 = 2.8; // 波动只存在于主波峰前后各二点八米的环带，环内会恢复平水。
pub(super) const PLAYER_RIPPLE_LAYER_WAVELENGTH: f32 = 2.8; // 二点八米完整波长形成约一点四米层间距，接近角色身高且网格可辨。
pub(super) const PLAYER_RIPPLE_ATTACK_SECONDS: f32 = 0.08; // 用八十毫秒建立局部振幅，使紧凑波纹迅速可见但不产生单帧跳变。
pub(super) const PLAYER_RIPPLE_LANDING_MIN_FALL_SPEED: f32 = 1.0; // 深水脚底下降速度超过每秒一米才记录明确回落，起跳改由Tnua真实动作序号授权。
pub(super) const BANK_WAVE_FADE_UV_WIDTH: f32 = 0.12; // 只在靠近两岸的百分之十二水带内衰减，岸边固定但河心保留完整高低波。
pub(super) const PLAYER_RIPPLE_ENTRY_AMPLITUDE: f32 = 0.07; // 入水波使用七厘米初始波高，明显高于背景但远低于角色脚踝高度。
pub(super) const PLAYER_RIPPLE_MIN_STEP_AMPLITUDE: f32 = 0.04; // 慢走使用四厘米近身波，通过同心明暗层而不是夸张高度辨识。
pub(super) const PLAYER_RIPPLE_MAX_STEP_AMPLITUDE: f32 = 0.06; // 疾跑脚步波上限为六厘米，连续移动叠加时仍保持克制。
pub(super) const PLAYER_RIPPLE_JUMP_AMPLITUDE: f32 = 0.09; // 每次真实起跳、落水或深水着底都生成同一种九厘米长期强波。
pub(super) const PLAYER_RIPPLE_MAX_HEIGHT: f32 = 0.12; // 多个波源最终限制在正负十二厘米，杜绝人物周围形成高大水墙。
pub(super) const PLAYER_RIPPLE_TROUGH_DARKENING: f32 = 0.18; // 只把波谷最多压暗百分之十八，使同心层可读且不会再额外增白。
pub(super) const PLAYER_RIPPLE_MIN_DEPTH: f32 = 0.01; // 脚底至少低于静止水位一厘米才算入水，排除齐平陆地和物理浮点抖动。
pub(super) const PLAYER_RIPPLE_MAX_DEPTH: f32 = 3.0; // 当前河床约深二点四米，允许玩家踩在河床时继续扰动上方水面。
pub(super) const PLAYER_RIPPLE_TELEPORT_DISTANCE: f32 = 12.0; // 单帧位移超过十二米视为传送，避免整段路径错误发出波纹。
pub(super) const RIBBON_STYLE_SENTINEL: f32 = 16.0; // 样条UV1.y与Blender整网格uniform流向x槽共用的高位哨兵，必须与WGSL一致。
