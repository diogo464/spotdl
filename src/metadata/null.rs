use std::time::Duration;

use super::MetadataCache;

pub struct NullMetadataCache;

impl MetadataCache for NullMetadataCache {
    fn store(&self, _key: &str, _value: &[u8], _ttl: Duration) -> std::io::Result<()> {
        Ok(())
    }

    fn load(&self, _key: &str) -> std::io::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}
