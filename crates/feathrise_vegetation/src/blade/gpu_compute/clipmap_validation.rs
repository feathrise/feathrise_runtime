//! Clipmap GPU 拓扑查询的 CPU 验证镜像；仅供离线测试，不进入运行时逐株路径。
use crate::components::{GpuBladeClipmapBand, GpuBladeClipmapHeight};
use bevy::prelude::*; // 与地形相同的双轴世界格距。 // 复用公共快照作为测试输入。

/// 用调用方提供的同级顶点高度验证真实显示三角面；不得用于逐株运行时贴地。
#[doc(hidden)] // API 仅用于 scene 的实际 Mesh oracle 回归。
pub fn sample_clipmap_surface_for_validation(
    height: &GpuBladeClipmapHeight,
    point: Vec2,
    mut sample: impl FnMut(Vec2, u32) -> f32,
) -> Option<f32> {
    // 不读取 ECS、纹理或地图文件。
    if point.cmplt(height.world_min).any() || point.cmpgt(height.world_max).any() {
        return None;
    } // 地图外草根禁止生成。
    if height
        .holes
        .iter()
        .take(height.hole_count.min(32) as usize)
        .any(|rect| point.cmpgt(rect.xy()).all() && point.cmplt(rect.zw()).all())
    {
        return None; // 严格内部禁草，边界仍与保留的地形一致。
    }
    let mut nearest:Option<f32>=None;
    for band in height.bands.iter().take(height.band_count as usize) {
        // 作者最多十六层，按内到外找到真实覆盖环。
        let step = height.cell_size * 2.0_f32.powi(band.geometry_mip as i32); // 当前几何格距。
        let local = (point - center(height, band.geometry_mip)) / step; // 当前粗层常规格坐标。
        if local.abs().cmpgt(band.outer_half_cells.as_vec2()).any() && !height.stitches.active() {
            continue;
        } // 当前外矩形没有覆盖查询点。
        for candidate in 0..if height.stitches.active(){16}else{1} {
        let inverse=height.stitches.candidate(point,height.world_min,height.world_max,step,center(height,band.geometry_mip),candidate);
        if candidate%4>0&&(inverse.x-point.x).abs()<0.000001||candidate/4>0&&(inverse.y-point.y).abs()<0.000001 {continue;}
        let cell = ((inverse-center(height,band.geometry_mip))/step).floor().as_ivec2();
        let offsets = [
            IVec2::ZERO,
            IVec2::new(-1, -1),
            IVec2::new(0, -1),
            IVec2::new(1, -1),
            IVec2::new(-1, 0),
            IVec2::new(1, 0),
            IVec2::new(-1, 1),
            IVec2::new(0, 1),
            IVec2::new(1, 1),
        ]; // 与 GPU 相同的固定局部邻域。
        for offset in offsets {
            // 只有过渡带或边界才会走后续邻格。
            let q = cell + offset; // 候选模板单元。
            if let Some(result) = sample_cell(height, point, q, *band, &mut sample) {
                if !height.stitches.active(){return Some(result);}
                nearest=Some(nearest.map_or(result,|old|old.max(result)));
            } // 命中即返回。
        } // 完成有限单元邻域检查。
        }
    } // 完成几何级定位。
    nearest // 边角收敛可让多个环带投影重叠，草根与深度缓冲一样取最高的有效正面。
} // 完成 CPU 诊断镜像。

fn center(height: &GpuBladeClipmapHeight, level: u32) -> Vec2 {
    // 与地形顶点 Shader 同式吸附。
    let step = height.cell_size * 2.0_f32.powi(level as i32); // 各级各自格距。
    height.world_min + (((height.fine_center - height.world_min) / step) + Vec2::splat(0.5)).floor() * step
    // 观察中心可在源边界外；必须与地形共享全局相机，不能把草根细环拉回远处对象。
} // 完成层中心。

