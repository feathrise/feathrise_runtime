//! 游戏子窗口为作者浮动控件让出原生可见区域；不缩放 Surface、不改变相机或拾取坐标。

use feathrise_editor_protocol::{MAX_VIEWPORT_EXCLUDED_RECTS, MAX_VIEWPORT_EXCLUDED_SHAPES, Mode, ViewportUiShape};

/// 将受检的局部物理尺寸转换为 GDI 的半开端点矩形。试玩保留完整画面。
fn exclusions(
    width: u32,
    height: u32,
    mode: Mode,
    rectangles: &[[u32; 4]],
) -> Result<Vec<[i32; 4]>, String> {
    if width > i32::MAX as u32
        || height > i32::MAX as u32
        || rectangles.len() > MAX_VIEWPORT_EXCLUDED_RECTS
    {
        return Err("原生视口尺寸或排除矩形数量越界".into());
    }
    let mut result = Vec::with_capacity(rectangles.len());
    for &[x, y, w, h] in rectangles {
        let right = x.checked_add(w).ok_or("原生排除矩形横向溢出")?;
        let bottom = y.checked_add(h).ok_or("原生排除矩形纵向溢出")?;
        if w == 0 || h == 0 || right > width || bottom > height {
            return Err("原生排除矩形必须完整位于工程视口内".into());
        }
        if mode == Mode::Editing {
            result.push([x as i32, y as i32, right as i32, bottom as i32]);
        }
    }
    Ok(result)
}

#[cfg(windows)]
struct Region(windows_sys::Win32::Graphics::Gdi::HRGN);

