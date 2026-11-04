//! 游戏与编辑器共用的数值解析、规范化、周期边界和步进纯算法。

use super::*;

pub fn format_numeric_value(value: NumericValue, spec: NumericInputSpec) -> String {
    match normalize_numeric_value(value, spec) {
        NumericValue::U32(value) => value.to_string(),
        NumericValue::I32(value) => value.to_string(), // 有符号整数保留首位负号且不显示小数。
        NumericValue::F32(value) => format!("{:.*}", spec.decimals as usize, value),
    }
}

/// 按输入规格统一完成跨类型转换、小数位整理以及钳制或周期边界处理。
pub fn normalize_numeric_value(value: NumericValue, spec: NumericInputSpec) -> NumericValue {
    match spec.kind {
        NumericValueKind::U32 => {
            let converted = match value {
                NumericValue::U32(value) => value, // 同类型值不丢失任何位。
                NumericValue::I32(value) => value.max(0) as u32, // 负有符号值在无符号边界安全归零。
                NumericValue::F32(value) => value.max(0.0).round() as u32, // 外部浮点赋值按最近整数进入离散域。
            };
            NumericValue::U32(normalize_u32(converted, spec)) // 所有来源最终共享同一边界语义。
        }
        NumericValueKind::I32 => {
            let converted = match value {
                NumericValue::U32(value) => value.min(i32::MAX as u32) as i32, // 过大无符号值饱和到i32上界。
                NumericValue::I32(value) => value, // 同类型值不做多余转换。
                NumericValue::F32(value) => value.round() as i32, // 浮点作者值按最近整数进入优先级等离散域。
            };
            NumericValue::I32(normalize_i32(converted, spec)) // 统一处理负数范围和可选循环。
        }
        NumericValueKind::F32 => {
            let converted = match value {
                NumericValue::U32(value) => value as f32, // 公开外部赋值允许整数进入浮点字段。
                NumericValue::I32(value) => value as f32, // 有符号整数转换时保留符号。
                NumericValue::F32(value) => value,        // 同类型值保持原始连续量。
            };
            NumericValue::F32(normalize_f32(converted, spec)) // 小数位与边界只在一个入口维护。
        }
    }
}

/// 返回按从小到大排序的无符号边界，损坏的反向配置不会触发`clamp`恐慌。
fn u32_bounds(spec: NumericInputSpec) -> (u32, u32) {
    let first = spec.min.and_then(NumericValue::as_u32).unwrap_or(0); // 未声明下界时使用类型下界。
    let second = spec.max.and_then(NumericValue::as_u32).unwrap_or(u32::MAX); // 未声明上界时使用类型上界。
    (first.min(second), first.max(second)) // 统一修正意外反向配置。
}

/// 规范化无符号整数；周期模式使用半开区间，因此最大端点回到最小端点。
fn normalize_u32(value: u32, spec: NumericInputSpec) -> u32 {
    let (minimum, maximum) = u32_bounds(spec); // 每次只解析一次同类型边界。
    if spec.boundary_mode == NumericInputBoundaryMode::Wrap
        && spec.min.and_then(NumericValue::as_u32).is_some()
        && spec.max.and_then(NumericValue::as_u32).is_some()
        && maximum > minimum
    {
        let span = u64::from(maximum - minimum); // u64避免端点距离计算溢出。
        let offset = if value >= minimum {
            u64::from(value - minimum) % span // 上方值直接取正向余数。
        } else {
            (span - u64::from(minimum - value) % span) % span // 下方值从周期尾部反向折回。
        };
        minimum + offset as u32 // offset严格小于span，转换和加法均安全。
    } else {
        value.clamp(minimum, maximum) // 无完整有效周期时安全退化为既有钳制。
    }
}

/// 返回按从小到大排序的有符号边界。
fn i32_bounds(spec: NumericInputSpec) -> (i32, i32) {
    let first = spec.min.and_then(NumericValue::as_i32).unwrap_or(i32::MIN); // 缺省覆盖完整负数域。
    let second = spec.max.and_then(NumericValue::as_i32).unwrap_or(i32::MAX); // 缺省覆盖完整正数域。
    (first.min(second), first.max(second)) // 反向作者配置仍得到确定闭区间。
}

