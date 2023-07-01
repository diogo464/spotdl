use std::{io::Result, time::Duration};

use librespot::metadata::Metadata;
use tokio::sync::Mutex;

use crate::{
    metadata::{Album, Artist, Disc, Image, Lyrics, LyricsKind, Playlist, SyncedLine, Track},
    session::Session,
    Credentials, Resource, ResourceId, SpotifyId,
};

use super::MetadataFetcher;

#[derive(Debug)]
enum FetcherSession {
    Ready(Session),
    Delayed(Credentials),
}

#[derive(Debug)]
pub struct SpotifyMetadataFetcher {
    session: Mutex<FetcherSession>,
    client: reqwest::Client,
}

macro_rules! fetch_helper {
    ($self:expr, $res:expr, $id:expr, $lstype:ty) => {{
        let lsid = ResourceId::new($res, $id).to_librespot();
        let session = $self.session().await?;
        let mut sleep = Duration::from_secs(1);
        let metadata = loop {
            let result = <$lstype>::get(session.librespot(), &lsid).await;
            match result {
                Ok(artist) => break artist,
                Err(err) if err.kind == librespot::core::error::ErrorKind::ResourceExhausted => {
                    tracing::debug!("librespot rate limited, waiting {:?}", sleep);
                    tokio::time::sleep(sleep).await;
                    sleep *= 2;
                }
                Err(err) => return Err(std::io::Error::other(err)),
            }
        };
        metadata
    }};
}

impl SpotifyMetadataFetcher {
    pub fn new(session: Session) -> Self {
        Self {
            session: Mutex::new(FetcherSession::Ready(session)),
            client: reqwest::Client::new(),
        }
    }

    pub fn delayed(credentials: Credentials) -> Self {
        Self {
            session: Mutex::new(FetcherSession::Delayed(credentials)),
            client: reqwest::Client::new(),
        }
    }

    async fn get_librespot_artist(&self, id: SpotifyId) -> Result<librespot::metadata::Artist> {
        tracing::debug!("fetching librespot artist {}", id);
        let artist = fetch_helper!(self, Resource::Track, id, librespot::metadata::Artist);
        tracing::debug!("fetched librespot artist {}", id);
        Ok(artist)
    }

    async fn get_librespot_album(&self, id: SpotifyId) -> Result<librespot::metadata::Album> {
        tracing::debug!("fetching librespot album {}", id);
        let album = fetch_helper!(self, Resource::Album, id, librespot::metadata::Album);
        tracing::debug!("fetched librespot album {}", id);
        Ok(album)
    }

    async fn get_librespot_track(&self, id: SpotifyId) -> Result<librespot::metadata::Track> {
        tracing::debug!("fetching librespot track {}", id);
        let track = fetch_helper!(self, Resource::Track, id, librespot::metadata::Track);
        tracing::debug!("fetched librespot track {}", id);
        Ok(track)
    }

    async fn get_librespot_playlist(&self, id: SpotifyId) -> Result<librespot::metadata::Playlist> {
        tracing::debug!("fetching librespot playlist {}", id);
        let playlist = fetch_helper!(self, Resource::Playlist, id, librespot::metadata::Playlist);
        tracing::debug!("fetched librespot playlist {}", id);
        Ok(playlist)
    }

    async fn get_librespot_lyrics(&self, id: SpotifyId) -> Result<librespot::metadata::Lyrics> {
        tracing::debug!("fetching librespot lyrics {}", id);
        let lyrics = fetch_helper!(self, Resource::Track, id, librespot::metadata::Lyrics);
        tracing::debug!("fetched librespot lyrics {}", id);
        Ok(lyrics)
    }

    async fn session(&self) -> Result<Session> {
        let mut fsession = self.session.lock().await;
        match &*fsession {
            FetcherSession::Ready(s) => Ok(s.clone()),
            FetcherSession::Delayed(c) => {
                let s = Session::connect(c.clone()).await.map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("failed to connect to spotify: {}", e),
                    )
                })?;
                *fsession = FetcherSession::Ready(s.clone());
                Ok(s)
            }
        }
    }
}

