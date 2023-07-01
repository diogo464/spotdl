use std::time::Duration;

use bytes::Bytes;

use serde::{Deserialize, Serialize};

use crate::ResourceId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub content_type: String,
    pub data: Bytes,
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
    pub alternatives: Vec<ResourceId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedLine {
    pub start_time: Duration,
    pub end_time: Duration,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LyricsKind {
    Unsynchronized(Vec<String>),
    Synchronized(Vec<SyncedLine>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lyrics {
    pub language: String,
    pub kind: LyricsKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub rid: ResourceId,
    pub name: String,
    pub tracks: Vec<ResourceId>,
}