/// 规范化有符号整数并允许小范围周期离散量复用同一公共语义。
fn normalize_i32(value: i32, spec: NumericInputSpec) -> i32 {
    let (minimum, maximum) = i32_bounds(spec); // 边界只从同类型值读取。
    if spec.boundary_mode == NumericInputBoundaryMode::Wrap
        && spec.min.and_then(NumericValue::as_i32).is_some()
        && spec.max.and_then(NumericValue::as_i32).is_some()
        && maximum > minimum
    {
        let span = i64::from(maximum) - i64::from(minimum); // i64覆盖完整i32端点差。
        let offset = (i64::from(value) - i64::from(minimum)).rem_euclid(span); // 负值也按数学余数循环。
        (i64::from(minimum) + offset) as i32 // 半开区间结果必然落回i32范围。
    } else {
        value.clamp(minimum, maximum) // 普通优先级等参数保持闭区间钳制。
    }
}

/// 返回有序浮点边界；NaN边界按类型缺省值替换，避免污染整个控件模型。
fn f32_bounds(spec: NumericInputSpec) -> (f32, f32) {
    let first = spec.min.and_then(NumericValue::as_f32).unwrap_or(f32::MIN); // 缺省使用有限类型下界。
    let second = spec.max.and_then(NumericValue::as_f32).unwrap_or(f32::MAX); // 缺省使用有限类型上界。
    let first = if first.is_nan() { f32::MIN } else { first }; // 非数下界不能传入`clamp`。
    let second = if second.is_nan() { f32::MAX } else { second }; // 非数上界回退有限最大值。
    (first.min(second), first.max(second)) // 反向配置也得到稳定顺序。
}

/// 把有限浮点值放入规格范围；循环模式要求调用方明确给出两个同类型端点。
fn bound_f32(value: f32, spec: NumericInputSpec) -> f32 {
    let (minimum, maximum) = f32_bounds(spec); // 钳制与循环共享同一有序范围。
    let span = maximum - minimum; // 周期长度单独受检，极端端点差不能溢出为无穷。
    if spec.boundary_mode == NumericInputBoundaryMode::Wrap
        && spec.min.and_then(NumericValue::as_f32).is_some()
        && spec.max.and_then(NumericValue::as_f32).is_some()
        && maximum > minimum
        && span.is_finite()
        && value.is_finite()
    {
        (value - minimum).rem_euclid(span) + minimum // 最大端点和多圈输入都回到半开区间。
    } else if value.is_nan() {
        minimum // NaN没有方向信息，确定回退业务最小值而不是污染模型。
    } else {
        value.clamp(minimum, maximum) // 非有限输入或不完整周期配置安全退化为端点钳制。
    }
}

/// 浮点规范化先应用边界再舍入，并再次应用边界避免舍入产生周期最大端点。
fn normalize_f32(value: f32, spec: NumericInputSpec) -> f32 {
    let bounded = bound_f32(value, spec); // 先把任意外部值收进正式业务域。
    let rounded = round_to_decimals(bounded, spec.decimals); // 显示值和模型值保持相同精度。
    bound_f32(rounded, spec) // 例如359.99舍入为360时仍应循环回0。
}

pub fn parse_numeric_buffer(buffer: &str, spec: NumericInputSpec) -> Option<NumericValue> {
    if buffer.is_empty() {
        return None;
    }
    match spec.kind {
        NumericValueKind::U32 => buffer
            .parse::<u32>()
            .ok()
            .map(|value| normalize_numeric_value(NumericValue::U32(value), spec)),
        NumericValueKind::I32 => buffer
            .parse::<i32>() // 正式支持负优先级等有符号离散作者参数。
            .ok()
            .map(|value| normalize_numeric_value(NumericValue::I32(value), spec)),
        NumericValueKind::F32 => buffer
            .parse::<f32>()
            .ok()
            .map(|value| normalize_numeric_value(NumericValue::F32(value), spec)),
    }
}

