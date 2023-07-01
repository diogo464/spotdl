use bytes::Bytes;

use crate::{
    metadata::{Album, Artist, Image, Playlist, Track},
    SpotifyId,
};

mod filesystem;
pub use filesystem::{FsCache, FsCacheMetadataFetcher, FsCacheMetadataFetcherParams};
mod null;
pub use null::NullMetadataFetcher;
mod rate_limited;
pub use rate_limited::RateLimitedMetadataFetcher;
mod spotify;
pub use spotify::SpotifyMetadataFetcher;

#[async_trait::async_trait]
pub trait MetadataFetcher: Send + Sync + 'static {
    async fn get_artist(&self, id: SpotifyId) -> std::io::Result<Artist>;
    async fn get_album(&self, id: SpotifyId) -> std::io::Result<Album>;
    async fn get_track(&self, id: SpotifyId) -> std::io::Result<Track>;
    async fn get_playlist(&self, id: SpotifyId) -> std::io::Result<Playlist>;
    async fn get_image(&self, url: &str) -> std::io::Result<Image>;
}

/*
use std::{future::Future, sync::Arc};

struct AMetadataFetcherInner {
    get_artist: Box<
        dyn Fn(SpotifyId) -> Box<dyn Future<Output = std::io::Result<Artist>> + Send> + Send + Sync,
    >,
    get_album: Box<
        dyn Fn(SpotifyId) -> Box<dyn Future<Output = std::io::Result<Album>> + Send> + Send + Sync,
    >,
    get_track: Box<
        dyn Fn(SpotifyId) -> Box<dyn Future<Output = std::io::Result<Track>> + Send> + Send + Sync,
    >,
    get_image:
        Box<dyn Fn(&str) -> Box<dyn Future<Output = std::io::Result<Bytes>> + Send> + Send + Sync>,
}

#[derive(Clone)]
pub struct AMetadataFetcher(Arc<AMetadataFetcherInner>);

impl AMetadataFetcher {
    pub fn new<F>(fetcher: F) -> Self
    where
        F: MetadataFetcher,
    {
        let fetcher = Arc::new(fetcher);
        Self(Arc::new(AMetadataFetcherInner {
            get_artist: Box::new(move |id| {
                let fetcher = fetcher.clone();
                Box::new(async move { fetcher.get_artist(id).await })
            }),
            get_album: Box::new(move |id| {
                let fetcher = fetcher.clone();
                Box::new(async move { fetcher.get_album(id).await })
            }),
            get_track: Box::new(move |id| {
                let fetcher = fetcher.clone();
                Box::new(async move { fetcher.get_track(id).await })
            }),
            get_image: Box::new(move |url| {
                let fetcher = fetcher.clone();
                Box::new(async move { fetcher.get_image(url).await })
            }),
        }))
    }

    pub fn get_artist(
        &self,
        id: SpotifyId,
    ) -> Box<dyn Future<Output = std::io::Result<Artist>> + Send + 'static> {
        (self.0.get_artist)(id)
    }
    pub fn get_album(
        &self,
        id: SpotifyId,
    ) -> Box<dyn Future<Output = std::io::Result<Album>> + Send + 'static> {
        (self.0.get_album)(id)
    }
    pub fn get_track(
        &self,
        id: SpotifyId,
    ) -> Box<dyn Future<Output = std::io::Result<Track>> + Send + 'static> {
        (self.0.get_track)(id)
    }
    pub fn get_image(
        &self,
        url: &str,
    ) -> Box<dyn Future<Output = std::io::Result<Bytes>> + Send + 'static> {
        (self.0.get_image)(url)
    }
}
*/
