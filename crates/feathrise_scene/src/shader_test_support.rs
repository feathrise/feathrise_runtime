//! 测试端以最小宿主桩代替 Bevy 模块，并用正式 WESL 条件编译生成 WGSL。
pub(crate) fn preprocess(source: &str, defines: &[&str], material_group: u32) -> String {
    let mut module = source.parse::<wesl::syntax::TranslationUnit>().expect("生产 WESL 应可解析");
    let mut features = wesl::Features::new();
    for name in defines {
        features.set(name, true);
    }
    wesl::pass::condcomp(&mut module, &features).expect("展开当前管线变体");
    module.imports.clear();
    module
        .to_string()
        .replace("constants::MATERIAL_BIND_GROUP", &material_group.to_string())
}