pub fn step_numeric_value(
    current: NumericValue,
    spec: NumericInputSpec,
    direction: f32,
) -> NumericValue {
    let step = spec.step.unwrap_or(match spec.kind {
        NumericValueKind::U32 => NumericValue::U32(1),
        NumericValueKind::I32 => NumericValue::I32(1), // 有符号整数默认一步一个单位。
        NumericValueKind::F32 => NumericValue::F32(1.0),
    });
    match (spec.kind, normalize_numeric_value(current, spec), step) {
        (NumericValueKind::U32, NumericValue::U32(current), NumericValue::U32(step)) => {
            let signed_step = i64::from(step) * i64::from(direction.signum() as i8); // i64容纳u32上下两个方向。
            let candidate = i64::from(current) + signed_step; // 候选值先保留低于零或高于u32的越界信息。
            if spec.boundary_mode == NumericInputBoundaryMode::Wrap {
                NumericValue::U32(wrap_u32_candidate(candidate, spec)) // 周期步进不能先被无符号饱和吞掉。
            } else {
                let next = candidate.clamp(0, i64::from(u32::MAX)) as u32; // 普通边界先饱和到类型域。
                normalize_numeric_value(NumericValue::U32(next), spec) // 再服从业务端点。
            }
        }
        (NumericValueKind::I32, NumericValue::I32(current), NumericValue::I32(step)) => {
            let magnitude = i64::from(step.saturating_abs()); // 负的错误步长按正幅度处理。
            let candidate = i64::from(current) + magnitude * i64::from(direction.signum() as i8); // i64保留跨端点信息。
            if spec.boundary_mode == NumericInputBoundaryMode::Wrap {
                NumericValue::I32(wrap_i32_candidate(candidate, spec)) // 周期整数从另一端继续。
            } else {
                let next = candidate.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32; // 避免大步长溢出。
                normalize_numeric_value(NumericValue::I32(next), spec) // 最后应用业务钳制。
            }
        }
        (NumericValueKind::F32, NumericValue::F32(current), NumericValue::F32(step)) => {
            normalize_numeric_value(NumericValue::F32(current + step * direction.signum()), spec)
        }
        _ => normalize_numeric_value(current, spec),
    }
}

/// 把可能越过u32类型域的步进候选直接放入显式业务周期。
fn wrap_u32_candidate(candidate: i64, spec: NumericInputSpec) -> u32 {
    let Some(minimum) = spec.min.and_then(NumericValue::as_u32) else {
        return candidate.clamp(0, i64::from(u32::MAX)) as u32; // 不完整周期配置退回类型钳制。
    };
    let Some(maximum) = spec.max.and_then(NumericValue::as_u32) else {
        return candidate.clamp(0, i64::from(u32::MAX)) as u32; // 缺少上界不能定义周期长度。
    };
    let (minimum, maximum) = (minimum.min(maximum), minimum.max(maximum)); // 修正意外反向配置。
    let span = i64::from(maximum) - i64::from(minimum); // 半开区间长度不包含最大端点。
    if span <= 0 {
        return minimum; // 零长度范围退化为唯一端点。
    }
    (i64::from(minimum) + (candidate - i64::from(minimum)).rem_euclid(span)) as u32 // 负候选同样稳定循环。
}

/// 把i64步进候选直接放入显式i32业务周期，避免类型饱和破坏循环。
fn wrap_i32_candidate(candidate: i64, spec: NumericInputSpec) -> i32 {
    let Some(minimum) = spec.min.and_then(NumericValue::as_i32) else {
        return candidate.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32; // 不完整周期配置退回类型钳制。
    };
    let Some(maximum) = spec.max.and_then(NumericValue::as_i32) else {
        return candidate.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32; // 缺少上界不能定义周期长度。
    };
    let (minimum, maximum) = (minimum.min(maximum), minimum.max(maximum)); // 统一有序端点。
    let span = i64::from(maximum) - i64::from(minimum); // 完整i32差仍安全落在i64。
    if span <= 0 {
        return minimum; // 零长度范围只保留固定值。
    }
    (i64::from(minimum) + (candidate - i64::from(minimum)).rem_euclid(span)) as i32 // 返回半开业务区间。
}

