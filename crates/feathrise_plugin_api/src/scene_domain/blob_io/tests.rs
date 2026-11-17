//! 两端共用传输的内容身份回归：坏字节和越界描述不能被采用。
use super::*;
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("feathrise-blob-{}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(), next_generation()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
#[test]
fn content_address_is_stable_and_corruption_is_rejected() {
    let fixture = Fixture::new();
    let blob = write_blob(&fixture.0, "example.domain", b"abc").unwrap();
    assert_eq!(blob.path, format!("target/feathrise/native-domain/example.domain/{}.json", blob.sha256));
    assert!(!fixture.0.join(".feathrise").exists());
    assert_eq!(blob.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(read_blob(&fixture.0, &blob).unwrap(), b"abc");
    assert_eq!(write_blob(&fixture.0, "example.domain", b"abc").unwrap(), blob);
    fs::write(fixture.0.join(&blob.path), b"abd").unwrap();
    assert!(read_blob(&fixture.0, &blob).is_err());
    assert!(write_blob(&fixture.0, "example.domain", b"abc").is_err());
}
#[test]
fn foreign_paths_and_invalid_domain_never_create_output() {
    let fixture = Fixture::new();
    assert!(write_blob(&fixture.0, "../escape", b"{}").is_err());
    assert!(write_blob(&fixture.0, ".", b"{}").is_err());
    assert!(write_blob(&fixture.0, "..", b"{}").is_err());
    assert!(fs::read_dir(&fixture.0).unwrap().next().is_none());
    let mut blob = write_blob(&fixture.0, "example.domain", b"{}").unwrap();
    blob.path = "../escape.json".into();
    assert!(read_blob(&fixture.0, &blob).is_err());
}
#[test]
fn legacy_cache_is_read_only_and_clean_target_can_be_recreated() {
    let fixture = Fixture::new();
    let body = br#"{"points":[]}"#;
    let hash = bytes_hash(body);
    let legacy = DomainBlob {
        path: format!(".feathrise/native-domain/example.domain/{hash}.json"),
        sha256: hash,
        bytes: body.len() as u64,
    };
    let legacy_path = fixture.0.join(&legacy.path);
    fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
    fs::write(&legacy_path, body).unwrap();
    assert_eq!(read_blob(&fixture.0, &legacy).unwrap(), body);

    let current = write_blob(&fixture.0, "example.domain", body).unwrap();
    assert_eq!(current.sha256, legacy.sha256);
    assert_ne!(current.path, legacy.path);
    fs::remove_dir_all(fixture.0.join("target")).unwrap();
    assert!(read_blob(&fixture.0, &current).is_err());
    assert_eq!(write_blob(&fixture.0, "example.domain", body).unwrap(), current);
    assert_eq!(read_blob(&fixture.0, &current).unwrap(), body);
    assert_eq!(fs::read(legacy_path).unwrap(), body);
}
#[cfg(unix)]
#[test]
fn redirected_target_does_not_receive_cache_writes() {
    let fixture = Fixture::new();
    let outside_cache = fixture.0.join("assets");
    fs::create_dir(&outside_cache).unwrap();
    std::os::unix::fs::symlink(&outside_cache, fixture.0.join("target")).unwrap();
    assert!(write_blob(&fixture.0, "example.domain", b"abc").is_err());
    assert!(fs::read_dir(outside_cache).unwrap().next().is_none());
}
