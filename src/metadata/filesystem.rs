use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::MetadataCache;

#[derive(Debug, Serialize, Deserialize)]
struct CacheItem {
    expires_at: u64,
    value: Vec<u8>,
}

#[derive(Debug, Clone)]
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
        let expires_at = std::time::SystemTime::now() + ttl;
        let expires_unix = expires_at
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_secs();
        let item = CacheItem {
            expires_at: expires_unix,
            value: value.to_vec(),
        };
        let path = self.base.join(key);
        if let Some(parent) = path.parent() {
            if !parent.is_dir() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = std::fs::File::create(path)?;
        serde_json::to_writer(&mut file, &item)?;
        Ok(())
    }

    fn load(&self, key: &str) -> std::io::Result<Option<Vec<u8>>> {
        let path = self.base.join(key);
        if !path.is_file() {
            tracing::trace!("cache miss: {}", key);
            return Ok(None);
        }
        let file = std::fs::File::open(&path)?;
        let item: CacheItem = serde_json::from_reader(file)?;
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_secs();
        if item.expires_at > now_unix {
            tracing::trace!("cache hit: {}", key);
            Ok(Some(item.value))
        } else {
            tracing::trace!("cache expired: {}", key);
            std::fs::remove_file(path)?;
            Ok(None)
        }
    }
}
