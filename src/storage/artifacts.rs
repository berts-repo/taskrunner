//! Content-addressed artifact store under <state root>/artifacts/. Files are
//! immutable once stored; metadata and links live in the event log and index,
//! not here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifact {
    pub sha256: String,
    pub size_bytes: u64,
    pub locator: String,
}

pub struct ArtifactStore {
    pub root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: &Path) -> ArtifactStore {
        ArtifactStore { root: root.to_path_buf() }
    }

    pub fn store(&self, content: &[u8]) -> io::Result<StoredArtifact> {
        let sha256 = format!("{:x}", Sha256::digest(content));
        let locator = format!("{}/{}", &sha256[..2], sha256);
        let path = self.path(&locator);
        if !path.exists() {
            let dir = path.parent().expect("locator has a directory part");
            fs::create_dir_all(dir)?;
            // Write-then-rename so a crash never leaves a partial blob at the
            // content-addressed path.
            let tmp = dir.join(format!(".tmp-{}", ulid::Ulid::new()));
            fs::write(&tmp, content)?;
            fs::rename(&tmp, &path)?;
        }
        Ok(StoredArtifact { sha256, size_bytes: content.len() as u64, locator })
    }

    pub fn read(&self, locator: &str) -> io::Result<Vec<u8>> {
        fs::read(self.path(locator))
    }

    pub fn path(&self, locator: &str) -> PathBuf {
        self.root.join(locator)
    }
}