fn vertex(
    height: &GpuBladeClipmapHeight,
    local: Vec2,
    geometry_mip: u32,
    fine: bool,
) -> (Vec2, u32) {
    // 返回 X/Z 与几何高度级。
    let sample_level = geometry_mip.saturating_sub(u32::from(fine)); // 内缘采上一细级。
    (
        height.stitches.position(local * height.cell_size * 2.0_f32.powi(geometry_mip as i32) + center(height, sample_level),height.world_min,height.world_max),
        sample_level,
    ) // 当前模板缩放与采样中心各自保留。
} // 完成模板顶点变换。

fn triangle(
    height: &GpuBladeClipmapHeight,
    point: Vec2,
    vertices: [(Vec2, u32); 3],
    sample: &mut impl FnMut(Vec2, u32) -> f32,
) -> Option<f32> {
    // 先选真实面，再采三个高度。
    let [a, b, c] = vertices; // 保留独立采样级。
    let ab = b.0 - a.0; // 第一条边。
    let ac = c.0 - a.0; // 第二条边。
    let ap = point - a.0; // 查询点相对首角。
    let det = ab.perp_dot(ac); // 有向面积。
    if det.abs() < 0.00000001 {
        return None;
    } // 半格中心偏移可压扁边缘面。
    if height.stitches.active()&&det>=0.0{return None;}
    let u = ap.perp_dot(ac) / det; // 第二角权重。
    let v = ab.perp_dot(ap) / det; // 第三角权重。
    if u < -0.000001 || v < -0.000001 || u + v > 1.000001 {
        return None;
    } // 面外不调用采高闭包。
    for (p, geometry_mip) in vertices {
        if height.stitches.boundary_height(p,height.world_min,height.world_max).is_some(){continue;}
        // 只检查已选中的真实三角，不遍历全图页表。
        if p.cmplt(height.world_min).any() || p.cmpgt(height.world_max).any() {
            continue;
        } // 地图外顶点仍是地形正式零高度。
        let mip = geometry_mip.min(height.max_mip); // 必须与GPU实际采样父页相同。
        let scale = 2.0_f32.powi(mip as i32); // 当前页的基础格覆盖倍率。
        let logical = ((p - height.world_min) / height.cell_size)
            .clamp(Vec2::ZERO, height.logical_cells.as_vec2())
            / scale; // 与地形同源逻辑坐标。
        let counts = (height.logical_cells.as_vec2() / (256.0 * scale))
            .ceil()
            .as_uvec2(); // 最远边界归最后一页。
        let tile = (logical / 256.0)
            .floor()
            .as_uvec2()
            .min(counts - UVec2::ONE); // 页内/页边与正式GPU同规则。
        let key = (tile.x | (tile.y << 16)) + 1; // 对齐当前缺页掩码编码。
        if height.missing_pages[mip as usize].to_array().contains(&key) {
            return None;
        } // 同内容局部缺页只拒绝受影响的候选。
    } // 其余面继续按已采用atlas采样。
    let mut get = |(p, mip): (Vec2, u32)| {
        if p.cmplt(height.world_min).any() || p.cmpgt(height.world_max).any() {
            0.0
        } else {
            height.stitches.boundary_height(p,height.world_min,height.world_max).unwrap_or_else(||height.stitches.height(p,sample(p,mip.min(height.max_mip)),height.world_min,height.world_max))
        }
    }; // 地图外顶点与正式材质同为零。
    Some(get(a) * (1.0 - u - v) + get(b) * u + get(c) * v) // 真实三角插值而非双线性地表。
} // 完成三角查询。

