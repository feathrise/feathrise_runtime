//! 场景包自己拥有的合成目录和四面体模型；单包解压后不读取其他 crate 的测试目录。

pub(crate) const VEGETATION_CATALOG_JSON: &str = include_str!("../tests/fixtures/vegetation_presets.json");

pub(crate) const VEGETATION_ASSET_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures"
);