pub fn push_numeric_char(buffer: &mut String, ch: char, spec: NumericInputSpec) -> bool {
    match spec.kind {
        NumericValueKind::U32 => {
            if ch.is_ascii_digit() {
                buffer.push(ch);
                true
            } else {
                false
            }
        }
        NumericValueKind::I32 => push_signed_integer_char(buffer, ch), // 有符号整数只允许首位负号和数字。
        NumericValueKind::F32 => push_float_char(buffer, ch, spec.decimals),
    }
}

/// 校验有符号整数的单个字符，避免复用浮点路径误接受小数点。
fn push_signed_integer_char(buffer: &mut String, ch: char) -> bool {
    if ch == '-' && buffer.is_empty() {
        buffer.push(ch); // 负号只能作为第一个字符出现一次。
        return true; // 明确报告缓冲已改变。
    }
    if ch.is_ascii_digit() {
        buffer.push(ch); // 数字可跟在空缓冲、负号或既有数字之后。
        return true; // 允许上层刷新显示和可选实时值。
    }
    false // 加号、小数点、第二个负号和其他字符全部拒绝。
}

fn push_float_char(buffer: &mut String, ch: char, decimals: u8) -> bool {
    // 浮点字段允许且只允许一个位于首字符的负号；无符号整数仍由上层分支明确拒绝符号。
    if ch == '-' {
        if !buffer.is_empty() {
            return false;
        }
        buffer.push(ch);
        return true;
    }
    if ch == '.' {
        if buffer.contains('.') {
            return false;
        }
        buffer.push(ch);
        return true;
    }
    if !ch.is_ascii_digit() {
        return false;
    }
    if let Some(dot) = buffer.find('.')
        && buffer.len().saturating_sub(dot + 1) >= decimals as usize
    {
        return false;
    }
    buffer.push(ch);
    true
}