fn sample_cell(
    height: &GpuBladeClipmapHeight,
    point: Vec2,
    cell: IVec2,
    band: GpuBladeClipmapBand,
    sample: &mut impl FnMut(Vec2, u32) -> f32,
) -> Option<f32> {
    // 可变矩形环带拓扑镜像。
    let outer = band.outer_half_cells.as_ivec2(); // 当前层两轴外边界。
    let inner = band.inner_half_cells.as_ivec2(); // 当前层两轴洞口。
    if cell.cmplt(-outer).any() || cell.cmpge(outer).any() {
        return None;
    } // 不存在的单元。
    let in_x = (-inner.x..inner.x).contains(&cell.x); // 洞内 X。
    let in_z = (-inner.y..inner.y).contains(&cell.y); // 洞内 Z。
    if inner.cmpgt(IVec2::ZERO).all() && in_x && in_z {
        return None;
    } // 细层已覆盖洞口。
    let mut side = None; // 默认普通两三角。
    if inner.cmpgt(IVec2::ZERO).all() {
        // 只有外层才有接缝。
        if cell.x == inner.x && in_z {
            side = Some(0);
        }
        // 左拆边。
        else if cell.y + 1 == -inner.y && in_x {
            side = Some(1);
        }
        // 上拆边。
        else if cell.x + 1 == -inner.x && in_z {
            side = Some(2);
        }
        // 右拆边。
        else if cell.y == inner.y && in_x {
            side = Some(3);
        } // 下拆边。
    } // 完成接缝方向。
    let q = cell.as_vec2(); // 单元局部原点。
    let locals = [q, q + Vec2::Y, q + Vec2::ONE, q + Vec2::X]; // 地形多边形顺序。
    let corners = locals.map(|p| {
        // 整点使用内缘标记。
        let edge = (p.x.abs() == inner.x as f32 && p.y.abs() <= inner.y as f32)
            || (p.y.abs() == inner.y as f32 && p.x.abs() <= inner.x as f32); // 包括四个内洞角。
        vertex(
            height,
            p,
            band.geometry_mip,
            inner.cmpgt(IVec2::ZERO).all() && edge,
        ) // 对齐细层中心。
    }); // 完成四角坐标生成。
    if let Some(side) = side {
        let mut nearest:Option<f32>=None;
        // 与地形 split_cell 同样建立五角扇形。
        let middle = vertex(height, q + Vec2::splat(0.5), band.geometry_mip, false); // 粗中心使用当前级高度。
        let midpoints = [
            Vec2::new(0.0, 0.5),
            Vec2::new(0.5, 1.0),
            Vec2::new(1.0, 0.5),
            Vec2::new(0.5, 0.0),
        ]; // 四种拆边中点。
        let seam = vertex(height, q + midpoints[side], band.geometry_mip, true); // 细边中点使用上一层。
        for edge in 0..4 {
            // 顺时针遍历四条边。
            let a = corners[edge]; // 当前角。
            let b = corners[(edge + 1) % 4]; // 下一角。
            if edge == side {
                // 内边拆两面。
                if let Some(y) = triangle(height, point, [middle, a, seam], sample) {
                    if !height.stitches.active(){return Some(y);}nearest=Some(nearest.map_or(y,|old|old.max(y)));
                } // 第一半面。
                if let Some(y) = triangle(height, point, [middle, seam, b], sample) {
                    if !height.stitches.active(){return Some(y);}nearest=Some(nearest.map_or(y,|old|old.max(y)));
                } // 第二半面。
            } else if let Some(y) = triangle(height, point, [middle, a, b], sample) {
                if !height.stitches.active(){return Some(y);}nearest=Some(nearest.map_or(y,|old|old.max(y)));
            } // 其余整面。
        } // 完成五个实际面查询。
        nearest // 此变形单元未覆盖时仍返回None。
    } else {
        // 常规格反对角线。
        let first=triangle(height,point,[corners[0],corners[1],corners[3]],sample);
        if !height.stitches.active(){return first.or_else(||triangle(height,point,[corners[3],corners[1],corners[2]],sample));}
        let second=triangle(height,point,[corners[3],corners[1],corners[2]],sample);
        match (first,second){(Some(a),Some(b))=>Some(a.max(b)),(a,b)=>a.or(b)}
    } // 完成单元分支。
} // 完成拓扑 CPU 镜像。
