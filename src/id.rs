use serde::{Deserialize, Serialize};
use thiserror::Error;

type Result<T> = std::result::Result<T, IdParseError>;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Hash)]
#[error("Invalid Spotify ID")]
pub struct IdParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resource {
    Artist,
    Album,
    Track,
    Playlist,
}

impl std::fmt::Display for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Resource::Artist => "artist",
            Resource::Album => "album",
            Resource::Track => "track",
            Resource::Playlist => "playlist",
        };
        write!(f, "{}", str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpotifyId(u128);

impl SpotifyId {
    pub fn new(id: u128) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for SpotifyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = librespot::core::SpotifyId {
            id: self.0,
            item_type: From::from(""),
        };
        let ids = id.to_base62().map_err(|_| std::fmt::Error)?;
        write!(f, "{}", ids)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId {
    pub resource: Resource,
    pub id: SpotifyId,
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = librespot::core::SpotifyId {
            id: self.id.0,
            item_type: From::from(""),
        };
        let ids = id.to_base62().map_err(|_| std::fmt::Error)?;
        write!(f, "spotify:{}:{}", self.resource, ids)
    }
}

impl From<(Resource, SpotifyId)> for ResourceId {
    fn from((resource, id): (Resource, SpotifyId)) -> Self {
        Self { resource, id }
    }
}

impl ResourceId {
    pub fn new(resource: Resource, id: SpotifyId) -> Self {
        Self { resource, id }
    }

    pub(crate) fn to_librespot(&self) -> librespot::core::SpotifyId {
        use librespot::core::spotify_id::SpotifyItemType;
        let ty = match self.resource {
            Resource::Artist => SpotifyItemType::Artist,
            Resource::Album => SpotifyItemType::Album,
            Resource::Track => SpotifyItemType::Track,
            Resource::Playlist => SpotifyItemType::Playlist,
        };
        librespot::core::SpotifyId {
            id: self.id.0,
            item_type: ty,
        }
    }

    pub(crate) fn from_librespot_with(resource: Resource, id: librespot::core::SpotifyId) -> Self {
        Self {
            resource,
            id: SpotifyId::new(id.id),
        }
    }
}

pub fn parse(identifier: &str, resource: Option<Resource>) -> Result<ResourceId> {
    if let Ok(id) = parse_uri(identifier) {
        return Ok(id);
    }

    if let Ok(id) = parse_url(identifier) {
        return Ok(id);
    }

    if let Ok(id) = parse_id(identifier) {
        if let Some(resource) = resource {
            return Ok((resource, id).into());
        }
    }

    Err(IdParseError)
}

pub fn parse_uri(uri: &str) -> Result<ResourceId> {
    // spotify:artist:6mdiAmATAx73kdxrNrnlao
    // spotify:album:7I9Wh2IgvI3Nnr8Z1ZSWby
    // spotify:track:4OROzZUy6gOWN4UGQVaZMF
    // spotify:playlist:37i9dQZEVXcL56F37CPtSC
    if uri.chars().map(|c| c == ':').count() != 2 {
        return Err(IdParseError);
    }

    let (idx, resource) = uri
        .find("artist")
        .map(|idx| (idx, Resource::Artist))
        .or_else(|| uri.find("album").map(|idx| (idx, Resource::Album)))
        .or_else(|| uri.find("track").map(|idx| (idx, Resource::Track)))
        .or_else(|| uri.find("playlist").map(|idx| (idx, Resource::Playlist)))
        .ok_or(IdParseError)?;
    let rem = &uri[idx..];
    let id_start = rem.find(":").map(|i| i + 1).ok_or(IdParseError)?;
    let id_end = rem[id_start..].find(":").unwrap_or(rem.len() - id_start);
    let id = &rem[id_start..id_start + id_end];
    let id = librespot::core::SpotifyId::from_base62(id)
        .map_err(|_| IdParseError)?
        .id;
    Ok(ResourceId::new(resource, SpotifyId::new(id)))
}

pub fn parse_id(id: &str) -> Result<SpotifyId> {
    let id = librespot::core::SpotifyId::from_base62(id)
        .map_err(|_| IdParseError)?
        .id;
    Ok(SpotifyId::new(id))
}

pub fn parse_url(url: &str) -> Result<ResourceId> {
    // https://open.spotify.com/artist/6mdiAmATAx73kdxrNrnlao?si=8a674ea0e87e44ca
    // https://open.spotify.com/album/7I9Wh2IgvI3Nnr8Z1ZSWby?si=WVIiAtxmRvCFhvZ3naN5OA
    // https://open.spotify.com/track/4OROzZUy6gOWN4UGQVaZMF?si=d976e0d51c9c4a73
    // https://open.spotify.com/playlist/37i9dQZEVXcL56F37CPtSC?si=ce71f46efc434bd5
    let (idx, resource) = url
        .find("artist")
        .map(|idx| (idx, Resource::Artist))
        .or_else(|| url.find("album").map(|idx| (idx, Resource::Album)))
        .or_else(|| url.find("track").map(|idx| (idx, Resource::Track)))
        .or_else(|| url.find("playlist").map(|idx| (idx, Resource::Playlist)))
        .ok_or(IdParseError)?;
    let rem = &url[idx..];
    let id_start = rem.find("/").map(|i| i + 1).ok_or(IdParseError)?;
    let id_end = rem[id_start..].find("?").unwrap_or(rem.len() - id_start);
    let id = &rem[id_start..id_start + id_end];
    let id = librespot::core::SpotifyId::from_base62(id)
        .map_err(|_| IdParseError)?
        .id;
    Ok(ResourceId::new(resource, SpotifyId::new(id)))
}