pub fn round_to_decimals(value: f32, decimals: u8) -> f32 {
    let factor = 10_f32.powi(decimals as i32);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_parse_and_clamp() {
        let spec = NumericInputSpec::u32()
            .with_min(NumericValue::U32(1))
            .with_max(NumericValue::U32(999));
        assert_eq!(
            parse_numeric_buffer("9999", spec),
            Some(NumericValue::U32(999))
        );
        assert_eq!(parse_numeric_buffer("0", spec), Some(NumericValue::U32(1)));
    }

    #[test]
    fn u32_step_uses_same_clamp_path() {
        let spec = NumericInputSpec::u32()
            .with_min(NumericValue::U32(1))
            .with_max(NumericValue::U32(50))
            .with_step(NumericValue::U32(1));
        assert_eq!(
            step_numeric_value(NumericValue::U32(50), spec, 1.0),
            NumericValue::U32(50)
        );
        assert_eq!(
            step_numeric_value(NumericValue::U32(1), spec, -1.0),
            NumericValue::U32(1)
        );
    }

    #[test]
    fn f32_parse_clamp_and_format() {
        let spec = NumericInputSpec::f32(2)
            .with_min(NumericValue::F32(0.10))
            .with_max(NumericValue::F32(2.00));
        assert_eq!(
            parse_numeric_buffer("2.345", spec),
            Some(NumericValue::F32(2.00))
        );
        assert_eq!(format_numeric_value(NumericValue::F32(0.5), spec), "0.50");
    }

    #[test]
    fn float_char_validation_limits_fractional_digits() {
        let spec = NumericInputSpec::f32(2);
        let mut buffer = String::new();
        assert!(push_numeric_char(&mut buffer, '1', spec));
        assert!(push_numeric_char(&mut buffer, '.', spec));
        assert!(push_numeric_char(&mut buffer, '2', spec));
        assert!(push_numeric_char(&mut buffer, '3', spec));
        assert!(!push_numeric_char(&mut buffer, '4', spec));
        assert_eq!(buffer, "1.23");
    }

    #[test]
    fn f32_accepts_one_leading_minus_and_parses_negative_value() {
        let spec = NumericInputSpec::f32(3)
            .with_min(NumericValue::F32(-100.0))
            .with_max(NumericValue::F32(100.0));
        let mut buffer = String::new();
        assert!(push_numeric_char(&mut buffer, '-', spec));
        assert!(!push_numeric_char(&mut buffer, '-', spec));
        assert!(push_numeric_char(&mut buffer, '1', spec));
        assert!(!push_numeric_char(&mut buffer, '-', spec));
        assert!(push_numeric_char(&mut buffer, '.', spec));
        assert!(push_numeric_char(&mut buffer, '2', spec));
        assert!(push_numeric_char(&mut buffer, '5', spec));
        assert_eq!(buffer, "-1.25");
        assert_eq!(
            parse_numeric_buffer(&buffer, spec),
            Some(NumericValue::F32(-1.25))
        );
        assert_eq!(parse_numeric_buffer("-", spec), None);
    }

    #[test]
    fn u32_rejects_sign_characters() {
        let spec = NumericInputSpec::u32();
        let mut buffer = String::new();
        assert!(!push_numeric_char(&mut buffer, '-', spec));
        assert!(!push_numeric_char(&mut buffer, '+', spec));
        assert!(buffer.is_empty());
        assert_eq!(parse_numeric_buffer("-1", spec), None);
    }

    #[test]
    fn i32_accepts_negative_text_and_steps_across_zero() {
        let spec = NumericInputSpec::i32()
            .with_min(NumericValue::I32(-20)) // 测试正式负数业务下界。
            .with_max(NumericValue::I32(20)) // 测试正式正数业务上界。
            .with_step(NumericValue::I32(3)); // 使用非一整数步长覆盖步进路径。
        let mut buffer = String::new(); // 从空精确输入缓冲开始。
        assert!(push_numeric_char(&mut buffer, '-', spec)); // 首位负号必须被接受。
        assert!(push_numeric_char(&mut buffer, '7', spec)); // 负号后的数字必须被接受。
        assert!(!push_numeric_char(&mut buffer, '.', spec)); // 整数路径不能误接受小数点。
        assert_eq!(
            parse_numeric_buffer(&buffer, spec),
            Some(NumericValue::I32(-7))
        ); // 文本必须保留符号。
        assert_eq!(
            step_numeric_value(NumericValue::I32(-1), spec, 1.0),
            NumericValue::I32(2), // 向上一步跨过零点且保持完整步长。
        );
    }

    #[test]
    fn floating_wrap_uses_half_open_angle_range() {
        let spec = NumericInputSpec::f32(1)
            .with_min(NumericValue::F32(0.0)) // 周期起点为零度。
            .with_max(NumericValue::F32(360.0)) // 最大端点不重复表示零度。
            .with_boundary_mode(NumericInputBoundaryMode::Wrap); // 精确输入和外部赋值都启用循环。
        assert_eq!(
            normalize_numeric_value(NumericValue::F32(-15.0), spec),
            NumericValue::F32(345.0), // 负角度从周期末端折回。
        );
        assert_eq!(
            parse_numeric_buffer("375", spec),
            Some(NumericValue::F32(15.0)), // 多出一圈的精确输入回到十五度。
        );
        assert_eq!(
            normalize_numeric_value(NumericValue::F32(360.0), spec),
            NumericValue::F32(0.0), // 半开区间最大端点与零度等价。
        );
    }

    #[test]
    fn i32_wrap_step_preserves_negative_direction() {
        let spec = NumericInputSpec::i32()
            .with_min(NumericValue::I32(-2)) // 周期离散范围起点。
            .with_max(NumericValue::I32(3)) // 半开区间包含-2至2。
            .with_step(NumericValue::I32(1)) // 每次移动一个离散档。
            .with_wrap(true); // 便捷构造器同样必须启用正式循环语义。
        assert_eq!(
            step_numeric_value(NumericValue::I32(-2), spec, -1.0),
            NumericValue::I32(2), // 从最小值向下应回到最后一个有效整数。
        );
        assert_eq!(
            step_numeric_value(NumericValue::I32(2), spec, 1.0),
            NumericValue::I32(-2), // 从最后一个有效整数向上回到最小值。
        );
    }
}
