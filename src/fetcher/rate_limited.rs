#![allow(dead_code)]
use crate::metadata::Image;

use super::MetadataFetcher;

// TODO: this is currently unimplemented because we added retries with sleeps in the spotify
// metadata fetcher

#[derive(Debug)]
pub struct RateLimitedMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    fetcher: F,
}

impl<F> RateLimitedMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    pub async fn new(fetcher: F) -> Self {
        Self { fetcher }
    }
}

#[async_trait::async_trait]
#[allow(unused_variables)]
impl<F> MetadataFetcher for RateLimitedMetadataFetcher<F>
where
    F: MetadataFetcher,
{
    async fn get_artist(&self, id: crate::SpotifyId) -> std::io::Result<crate::metadata::Artist> {
        todo!()
    }

    async fn get_album(&self, id: crate::SpotifyId) -> std::io::Result<crate::metadata::Album> {
        todo!()
    }

    async fn get_track(&self, id: crate::SpotifyId) -> std::io::Result<crate::metadata::Track> {
        todo!()
    }

    async fn get_playlist(
        &self,
        id: crate::SpotifyId,
    ) -> std::io::Result<crate::metadata::Playlist> {
        todo!()
    }

    async fn get_image(&self, url: &str) -> std::io::Result<Image> {
        todo!()
    }
}
