//! 元数据损坏、真实格式/架构校验与平台拒绝的无执行回归。
use super::*;

fn metadata() -> PluginMetadata {
    PluginMetadata {
        format: 1,
        id: "fixture.author".into(),
        tool: "夹具作者".into(),
        version: "1.2.3".into(),
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        abi_major: crate::ABI_MAJOR,
        runtime_dependencies: vec![],
        runtime: None,
    }
}

#[test]
fn envelope_round_trip_rejects_truncation_and_unknown_format() {
    let expected = metadata();
    let bytes = encode_plugin_metadata(&expected).unwrap();
    assert_eq!(decode_plugin_metadata(&bytes).unwrap(), expected);
    assert!(decode_plugin_metadata(&bytes[..bytes.len() - 1]).is_err());
    let mut bad = bytes.clone();
    bad[16] = 2;
    assert!(decode_plugin_metadata(&bad).is_err());
    let mut padded = bytes;
    padded.extend([0; 511]);
    assert_eq!(decode_plugin_metadata(&padded).unwrap(), expected);
    *padded.last_mut().unwrap() = 1;
    assert!(decode_plugin_metadata(&padded).is_err());
}

#[test]
fn invalid_identity_and_foreign_platform_are_rejected() {
    let mut expected = metadata();
    expected.id = "../escape".into();
    assert!(encode_plugin_metadata(&expected).is_err());
    expected = metadata();
    expected.target_os = if cfg!(target_os = "windows") {
        "linux"
    } else {
        "windows"
    }
    .into();
    assert!(validate_current_platform(&expected).is_err());
}

#[test]
fn advertised_oversize_and_duplicate_json_fields_are_rejected() {
    let mut bytes = encode_plugin_metadata(&metadata()).unwrap();
    bytes[20..24].copy_from_slice(&((MAX_METADATA_JSON_BYTES + 1) as u32).to_le_bytes());
    assert!(decode_plugin_metadata(&bytes).is_err());
    let json = serde_json::to_string(&metadata()).unwrap();
    let duplicated = format!("{{\"format\":1,{}", &json[1..]);
    let mut bytes = METADATA_MAGIC.to_vec();
    bytes.extend(METADATA_FORMAT.to_le_bytes());
    bytes.extend((duplicated.len() as u32).to_le_bytes());
    bytes.extend(duplicated.as_bytes());
    assert!(decode_plugin_metadata(&bytes).is_err());
}

#[cfg(feature = "reader-test-support")]
#[test]
fn all_three_binary_formats_and_both_cpus_are_read_statically() {
    let root = std::env::temp_dir().join(format!("feathrise-meta-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for (os, suffix) in [("windows", "dll"), ("linux", "so"), ("macos", "dylib")] {
        for arch in ["x86_64", "aarch64"] {
            let mut expected = metadata();
            expected.target_os = os.into();
            expected.target_arch = arch.into();
            let file = root.join(format!("{os}-{arch}.{suffix}"));
            test_support::write_metadata_fixture(&file, &expected).unwrap();
            assert_eq!(read_plugin_metadata(&file).unwrap(), expected);
            let wrong = root.join(format!("{os}-{arch}.wrong"));
            std::fs::copy(&file, &wrong).unwrap();
            assert!(read_plugin_metadata(&wrong).is_err());
        }
    }
    // 仅回收本测试的精确临时目录，不触碰构建产物。
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "reader-test-support")]
#[test]
fn binary_headers_must_match_metadata_and_stay_within_bounds() {
    let root = std::env::temp_dir().join(format!("feathrise-meta-bounds-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("fixture.dll");
    let mut expected = metadata();
    expected.target_os = "windows".into();
    expected.target_arch = "x86_64".into();
    test_support::write_metadata_fixture(&file, &expected).unwrap();
    let original = std::fs::read(&file).unwrap();
    let mut bytes = original.clone();
    bytes[132..134].copy_from_slice(&0xaa64_u16.to_le_bytes());
    std::fs::write(&file, &bytes).unwrap();
    assert!(
        read_plugin_metadata(&file)
            .unwrap_err()
            .contains("实际二进制")
    );
    let mut bytes = original.clone();
    bytes[134..136].copy_from_slice(&2_u16.to_le_bytes());
    let section = bytes[392..432].to_vec();
    bytes[432..472].copy_from_slice(&section);
    std::fs::write(&file, &bytes).unwrap();
    assert!(
        read_plugin_metadata(&file)
            .unwrap_err()
            .contains("重复元数据")
    );
    let mut bytes = original.clone();
    bytes[400..404].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&file, &bytes).unwrap();
    assert!(read_plugin_metadata(&file).is_err());
    std::fs::write(&file, &original[..512]).unwrap();
    assert!(read_plugin_metadata(&file).is_err());
    let mut bytes = original;
    bytes[392..399].copy_from_slice(b".noinfo");
    std::fs::write(&file, &bytes).unwrap();
    assert!(
        read_plugin_metadata(&file)
            .unwrap_err()
            .contains("缺少静态元数据")
    );
    std::fs::remove_dir_all(root).unwrap();
}