#[cfg(windows)]
impl Region {
    fn shape(shape: &ViewportUiShape) -> Result<Self, String> {
        use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::*};
        if !shape.valid() { return Err("作者轮廓无效".into()); }
        let handle = match shape {
            ViewportUiShape::Ellipse { outer: [l, t, r, b], .. } => unsafe { CreateEllipticRgn(*l, *t, *r, *b) },
            ViewportUiShape::Quad { points } => {
                let points = points.map(|[x, y]| POINT { x, y });
                unsafe { CreatePolygonRgn(points.as_ptr(), 4, WINDING) }
            }
        };
        if handle.is_null() { return Err("创建作者轮廓区域失败".into()); }
        let region = Self(handle);
        if let ViewportUiShape::Ellipse { inner: Some([l, t, r, b]), .. } = shape {
            let inner = Self(unsafe { CreateEllipticRgn(*l, *t, *r, *b) });
            if inner.0.is_null() || unsafe { CombineRgn(region.0, region.0, inner.0, RGN_DIFF) } == ERROR {
                return Err("创建作者空心圆环失败".into());
            }
        }
        Ok(region)
    }

    fn rectangle([left, top, right, bottom]: [i32; 4]) -> Result<Self, String> {
        let handle =
            unsafe { windows_sys::Win32::Graphics::Gdi::CreateRectRgn(left, top, right, bottom) };
        if handle.is_null() {
            Err(format!(
                "创建工程视口区域失败：{}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for Region {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(self.0);
        }
    }
}

#[cfg(windows)]
fn build(width: u32, height: u32, mode: Mode, rectangles: &[[u32; 4]], shapes: &[ViewportUiShape]) -> Result<Region, String> {
    use windows_sys::Win32::Graphics::Gdi::{CombineRgn, ERROR, RGN_DIFF};
    let exclusions = exclusions(width, height, mode, rectangles)?;
    if shapes.len() > MAX_VIEWPORT_EXCLUDED_SHAPES || shapes.iter().any(|shape| !shape.valid()) {
        return Err("作者轮廓无效或超过十个固定控件预算".into());
    }
    let full = Region::rectangle([0, 0, width.max(1) as i32, height.max(1) as i32])?;
    for rectangle in exclusions {
        let excluded = Region::rectangle(rectangle)?;
        if unsafe { CombineRgn(full.0, full.0, excluded.0, RGN_DIFF) } == ERROR {
            return Err(format!(
                "裁剪作者浮动控件区域失败：{}",
                std::io::Error::last_os_error()
            ));
        }
    }
    if mode == Mode::Editing {
        for shape in shapes {
            let excluded = Region::shape(shape)?;
            if unsafe { CombineRgn(full.0, full.0, excluded.0, RGN_DIFF) } == ERROR {
                return Err("裁剪作者透明轮廓失败".into());
            }
        }
    }
    Ok(full)
}

#[cfg(windows)]
pub(super) fn install(
    hwnd: windows_sys::Win32::Foundation::HWND,
    width: u32,
    height: u32,
    mode: Mode,
    rectangles: &[[u32; 4]],
    shapes: &[ViewportUiShape],
) -> Result<(), String> {
    let region = build(width, height, mode, rectangles, shapes)?;
    if unsafe { windows_sys::Win32::Graphics::Gdi::SetWindowRgn(hwnd, region.0, 1) } == 0 {
        // 失败时系统没有接管，RAII负责释放；成功后的句柄只能由系统管理，禁止再次DeleteObject。
        return Err(format!(
            "安装工程视口裁剪区域失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    std::mem::forget(region);
    #[cfg(feature = "validation")]
    evidence(hwnd, width, height, mode, rectangles, shapes)?;
    Ok(())
}

#[cfg(all(windows, feature = "validation"))]
pub(super) fn evidence(
    hwnd: windows_sys::Win32::Foundation::HWND,
    width: u32,
    height: u32,
    mode: Mode,
    rectangles: &[[u32; 4]],
    shapes: &[ViewportUiShape],
) -> Result<serde_json::Value, String> {
    use windows_sys::Win32::Graphics::Gdi::{ERROR, EqualRgn, GetWindowRgn, PtInRegion};
    let expected = build(width, height, mode, rectangles, shapes)?;
    let actual = Region::rectangle([0, 0, 0, 0])?;
    let kind = unsafe { GetWindowRgn(hwnd, actual.0) };
    if kind == ERROR || unsafe { EqualRgn(actual.0, expected.0) } == 0 {
        return Err("真实窗口区域与完整作者遮挡合同不一致".into());
    }
    let mut samples = Vec::with_capacity(rectangles.len() + 3);
    for point in [
        [0, 0],
        [(width.max(1) / 2) as i32, (height.max(1) / 2) as i32],
        [width.max(1) as i32 - 1, height.max(1) as i32 - 1],
    ] {
        let included = unsafe { PtInRegion(actual.0, point[0], point[1]) } != 0;
        let expected_included = unsafe { PtInRegion(expected.0, point[0], point[1]) } != 0;
        if included != expected_included {
            return Err("真实窗口视口取样与预期区域不一致".into());
        }
        samples.push(serde_json::json!({"point":point,"included":included,"expected_included":expected_included}));
    }
    for &[x, y, w, h] in rectangles {
        let point = [(x + w / 2) as i32, (y + h / 2) as i32];
        let included = unsafe { PtInRegion(actual.0, point[0], point[1]) } != 0;
        if included != (mode == Mode::Playing) {
            return Err("真实窗口未按模式让出作者控件中心点".into());
        }
        samples.push(serde_json::json!({"point":point,"included":included,"expected_included":mode == Mode::Playing}));
    }
    for point in shapes.iter().flat_map(ViewportUiShape::probe_points) {
        let included = unsafe { PtInRegion(actual.0, point[0], point[1]) } != 0;
        let expected_included = unsafe { PtInRegion(expected.0, point[0], point[1]) } != 0;
        if included != expected_included { return Err("真实窗口未保留方向轴透明轮廓".into()); }
        samples.push(serde_json::json!({"point":point,"included":included,"expected_included":expected_included}));
    }
    let parent = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::GA_PARENT,
        )
    };
    let mut parent_pid = 0;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            parent,
            &mut parent_pid,
        );
    }
    let enabled =
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd) } != 0;
    let active_rectangles: &[[u32; 4]] = if mode == Mode::Editing {
        rectangles
    } else {
        &[]
    };
    let active_shapes: &[ViewportUiShape] = if mode == Mode::Editing { shapes } else { &[] };
    Ok(
        serde_json::json!({"source":"GetWindowRgn+EqualRgn+PtInRegion","verified":true,
        "mode":mode,"hwnd":hwnd as usize as u64,"parent_hwnd":parent as usize as u64,"parent_pid":parent_pid,
        "window_enabled":enabled,"width":width,"height":height,"region_equal":true,
        "excluded_rects":active_rectangles,"requested_excluded_rects":rectangles,
        "excluded_shapes":active_shapes,"requested_excluded_shapes":shapes,"region_type":kind,"samples":samples}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn orientation_regions_keep_empty_corners_and_hollow_centers_transparent() {
        use windows_sys::Win32::Graphics::Gdi::PtInRegion;
        let shapes = [
            ViewportUiShape::Ellipse { outer: [20, 20, 40, 40], inner: Some([24, 24, 36, 36]) },
            ViewportUiShape::Ellipse { outer: [60, 20, 80, 40], inner: None },
            ViewportUiShape::Quad { points: [[20, 60], [22, 58], [42, 78], [40, 80]] },
        ];
        let region = build(100, 100, Mode::Editing, &[], &shapes).unwrap();
        for (point, visible) in [([20, 20], true), ([30, 30], true), ([30, 21], false),
            ([70, 30], false), ([60, 20], true), ([31, 69], false), ([20, 79], true)] {
            assert_eq!(unsafe { PtInRegion(region.0, point[0], point[1]) } != 0, visible, "{point:?}");
        }
        let playing = build(100, 100, Mode::Playing, &[], &shapes).unwrap();
        assert_ne!(unsafe { PtInRegion(playing.0, 30, 21) }, 0);
        assert_ne!(unsafe { PtInRegion(playing.0, 70, 30) }, 0);
        let started = std::time::Instant::now();
        for _ in 0..1000 { drop(build(100, 100, Mode::Editing, &[], &shapes).unwrap()); }
        eprintln!("XYZ固定三轮廓区域：1000次构造共{:?}", started.elapsed());
    }

    #[test]
    fn editing_regions_use_physical_half_open_rectangles_and_play_keeps_the_full_surface() {
        let rectangles = [[0, 0, 20, 10], [80, 70, 20, 30]];
        assert_eq!(
            exclusions(100, 100, Mode::Editing, &rectangles).unwrap(),
            vec![[0, 0, 20, 10], [80, 70, 100, 100]]
        );
        assert!(
            exclusions(100, 100, Mode::Playing, &rectangles)
                .unwrap()
                .is_empty()
        );
        assert!(exclusions(0, 0, Mode::Editing, &[]).unwrap().is_empty());
    }

    #[test]
    fn region_requests_reject_overflow_empty_and_outside_rectangles_even_during_play() {
        for rectangle in [
            [0, 0, 0, 1],
            [0, 0, 1, 0],
            [99, 0, 2, 1],
            [0, 99, 1, 2],
            [u32::MAX, 0, 2, 1],
        ] {
            assert!(exclusions(100, 100, Mode::Editing, &[rectangle]).is_err());
            assert!(exclusions(100, 100, Mode::Playing, &[rectangle]).is_err());
        }
        assert!(exclusions(100, 100, Mode::Editing, &[[0, 0, 1, 1]; 33]).is_err());
        assert!(exclusions(u32::MAX, 100, Mode::Playing, &[]).is_err());
    }
}
