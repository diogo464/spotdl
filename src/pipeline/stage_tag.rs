use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::{
    metadata::{self, MetadataFetcher},
    SpotifyId,
};

use super::{PipelineStage, StageArtifact};

const TAG_SPOTIFY_TRACK_ID: &str = "spotify_track_id";
const TAG_SPOTIFY_ALBUM_ID: &str = "spotify_album_id";
const TAG_SPOTIFY_ARTIST_ID: &str = "spotify_artist_id";

#[derive(Debug, Default)]
pub struct TagStage;

impl TagStage {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl PipelineStage for TagStage {
    fn name(&self) -> &'static str {
        "tag"
    }

    async fn process(
        &self,
        fetcher: &MetadataFetcher,
        _work_dir: &Path,
        artifact: StageArtifact,
    ) -> std::io::Result<PathBuf> {
        let tag = match metadata_to_tag(fetcher, artifact.resource_id.id).await {
            Ok(tag) => tag,
            Err(err) => {
                tracing::error!(
                    "failed to get metadata for {}: {}",
                    artifact.resource_id,
                    err
                );
                return Err(std::io::Error::other("failed to get metadata"));
            }
        };

        if artifact
            .file_path
            .extension()
            .map(|ext| ext == "wav")
            .unwrap_or(false)
        {
            tag.write_to_wav_path(&artifact.file_path, id3::Version::Id3v24)
                .map_err(|err| {
                    tracing::error!(
                        "failed to write tag to {}: {}",
                        artifact.file_path.display(),
                        err
                    );
                    std::io::Error::other(err)
                })?;
        } else {
            tag.write_to_path(&artifact.file_path, id3::Version::Id3v24)
                .map_err(|err| {
                    tracing::error!(
                        "failed to write tag to {}: {}",
                        artifact.file_path.display(),
                        err
                    );
                    std::io::Error::other(err)
                })?;
        }

        Ok(artifact.file_path)
    }
}

async fn metadata_to_tag(
    fetcher: &MetadataFetcher,
    track_id: SpotifyId,
) -> anyhow::Result<id3::Tag> {
    use id3::TagLike;

    let mut tag = id3::Tag::new();
    let track = fetcher.get_track(track_id).await?;
    let album = fetcher.get_album(track.album.id).await?;
    let artist = fetcher.get_artist(album.artists[0].id).await?;

    // set the track artists
    {
        let mut artists = String::new();
        for artist_id in track.artists.iter() {
            let artist = fetcher.get_artist(artist_id.id).await?;
            if !artists.is_empty() {
                artists.push('\0');
            }
            artists.push_str(&artist.name);
        }
        tag.set_artist(&artists);
    }

    // set the album artists
    {
        let mut artists = String::new();
        for artist_id in album.artists.iter() {
            let artist = fetcher.get_artist(artist_id.id).await?;
            if !artists.is_empty() {
                artists.push('\0');
            }
            artists.push_str(&artist.name);
        }
        tag.set_album_artist(&artists);
    }

    // set title
    tag.set_title(&track.name);

    // set duration
    tag.set_duration(track.duration.as_millis() as u32);

    // set genre
    {
        let mut genre = String::new();
        for g in artist.genres.iter() {
            if !genre.is_empty() {
                genre.push('\0');
            }
            genre.push_str(g);
        }
        tag.set_genre(&genre);
    }

    // set disc
    tag.set_disc(track.disc_number as u32);
    tag.set_total_discs(album.discs.len() as u32);

    // set track number
    tag.set_track(track.track_number as u32);
    tag.set_total_tracks(album.discs[track.disc_number as usize - 1].tracks.len() as u32);

    // set lyrics
    if let Some(ref lyrics) = track.lyrics {
        match &lyrics.kind {
            metadata::LyricsKind::Unsynchronized(lines) => {
                let combined = lines.join("\n");
                tag.add_frame(id3::frame::Lyrics {
                    lang: lyrics.language.clone(),
                    description: String::default(),
                    text: combined,
                });
            }
            metadata::LyricsKind::Synchronized(lines) => {
                let mut content = Vec::new();
                for line in lines {
                    content.push((line.start_time.as_millis() as u32, line.text.clone()));
                }
                let synced = id3::frame::SynchronisedLyrics {
                    lang: lyrics.language.clone(),
                    timestamp_format: id3::frame::TimestampFormat::Ms,
                    content_type: id3::frame::SynchronisedLyricsType::Lyrics,
                    description: String::default(),
                    content,
                };
                tag.add_frame(synced);
            }
        }
    }

    // add cover image
    if let Some(cover) = album.cover {
        let response = reqwest::get(&cover)
            .await
            .context("downloading cover image")?;
        let mimetype = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_owned();
        let data = response.bytes().await.context("reading cover image")?;

        tag.add_frame(id3::frame::Picture {
            mime_type: mimetype.to_owned(),
            picture_type: id3::frame::PictureType::CoverFront,
            description: String::default(),
            data: data.to_vec(),
        });
    }

    // set spotify ids
    tag.add_frame(id3::frame::ExtendedText {
        description: TAG_SPOTIFY_TRACK_ID.to_string(),
        value: track.rid.id.to_string(),
    });
    tag.add_frame(id3::frame::ExtendedText {
        description: TAG_SPOTIFY_ALBUM_ID.to_string(),
        value: album.rid.id.to_string(),
    });
    tag.add_frame(id3::frame::ExtendedText {
        description: TAG_SPOTIFY_ARTIST_ID.to_string(),
        value: artist.rid.id.to_string(),
    });

    Ok(tag)
}
