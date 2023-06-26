use std::time::Duration;

use librespot::metadata::Metadata;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

use crate::{session::Session, Resource, ResourceId, SpotifyId};

mod null;
pub use null::NullMetadataCache;

mod filesystem;
pub use filesystem::FsMetadataCache;

#[derive(Debug, Error)]
#[error("Metadata error: {0}")]
pub struct MetadataError(Box<dyn std::error::Error + Send + Sync + 'static>);

impl From<librespot::core::Error> for MetadataError {
    fn from(value: librespot::core::Error) -> Self {
        Self(Box::new(value))
    }
}

type Result<T, E = MetadataError> = std::result::Result<T, E>;

pub trait MetadataCache: Send + Sync + 'static {
    fn store(&self, key: &str, value: &[u8], ttl: Duration) -> std::io::Result<()>;
    fn load(&self, key: &str) -> std::io::Result<Option<Vec<u8>>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub rid: ResourceId,
    pub name: String,
    pub albums: Vec<ResourceId>,
    pub singles: Vec<ResourceId>,
    pub compilations: Vec<ResourceId>,
    pub appears_on: Vec<ResourceId>,
    pub genres: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub rid: ResourceId,
    pub name: String,
    pub original_name: String,
    pub version_name: String,
    pub artists: Vec<ResourceId>,
    pub label: String,
    pub discs: Vec<Disc>,
    /// Cover URL
    pub cover: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disc {
    pub number: u32,
    pub tracks: Vec<ResourceId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub rid: ResourceId,
    pub name: String,
    pub album: ResourceId,
    pub disc_number: u32,
    pub track_number: u32,
    pub duration: Duration,
    pub artists: Vec<ResourceId>,
    pub lyrics: Option<Lyrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedLine {
    pub start_time: Duration,
    pub end_time: Duration,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Lyrics {
    Unsynchronized(Vec<String>),
    Synchronized(Vec<SyncedLine>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub rid: ResourceId,
    pub name: String,
    pub tracks: Vec<ResourceId>,
}

pub struct MetadataFetcher {
    cache: Box<dyn MetadataCache>,
    session: Session,
}

impl MetadataFetcher {
    pub fn new<C>(session: Session, cache: C) -> Self
    where
        C: MetadataCache,
    {
        Self {
            cache: Box::new(cache),
            session,
        }
    }

    pub async fn get_artist(&self, id: SpotifyId) -> Result<Artist> {
        let rid = ResourceId::new(Resource::Artist, id);
        if let Some(artist) = self.cache_load(rid) {
            return Ok(artist);
        }

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
        self.cache_store(rid, &artist);
        Ok(artist)
    }

    pub async fn get_album(&self, id: SpotifyId) -> Result<Album> {
        let rid = ResourceId::new(Resource::Album, id);
        if let Some(album) = self.cache_load(rid) {
            return Ok(album);
        }

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
        self.cache_store(rid, &album);
        Ok(album)
    }

    pub async fn get_track(&self, id: SpotifyId) -> Result<Track> {
        let rid = ResourceId::new(Resource::Track, id);
        if let Some(track) = self.cache_load(rid) {
            return Ok(track);
        }

        let track = self.get_librespot_track(id).await?;
        let lyrics = if track.has_lyrics {
            let lyrics = self.get_librespot_lyrics(id).await?;
            let inner = lyrics.lyrics;
            match inner.sync_type {
                librespot::metadata::lyrics::SyncType::Unsynced => {
                    let lines = inner.lines.into_iter().map(|l| l.words).collect::<Vec<_>>();
                    Some(Lyrics::Unsynchronized(lines))
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
                    Some(Lyrics::Synchronized(lines))
                }
            }
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
        };
        self.cache_store(rid, &track);
        Ok(track)
    }

    pub async fn get_playlist(&self, id: SpotifyId) -> Result<Playlist> {
        let rid = ResourceId::new(Resource::Playlist, id);
        if let Some(playlist) = self.cache_load(rid) {
            return Ok(playlist);
        }

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
        self.cache_store(rid, &playlist);
        Ok(playlist)
    }

    async fn get_librespot_artist(&self, id: SpotifyId) -> Result<librespot::metadata::Artist> {
        tracing::debug!("fetching librespot artist {}", id);
        let lsid = ResourceId::new(Resource::Artist, id).to_librespot();
        let artist = librespot::metadata::Artist::get(&self.session.librespot(), &lsid).await?;
        tracing::debug!("fetched librespot artist {}", id);
        Ok(artist)
    }

    async fn get_librespot_album(&self, id: SpotifyId) -> Result<librespot::metadata::Album> {
        tracing::debug!("fetching librespot album {}", id);
        let lsid = ResourceId::new(Resource::Album, id).to_librespot();
        let album = librespot::metadata::Album::get(&self.session.librespot(), &lsid).await?;
        tracing::debug!("fetched librespot album {}", id);
        Ok(album)
    }

    async fn get_librespot_track(&self, id: SpotifyId) -> Result<librespot::metadata::Track> {
        tracing::debug!("fetching librespot track {}", id);
        let lsid = ResourceId::new(Resource::Track, id).to_librespot();
        let track = librespot::metadata::Track::get(&self.session.librespot(), &lsid).await?;
        tracing::debug!("fetched librespot track {}", id);
        Ok(track)
    }

    async fn get_librespot_playlist(&self, id: SpotifyId) -> Result<librespot::metadata::Playlist> {
        tracing::debug!("fetching librespot playlist {}", id);
        let lsid = ResourceId::new(Resource::Playlist, id).to_librespot();
        let playlist = librespot::metadata::Playlist::get(&self.session.librespot(), &lsid).await?;
        tracing::debug!("fetched librespot playlist {}", id);
        Ok(playlist)
    }

    async fn get_librespot_lyrics(&self, id: SpotifyId) -> Result<librespot::metadata::Lyrics> {
        tracing::debug!("fetching librespot lyrics {}", id);
        let lsid = ResourceId::new(Resource::Track, id).to_librespot();
        let lyrics = librespot::metadata::Lyrics::get(&self.session.librespot(), &lsid).await?;
        tracing::debug!("fetched librespot lyrics {}", id);
        Ok(lyrics)
    }

    fn cache_load<T>(&self, key: ResourceId) -> Option<T>
    where
        T: DeserializeOwned,
    {
        tracing::debug!("loading metadata from cache: {}", key);
        let key = key.to_string();
        let data = match self.cache.load(&key) {
            Ok(Some(data)) => data,
            Ok(None) => return None,
            Err(e) => {
                tracing::error!("Failed to load metadata from cache: {}", e);
                return None;
            }
        };
        match serde_json::from_slice(&data) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::error!("Failed to deserialize metadata from cache: {}", err);
                None
            }
        }
    }

    fn cache_store<T>(&self, key: ResourceId, value: &T)
    where
        T: Serialize,
    {
        tracing::debug!("storing metadata in cache: {}", key);
        let key = key.to_string();
        let data = match serde_json::to_vec(value) {
            Ok(data) => data,
            Err(err) => {
                tracing::error!("Failed to serialize metadata for cache: {}", err);
                return;
            }
        };

        // TODO: customize cache ttl
        if let Err(err) = self
            .cache
            .store(&key, &data, Duration::from_secs(60 * 60 * 4))
        {
            tracing::error!("Failed to store metadata in cache: {}", err);
        }
    }
}
