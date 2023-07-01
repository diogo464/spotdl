use std::hash::{BuildHasher, Hasher};
use std::io::Result;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::metadata::{Album, Artist, Track};
use crate::{Resource, ResourceId, SpotifyId};

use super::MetadataFetcher;

#[derive(Debug, Serialize, Deserialize)]
struct CacheItem<T> {
    created: u64,
    value: T,
}

#[derive(Debug, Clone)]
pub struct FsCacheMetadataFetcherParams {
    pub directory: PathBuf,
    pub artist_ttl: Duration,
    pub album_ttl: Duration,
    pub track_ttl: Duration,
    pub playlist_ttl: Duration,
}

#[derive(Debug)]
pub struct FsCacheMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    fetcher: F,
    params: FsCacheMetadataFetcherParams,
}

impl<F> FsCacheMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    const BUCKETS: u64 = 256;
    const RANDOM_STATE: ahash::RandomState = ahash::RandomState::with_seeds(0, 0, 0, 0);

    pub async fn new(fetcher: F, directory: PathBuf) -> Result<Self> {
        Self::with(
            fetcher,
            FsCacheMetadataFetcherParams {
                directory,
                artist_ttl: Duration::from_secs(60 * 60 * 24),
                album_ttl: Duration::MAX,
                track_ttl: Duration::MAX,
                playlist_ttl: Duration::from_secs(60 * 60 * 1),
            },
        )
        .await
    }

    pub async fn with(fetcher: F, params: FsCacheMetadataFetcherParams) -> Result<Self> {
        tokio::fs::create_dir_all(&params.directory).await?;
        Ok(Self { fetcher, params })
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

    async fn load<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
    {
        let path = self.key_path(key);
        let contents = tokio::fs::read_to_string(path).await?;
        let value = serde_json::from_str(&contents)?;
        tracing::trace!("loaded {} from cache", key);
        Ok(Some(value))
    }

    fn key_bucket(key: &str) -> u64 {
        let mut hasher = Self::RANDOM_STATE.build_hasher();
        hasher.write(key.as_bytes());
        hasher.finish() % Self::BUCKETS
    }

    fn key_path(&self, key: &str) -> PathBuf {
        let bucket = Self::key_bucket(key);
        self.params.directory.join(bucket.to_string()).join(key)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("failed to get current time")
            .as_secs()
    }
}

#[async_trait::async_trait]
impl<F> MetadataFetcher for FsCacheMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    async fn get_artist(&self, id: SpotifyId) -> Result<Artist> {
        let key = ResourceId::from((Resource::Artist, id)).to_string();
        if let Ok(Some(artist)) = self.load(&key).await {
            return Ok(artist);
        }
        let artist = self.fetcher.get_artist(id).await?;
        self.store(&key, &artist).await?;
        Ok(artist)
    }

    async fn get_album(&self, id: SpotifyId) -> Result<Album> {
        let key = ResourceId::from((Resource::Album, id)).to_string();
        if let Ok(Some(album)) = self.load(&key).await {
            return Ok(album);
        }
        let album = self.fetcher.get_album(id).await?;
        self.store(&key, &album).await?;
        Ok(album)
    }

    async fn get_track(&self, id: SpotifyId) -> Result<Track> {
        let key = ResourceId::from((Resource::Track, id)).to_string();
        if let Ok(Some(track)) = self.load(&key).await {
            return Ok(track);
        }
        let track = self.fetcher.get_track(id).await?;
        self.store(&key, &track).await?;
        Ok(track)
    }

    async fn get_playlist(&self, id: SpotifyId) -> std::io::Result<crate::metadata::Playlist> {
        let key = ResourceId::from((Resource::Playlist, id)).to_string();
        if let Ok(Some(playlist)) = self.load(&key).await {
            return Ok(playlist);
        }
        let playlist = self.fetcher.get_playlist(id).await?;
        self.store(&key, &playlist).await?;
        Ok(playlist)
    }

    async fn get_image(&self, url: &str) -> Result<Bytes> {
        let key = format!("image-{}", url);
        if let Ok(Some(image)) = self.load(&key).await {
            return Ok(image);
        }
        let image = self.fetcher.get_image(url).await?;
        self.store(&key, &image).await?;
        Ok(image)
    }
}