use std::hash::{BuildHasher, Hasher};
use std::io::Result;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::metadata::{Album, Artist, Image, Track};
use crate::{Resource, ResourceId, SpotifyId};

use super::MetadataFetcher;

#[derive(Debug, Serialize, Deserialize)]
struct CacheItem<T> {
    created: u64,
    value: T,
}

#[derive(Debug)]
pub struct FsCache {
    directory: PathBuf,
}

impl FsCache {
    const BUCKETS: u64 = 256;
    const RANDOM_STATE: ahash::RandomState = ahash::RandomState::with_seeds(0, 0, 0, 0);

    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// List all keys in the cache.
    pub async fn list(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for bucket in 0..Self::BUCKETS {
            let bucket_path = self.directory.join(bucket.to_string());
            if !bucket_path.exists() {
                continue;
            }
            let mut dir = tokio::fs::read_dir(bucket_path).await?;
            while let Some(entry) = dir.next_entry().await? {
                if entry.path().is_dir() {
                    continue;
                }
                let key = entry
                    .path()
                    .file_name()
                    .expect("all cache keys should have a file name")
                    .to_str()
                    .expect("all cache keys should be valid UTF-8")
                    .to_string();
                keys.push(key);
            }
        }
        Ok(keys)
    }

    /// Remove a key from the cache.
    pub async fn remove(&self, key: &str) -> Result<()> {
        let bucket = self.key_path(key);
        match tokio::fs::remove_file(bucket).await {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => todo!(),
        }
    }

    pub async fn clear(&self) -> Result<()> {
        tokio::fs::remove_dir_all(&self.directory).await?;
        Ok(())
    }

    async fn store<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Clone + Send + 'static,
    {
        let path = self.key_path(key);
        let cache_item = CacheItem {
            created: Self::now(),
            value,
        };
        let contents = serde_json::to_string_pretty(&cache_item)?;
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(path, &contents).await?;
        tracing::trace!("stored {} to cache", key);
        Ok(())
    }

    async fn load<T>(&self, key: &str, ttl: Duration) -> Result<Option<T>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
    {
        let path = self.key_path(key);
        let contents = tokio::fs::read_to_string(&path).await?;
        let cache_item: CacheItem<T> = serde_json::from_str(&contents)?;
        if Self::now() - cache_item.created > ttl.as_secs() {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
        tracing::trace!("loaded {} from cache", key);
        Ok(Some(cache_item.value))
    }

    fn key_bucket(key: &str) -> u64 {
        let mut hasher = Self::RANDOM_STATE.build_hasher();
        hasher.write(key.as_bytes());
        hasher.finish() % Self::BUCKETS
    }

    fn key_path(&self, key: &str) -> PathBuf {
        let bucket = Self::key_bucket(key);
        self.directory.join(bucket.to_string()).join(key)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("failed to get current time")
            .as_secs()
    }
}

#[derive(Debug, Clone)]
pub struct FsCacheMetadataFetcherParams {
    pub artist_ttl: Duration,
    pub album_ttl: Duration,
    pub track_ttl: Duration,
    pub playlist_ttl: Duration,
    pub image_ttl: Duration,
}

impl Default for FsCacheMetadataFetcherParams {
    fn default() -> Self {
        Self {
            artist_ttl: Duration::from_secs(60 * 60 * 24),
            album_ttl: Duration::MAX,
            track_ttl: Duration::MAX,
            playlist_ttl: Duration::from_secs(60 * 60),
            image_ttl: Duration::MAX,
        }
    }
}

#[derive(Debug)]
pub struct FsCacheMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    fetcher: F,
    cache: FsCache,
    params: FsCacheMetadataFetcherParams,
}

impl<F> FsCacheMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    const ENGINE: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        base64::engine::GeneralPurposeConfig::new(),
    );

    pub async fn new(fetcher: F, directory: PathBuf) -> Result<Self> {
        Self::with(fetcher, directory, Default::default()).await
    }

    pub async fn with(
        fetcher: F,
        directory: PathBuf,
        params: FsCacheMetadataFetcherParams,
    ) -> Result<Self> {
        Ok(Self {
            fetcher,
            cache: FsCache::new(directory),
            params,
        })
    }
}

#[async_trait::async_trait]
impl<F> MetadataFetcher for FsCacheMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    async fn get_artist(&self, id: SpotifyId) -> Result<Artist> {
        let key = ResourceId::from((Resource::Artist, id)).to_string();
        if let Ok(Some(artist)) = self.cache.load(&key, self.params.artist_ttl).await {
            return Ok(artist);
        }
        let artist = self.fetcher.get_artist(id).await?;
        self.cache.store(&key, &artist).await?;
        Ok(artist)
    }

    async fn get_album(&self, id: SpotifyId) -> Result<Album> {
        let key = ResourceId::from((Resource::Album, id)).to_string();
        if let Ok(Some(album)) = self.cache.load(&key, self.params.album_ttl).await {
            return Ok(album);
        }
        let album = self.fetcher.get_album(id).await?;
        self.cache.store(&key, &album).await?;
        Ok(album)
    }

    async fn get_track(&self, id: SpotifyId) -> Result<Track> {
        let key = ResourceId::from((Resource::Track, id)).to_string();
        if let Ok(Some(track)) = self.cache.load(&key, self.params.track_ttl).await {
            return Ok(track);
        }
        let track = self.fetcher.get_track(id).await?;
        self.cache.store(&key, &track).await?;
        Ok(track)
    }

    async fn get_playlist(&self, id: SpotifyId) -> std::io::Result<crate::metadata::Playlist> {
        let key = ResourceId::from((Resource::Playlist, id)).to_string();
        if let Ok(Some(playlist)) = self.cache.load(&key, self.params.playlist_ttl).await {
            return Ok(playlist);
        }
        let playlist = self.fetcher.get_playlist(id).await?;
        self.cache.store(&key, &playlist).await?;
        Ok(playlist)
    }

    async fn get_image(&self, url: &str) -> Result<Image> {
        use base64::engine::Engine;

        let encoded = Self::ENGINE.encode(url);
        let key = format!("image:{}", encoded);
        if let Ok(Some(image)) = self.cache.load(&key, self.params.image_ttl).await {
            return Ok(image);
        }
        let image = self.fetcher.get_image(url).await?;
        self.cache.store(&key, &image).await?;
        Ok(image)
    }
}
