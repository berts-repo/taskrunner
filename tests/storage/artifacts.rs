use sha2::{Digest, Sha256};
use taskrunner::storage::artifacts::ArtifactStore;

#[test]
fn stores_and_reads_back_content_by_locator() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let stored = store.store(b"hello artifacts").unwrap();

    assert_eq!(stored.sha256, format!("{:x}", Sha256::digest(b"hello artifacts")));
    assert_eq!(stored.size_bytes, "hello artifacts".len() as u64);
    assert_eq!(stored.locator, format!("{}/{}", &stored.sha256[..2], stored.sha256));
    assert_eq!(store.read(&stored.locator).unwrap(), b"hello artifacts");
}

#[test]
fn deduplicates_identical_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path());
    let a = store.store(b"same bytes").unwrap();
    let b = store.store(b"same bytes").unwrap();
    assert_eq!(a, b);
}