#[async_trait::async_trait]
impl MetadataFetcher for SpotifyMetadataFetcher {
    async fn get_artist(&self, id: crate::SpotifyId) -> std::io::Result<crate::metadata::Artist> {
        let artist = self.get_librespot_artist(id).await?;

        let albums = artist
            .albums_current()
            .map(|id| ResourceId::from_librespot_with(Resource::Album, *id))
            .collect::<Vec<_>>();

        let singles = artist
            .singles_current()
            .map(|id| ResourceId::from_librespot_with(Resource::Album, *id))
            .collect::<Vec<_>>();

        let compilations = artist
            .compilations_current()
            .map(|id| ResourceId::from_librespot_with(Resource::Album, *id))
            .collect::<Vec<_>>();

        let appears_on = artist
            .appears_on_albums_current()
            .map(|id| ResourceId::from_librespot_with(Resource::Album, *id))
            .collect::<Vec<_>>();

        let artist = Artist {
            rid: ResourceId::new(Resource::Artist, id),
            name: artist.name,
            albums,
            singles,
            compilations,
            appears_on,
            genres: artist.genre,
        };
        Ok(artist)
    }

    async fn get_album(&self, id: crate::SpotifyId) -> std::io::Result<crate::metadata::Album> {
        let album = self.get_librespot_album(id).await?;

        let artists = album
            .artists
            .0
            .into_iter()
            .map(|artist| ResourceId::from_librespot_with(Resource::Artist, artist.id))
            .collect::<Vec<_>>();

        let mut discs = Vec::new();
        for disc in album.discs.0 {
            let tracks = disc
                .tracks
                .0
                .into_iter()
                .map(|id| ResourceId::from_librespot_with(Resource::Track, id))
                .collect::<Vec<_>>();

            discs.push(Disc {
                number: disc.number as u32,
                tracks,
            });
        }

        let mut best_cover = None;
        let mut best_cover_size = 0;
        for cover in album.covers.0 {
            let size = cover.width * cover.height;
            if size > best_cover_size {
                best_cover = Some(cover.id);
                best_cover_size = size;
            }
        }
        let cover = best_cover.map(|id| format!("https://i.scdn.co/image/{id}"));

        let album = Album {
            rid: ResourceId::new(Resource::Album, id),
            name: album.name,
            original_name: album.original_title,
            version_name: album.version_title,
            artists,
            label: album.label,
            discs,
            cover,
        };
        Ok(album)
    }

    async fn get_track(&self, id: crate::SpotifyId) -> std::io::Result<crate::metadata::Track> {
        let track = self.get_librespot_track(id).await?;
        let lyrics = if track.has_lyrics {
            let lyrics = self.get_librespot_lyrics(id).await?;
            let inner = lyrics.lyrics;
            let kind = match inner.sync_type {
                librespot::metadata::lyrics::SyncType::Unsynced => {
                    let lines = inner.lines.into_iter().map(|l| l.words).collect::<Vec<_>>();

                    LyricsKind::Unsynchronized(lines)
                }
                librespot::metadata::lyrics::SyncType::LineSynced => {
                    let lines = inner
                        .lines
                        .into_iter()
                        .map(|l| SyncedLine {
                            start_time: Duration::from_millis(l.start_time_ms.parse().unwrap()),
                            end_time: Duration::from_millis(l.end_time_ms.parse().unwrap()),
                            text: l.words,
                        })
                        .collect::<Vec<_>>();

                    LyricsKind::Synchronized(lines)
                }
            };
            Some(Lyrics {
                language: inner.language,
                kind,
            })
        } else {
            None
        };

        let artists = track
            .artists_with_role
            .0
            .into_iter()
            .map(|artist| ResourceId::from_librespot_with(Resource::Artist, artist.id))
            .collect::<Vec<_>>();

        let track = Track {
            rid: ResourceId::new(Resource::Track, id),
            name: track.name,
            album: ResourceId::from_librespot_with(Resource::Album, track.album.id),
            disc_number: track.disc_number as u32,
            track_number: track.number as u32,
            duration: Duration::from_millis(track.duration as u64),
            artists,
            lyrics,
            alternatives: track
                .alternatives
                .0
                .into_iter()
                .map(|id| ResourceId::from_librespot_with(Resource::Track, id))
                .collect(),
        };
        Ok(track)
    }

    async fn get_playlist(&self, id: SpotifyId) -> std::io::Result<crate::metadata::Playlist> {
        let playlist = self.get_librespot_playlist(id).await?;

        let tracks = playlist
            .tracks()
            .map(|id| ResourceId::from_librespot_with(Resource::Track, *id))
            .collect::<Vec<_>>();

        let playlist = Playlist {
            rid: ResourceId::new(Resource::Playlist, id),
            name: playlist.name().to_string(),
            tracks,
        };
        Ok(playlist)
    }

    async fn get_image(&self, url: &str) -> std::io::Result<Image> {
        let response = self.client.get(url).send().await.map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to send get request: {}", e),
            )
        })?;

        let mimetype = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_owned();
        let data = response.bytes().await.map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to get response bytes: {}", e),
            )
        })?;
        let image = Image {
            data,
            content_type: mimetype,
        };

        Ok(image)
    }
}
