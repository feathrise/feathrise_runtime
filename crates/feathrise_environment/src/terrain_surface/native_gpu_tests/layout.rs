//! 项目地表shader的Function布局合同：计算仅用标量、向量、矩阵，标准PBR宿主单独保留。
pub(super) fn assert_contract(module: &wgpu::naga::Module) {
    for (_, function) in module.functions.iter() {
        assert_locals(module, function, false);
        if matches!(
            function.name.as_deref(),
            Some("sample_surface_weights" | "normalize_surface_weights")
        ) {
            assert_eight_weight_matrix(module, function.result.as_ref().unwrap().ty);
        }
        if function.name.as_deref() == Some("blend_surface") {
            assert_eight_weight_matrix(module, function.arguments[0].ty);
        }
    }
    for entry in &module.entry_points {
        // 唯一豁免是生产片元沿用的Bevy输入/PBR局部；标准输出也必须SSA构造。
        assert_locals(module, &entry.function, entry.name == "fragment");
    }
    let (_, weights) = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("IndexedSurfaceWeights"))
        .expect("项目四槽返回结构必须存在");
    let wgpu::naga::TypeInner::Struct { members, .. } = &weights.inner else {
        panic!("四槽权重必须由两个向量返回");
    };
    assert_eq!(members.len(), 2);
    for member in members {
        assert!(matches!(
            module.types[member.ty].inner,
            wgpu::naga::TypeInner::Vector {
                size: wgpu::naga::VectorSize::Quad,
                ..
            }
        )); // 动态读取返回槽也不能让后端生成Function数组临时变量。
    }
}

fn assert_locals(
    module: &wgpu::naga::Module,
    function: &wgpu::naga::Function,
    standard_fragment: bool,
) {
    for (_, variable) in function.local_variables.iter() {
        let ty = &module.types[variable.ty];
        let standard_pbr = standard_fragment
            && matches!(
                ty.name.as_deref(),
                Some("VertexOutput" | "PbrInput")
            );
        assert!(
            standard_pbr
                || !matches!(
                    ty.inner,
                    wgpu::naga::TypeInner::Array { .. } | wgpu::naga::TypeInner::Struct { .. }
                ),
            "{} 的项目Function局部 {:?} 不得携带Offset/ArrayStride结构布局",
            function.name.as_deref().unwrap_or("entry"),
            variable.name
        );
    }
}

fn assert_eight_weight_matrix(
    module: &wgpu::naga::Module,
    ty: wgpu::naga::Handle<wgpu::naga::Type>,
) {
    assert!(matches!(
        module.types[ty].inner,
        wgpu::naga::TypeInner::Matrix {
            columns: wgpu::naga::VectorSize::Bi,
            rows: wgpu::naga::VectorSize::Quad,
            ..
        }
    )); // 两列四通道完整保留八层，无Function数组或接口布局结构。
}
