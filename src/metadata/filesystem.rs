use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::MetadataCache;

#[derive(Debug, Serialize, Deserialize)]
struct CacheItem {
    expires_at: std::time::SystemTime,
    value: Vec<u8>,
}

pub struct FsMetadataCache {
    base: PathBuf,
}

impl FsMetadataCache {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }
}

impl MetadataCache for FsMetadataCache {
    fn store(&self, key: &str, value: &[u8], ttl: std::time::Duration) -> std::io::Result<()> {
        let item = CacheItem {
            expires_at: std::time::SystemTime::now() + ttl,
            value: value.to_vec(),
        };
        let path = self.base.join(key);
        let mut file = std::fs::File::create(path)?;
        serde_json::to_writer(&mut file, &item)?;
        Ok(())
    }

    fn load(&self, key: &str) -> std::io::Result<Option<Vec<u8>>> {
        let path = self.base.join(key);
        if !path.is_file() {
            return Ok(None);
        }
        let file = std::fs::File::open(&path)?;
        let item: CacheItem = serde_json::from_reader(file)?;
        if item.expires_at < std::time::SystemTime::now() {
            Ok(Some(item.value))
        } else {
            std::fs::remove_file(path)?;
            Ok(None)
        }
    }
}
