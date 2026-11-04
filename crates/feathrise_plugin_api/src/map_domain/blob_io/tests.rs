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
    assert!(fs::read_dir(&fixture.0).unwrap().next().is_none());
    let mut blob = write_blob(&fixture.0, "example.domain", b"{}").unwrap();
    blob.path = "../escape.json".into();
    assert!(read_blob(&fixture.0, &blob).is_err());
}
