use crate::metadata::Image;

use super::MetadataFetcher;

#[derive(Debug)]
pub struct NullMetadataFetcher;

#[async_trait::async_trait]
impl MetadataFetcher for NullMetadataFetcher {
    async fn get_artist(&self, _id: crate::SpotifyId) -> std::io::Result<crate::metadata::Artist> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "NullMetadataFetcher::get_artist",
        ))
    }

    async fn get_album(&self, _id: crate::SpotifyId) -> std::io::Result<crate::metadata::Album> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "NullMetadataFetcher::get_album",
        ))
    }

    async fn get_track(&self, _id: crate::SpotifyId) -> std::io::Result<crate::metadata::Track> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "NullMetadataFetcher::get_track",
        ))
    }

    async fn get_playlist(
        &self,
        _id: crate::SpotifyId,
    ) -> std::io::Result<crate::metadata::Playlist> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "NullMetadataFetcher::get_playlist",
        ))
    }

    async fn get_image(&self, _url: &str) -> std::io::Result<Image> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "NullMetadataFetcher::get_image",
        ))
    }
}
