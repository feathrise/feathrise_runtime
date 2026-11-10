//! 可选热补丁消息的身份、路径与整数回归；旧能力要求及协议版本保持。
use crate::*;

fn identity() -> SessionIdentity { SessionIdentity { project_id: "patch-fixture".into(), session: 7, generation: 3 } }
fn request(artifact: &str) -> Envelope {
    Envelope::new(identity(), 1, Message::ApplyCodePatch { patch_id: 9, source_generation: 11,
        base_build: "a".repeat(64), artifact: artifact.into(), sha256: "b".repeat(64) })
}

#[test]
fn optional_patch_request_roundtrips_without_new_mandatory_capabilities() {
    assert_eq!(PROTOCOL_VERSION, 2);
    let request = request("patches/9/package.json");
    request.validate(&Limits::default()).unwrap();
    let mut encoded = Vec::new(); write_frame(&mut encoded, &request, &Limits::default()).unwrap();
    let decoded = read_frame(&mut encoded.as_slice(), &Limits::default()).unwrap().unwrap();
    assert_eq!(decoded.message, request.message);
    let hello = Envelope::new(identity(), 1, Message::Hello { build_identity: "old-sdk".into(), capabilities: vec!["resident-world-v1".into()] });
    SessionGuard::verify_handshake(&hello, "old-sdk", &["resident-world-v1"]).unwrap();
}

#[test]
fn absolute_parent_empty_and_control_patch_paths_are_rejected() {
    for path in ["/package.json", "C:/package.json", "../package.json", "a/../p.json", "a//p.json", "a\\..\\p.json", "a\0p.json"] {
        assert!(request(path).validate(&Limits::default()).is_err(), "{path:?}");
    }
}

#[test]
fn patch_results_distinguish_real_success_from_failure_and_keep_u64_identity() {
    let result = |applied, epoch, reason| Envelope::new(identity(), 1, Message::CodePatchResult {
        patch_id: u64::MAX, source_generation: 9007199254740993, base_build: "a".repeat(64),
        pid: 42, world_id: u64::MAX, epoch, applied, reason,
    });
    let success = result(true, 1, None);
    success.validate(&Limits::default()).unwrap();
    let bytes = serde_json::to_vec(&success).unwrap();
    assert_eq!(serde_json::from_slice::<Envelope>(&bytes).unwrap().message, success.message);
    result(false, 0, Some("旧实例继续运行".into())).validate(&Limits::default()).unwrap();
    assert!(result(true, 0, None).validate(&Limits::default()).is_err());
    assert!(result(false, 0, None).validate(&Limits::default()).is_err());
    assert!(result(true, 1, Some("错误".into())).validate(&Limits::default()).is_err());
}
