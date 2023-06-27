use crate::{
    metadata::{self, MetadataFetcher},
    SpotifyId,
};

pub const TAG_SPOTIFY_TRACK_ID: &str = "spotify_track_id";
pub const TAG_SPOTIFY_ALBUM_ID: &str = "spotify_album_id";
pub const TAG_SPOTIFY_ARTIST_ID: &str = "spotify_artist_id";

pub async fn fetch_metadata_to_tag(
    track_id: SpotifyId,
    fetcher: &MetadataFetcher,
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
        let response = reqwest::get(&cover).await?;
        let mimetype = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_owned();
        let data = response.bytes().await?;

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