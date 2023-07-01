#![feature(async_fn_in_trait)]
use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use bytes::Bytes;
use serde::{de::DeserializeOwned, Serialize};
use spotdl::{
    metadata::{Album, Artist, Track},
    Resource, ResourceId, SpotifyId,
};

trait Cache {
    async fn store<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Clone + Send + 'static;
    async fn load<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned + Clone + Send + 'static;
}

#[derive(Debug)]
struct FsCache(PathBuf);

impl FsCache {
    pub async fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        tokio::fs::create_dir_all(&path).await?;
        Ok(Self(path))
    }
}

impl Cache for FsCache {
    async fn store<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Clone + Send + 'static,
    {
        let path = self.0.join(key);
        let contents = serde_json::to_vec(value)?;
        tokio::fs::write(path, contents).await?;
        Ok(())
    }

    async fn load<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
    {
        let path = self.0.join(key);
        let contents = tokio::fs::read(path).await?;
        let value = serde_json::from_slice(&contents)?;
        Ok(Some(value))
    }
}

trait MetadataFetcher {
    async fn get_artist(&self, id: SpotifyId) -> Result<Artist>;
    async fn get_album(&self, id: SpotifyId) -> Result<Album>;
    async fn get_track(&self, id: SpotifyId) -> Result<Track>;
    async fn get_image(&self, url: &str) -> Result<Bytes>;
}

struct DummyMetadataFetcher;

impl MetadataFetcher for DummyMetadataFetcher {
    async fn get_artist(&self, id: SpotifyId) -> Result<Artist> {
        Ok(Artist {
            rid: ResourceId::from((Resource::Artist, id)),
            name: format!("artist-{}", id),
            albums: vec![],
            singles: vec![],
            compilations: vec![],
            appears_on: vec![],
            genres: vec![],
        })
    }

    async fn get_album(&self, id: SpotifyId) -> Result<Album> {
        Ok(Album {
            rid: ResourceId::from((Resource::Album, id)),
            name: format!("album-{}", id),
            original_name: format!("album-{}-original", id),
            version_name: format!("album-{}-version", id),
            artists: vec![],
            label: format!("label-{}", id),
            discs: vec![],
            cover: None,
        })
    }

    async fn get_track(&self, id: SpotifyId) -> Result<Track> {
        Ok(Track {
            rid: ResourceId::from((Resource::Track, id)),
            name: format!("track-{}", id),
            album: ResourceId::from((Resource::Album, id)),
            disc_number: 1,
            track_number: 1,
            duration: Duration::from_secs(5),
            artists: vec![],
            lyrics: None,
            alternatives: vec![],
        })
    }

    async fn get_image(&self, url: &str) -> Result<Bytes> {
        Ok(Bytes::from(format!("image-{}", url)))
    }
}

struct CachedMetadataFetcher<F> {
    fetcher: F,
    directory: PathBuf,
}

impl<F> CachedMetadataFetcher<F> {
    pub async fn new(fetcher: F, directory: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&directory).await?;
        Ok(Self { fetcher, directory })
    }

    async fn store<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Clone + Send + 'static,
    {
        let path = self.directory.join(key);
        let contents = serde_json::to_string_pretty(value)?;
        tokio::fs::write(path, &contents).await?;
        tracing::info!("stored {} to cache:\n{}", key, contents);
        Ok(())
    }

    async fn load<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
    {
        let path = self.directory.join(key);
        let contents = tokio::fs::read_to_string(path).await?;
        let value = serde_json::from_str(&contents)?;
        tracing::info!("loaded {} from cache:\n{}", key, contents);
        Ok(Some(value))
    }
}

impl<F> MetadataFetcher for CachedMetadataFetcher<F>
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let tempdir = tempfile::tempdir()?;
    let dummy_fetcher = DummyMetadataFetcher;
    let fetcher = CachedMetadataFetcher::new(dummy_fetcher, tempdir.path().to_owned()).await?;

    let artist = fetcher.get_artist(SpotifyId::new(1)).await?;
    fetcher.get_artist(SpotifyId::new(1)).await?;
    fetcher.get_artist(SpotifyId::new(1)).await?;
    fetcher.get_artist(SpotifyId::new(1)).await?;
    fetcher.get_artist(SpotifyId::new(1)).await?;

    Ok(())
}
