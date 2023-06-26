#![feature(io_error_other)]

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde::{de::DeserializeOwned, Serialize};
use spotdl::{
    metadata::{FsMetadataCache, MetadataFetcher},
    session::Session,
    Credentials, LoginCredentials, Resource, ResourceId, SpotifyId,
};
use tokio::io::AsyncWriteExt;

const TAG_SPOTIFY_TRACK_ID: &str = "SPOTIFY_TRACK_ID";
const TAG_SPOTIFY_ALBUM_ID: &str = "SPOTIFY_ALBUM_ID";
const TAG_SPOTIFY_ARTIST_ID: &str = "SPOTIFY_ARTIST_ID";

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    subcmd: SubCmd,
}

#[derive(Debug, Parser)]
struct EnvCacheDir {
    #[clap(long, env = "SPOTDL_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct EnvAuth {
    #[clap(long, env = "SPOTDL_USER")]
    username: Option<String>,
    #[clap(long, env = "SPOTDL_PASS")]
    password: Option<String>,
}

#[derive(Debug, Parser)]
enum SubCmd {
    Login(LoginArgs),
    Logout(LogoutArgs),
    Info(InfoArgs),
    Download(DownloadArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ResourceKind {
    Artist,
    Album,
    Track,
    Playlist,
}

#[derive(Debug, Parser)]
struct ResourceSelector {
    /// Identifier kind.
    /// If not specified, it will be inferred from the identifier.
    /// It is only required if the identifier is ambiguous, for example, '5ujWPN9c4lh8pohnf4iVYK'.
    #[clap(long)]
    kind: Option<ResourceKind>,

    /// Identifier.
    /// This can be a Spotify URI, URL or ID.
    /// Ex:
    ///     - spotify:track:5ujWPN9c4lh8pohnf4iVYK
    ///     - https://open.spotify.com/track/5ujWPN9c4lh8pohnf4iVYK
    ///     - 5ujWPN9c4lh8pohnf4iVYK
    id: String,
}

#[derive(Debug, Parser)]
struct LoginArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    /// Spotify username
    username: String,

    /// Spotify password
    password: String,
}

#[derive(Debug, Parser)]
struct LogoutArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    username: String,
}

#[derive(Debug, Parser)]
struct InfoArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(flatten)]
    resource: ResourceSelector,
}

#[derive(Debug, Parser)]
struct DownloadArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(long)]
    output_dir: Option<PathBuf>,

    #[clap(flatten)]
    resource: ResourceSelector,
}

async fn subcmd_login(args: LoginArgs) -> Result<()> {
    let credentials_dir = helper_get_credentials_dir(&args.env_cache_dir)?;
    let credentials_path = credentials_dir.join(&args.username);
    let login_credentials = LoginCredentials {
        username: args.username,
        password: args.password,
    };
    let credentials = spotdl::session::login(&login_credentials)
        .await
        .context("logging in")?;
    tokio::fs::create_dir_all(&credentials_dir)
        .await
        .context("creating credentials directory")?;
    helper_write_json(&credentials_path, &credentials).await?;
    Ok(())
}

async fn subcmd_logout(args: LogoutArgs) -> Result<()> {
    let credentials_dir = helper_get_credentials_dir(&args.env_cache_dir)?;
    let credentials_path = credentials_dir.join(args.username);
    if credentials_path.exists() {
        tokio::fs::remove_file(&credentials_path)
            .await
            .context("removing credentials")?;
    }
    Ok(())
}

async fn subcmd_info(args: InfoArgs) -> Result<()> {
    let rid = helper_resource_selector_to_id(&args.resource)?;
    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::delayed(credentials, cache);

    let mut output = Vec::new();
    match rid.resource {
        Resource::Artist => {
            let artist = fetcher.get_artist(rid.id).await?;
            serde_json::to_writer_pretty(&mut output, &artist)?;
        }
        Resource::Album => {
            let album = fetcher.get_album(rid.id).await?;
            serde_json::to_writer_pretty(&mut output, &album)?;
        }
        Resource::Track => {
            let track = fetcher.get_track(rid.id).await?;
            serde_json::to_writer_pretty(&mut output, &track)?;
        }
        Resource::Playlist => {
            let playlist = fetcher.get_playlist(rid.id).await?;
            serde_json::to_writer_pretty(&mut output, &playlist)?;
        }
    }

    tokio::io::stdout().write_all(&output).await?;

    Ok(())
}

type TrackIdSender = tokio::sync::mpsc::Sender<SpotifyId>;
type TrackIdReceiver = tokio::sync::mpsc::Receiver<SpotifyId>;

async fn subcmd_download(args: DownloadArgs) -> Result<()> {
    let rid = helper_resource_selector_to_id(&args.resource)?;
    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let session = Session::connect(credentials).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::new(session.clone(), cache);
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let output_dir = args
        .output_dir
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let handle = tokio::spawn(subcmd_download_task(
        rx,
        output_dir,
        fetcher.clone(),
        session,
    ));

    match rid.resource {
        Resource::Artist => {
            let artist = fetcher.get_artist(rid.id).await?;
            for album in artist.albums {
                let album = fetcher.get_album(album.id).await?;
                for disc in album.discs {
                    for track in disc.tracks {
                        tx.send(track.id).await?;
                    }
                }
            }
        }
        Resource::Album => {
            let album = fetcher.get_album(rid.id).await?;
            for disc in album.discs {
                for track in disc.tracks {
                    tx.send(track.id).await?;
                }
            }
        }
        Resource::Track => {
            tx.send(rid.id).await?;
        }
        Resource::Playlist => {
            let playlist = fetcher.get_playlist(rid.id).await?;
            for track in playlist.tracks {
                tx.send(track.id).await?;
            }
        }
    }
    drop(tx);

    handle.await??;

    Ok(())
}

async fn subcmd_download_task(
    mut rx: TrackIdReceiver,
    out_dir: PathBuf,
    fetcher: MetadataFetcher,
    session: Session,
) -> Result<()> {
    tokio::fs::create_dir_all(&out_dir)
        .await
        .context("creating output directory")?;

    let known_tracks = helper_scan_track_ids_in_dir(&out_dir).await?;
    let known_tracks: HashSet<SpotifyId> = HashSet::from_iter(known_tracks.into_iter());

    let mut buffer = Vec::new();
    while let Some(track) = rx.recv().await {
        if known_tracks.contains(&track) {
            tracing::info!("skipping known track {}", track);
            continue;
        }

        let samples = spotdl::download::download_samples(&session, track).await?;
        let track = fetcher.get_track(track).await?;
        let album = fetcher.get_album(track.album.id).await?;
        let artist = fetcher.get_artist(track.artists[0].id).await?;
        let header = wav::Header::new(
            wav::WAV_FORMAT_PCM,
            spotdl::download::NUM_CHANNELS as u16,
            spotdl::download::SAMPLE_RATE as u32,
            16,
        );

        let mut track_path = out_dir.clone();
        track_path.push(artist.name);
        track_path.push(album.name);
        tokio::fs::create_dir_all(&track_path)
            .await
            .context("creating track directory")?;
        track_path.push(format!("{} - {}.wav", track.track_number, track.name));
        buffer.clear();
        let mut cursor = std::io::Cursor::new(&mut buffer);
        wav::write(header, &wav::BitDepth::Sixteen(samples), &mut cursor)
            .context("writing wav file")?;
        let mut track_file = tokio::fs::File::create(&track_path)
            .await
            .context("creating track file")?;
        track_file
            .write_all(&buffer)
            .await
            .context("writing track file")?;

        let tag = helper_metadata_to_tag(&fetcher, &track)
            .await
            .context("getting metadata")?;
        tag.write_to_wav_path(&track_path, id3::Version::Id3v24)
            .context("writing metadata")?;
    }

    Ok(())
}

async fn helper_metadata_to_tag(
    fetcher: &MetadataFetcher,
    track: &spotdl::metadata::Track,
) -> Result<id3::Tag> {
    use id3::TagLike;

    let mut tag = id3::Tag::new();
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
            spotdl::metadata::LyricsKind::Unsynchronized(lines) => {
                let combined = lines.join("\n");
                tag.add_frame(id3::frame::Lyrics {
                    lang: lyrics.language.clone(),
                    description: String::default(),
                    text: combined,
                });
            }
            spotdl::metadata::LyricsKind::Synchronized(lines) => {
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

async fn helper_scan_track_id_in_file(path: &Path) -> Option<SpotifyId> {
    const EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a"];

    let ext = path.extension().and_then(|ext| ext.to_str())?;
    if !EXTENSIONS.contains(&ext) {
        tracing::debug!(
            "skipping file '{}' because it has an unknown extension",
            path.display()
        );
        return None;
    }

    let tag_result = if ext == "wav" {
        id3::Tag::read_from_wav_path(path)
    } else {
        id3::Tag::read_from_path(path)
    };

    let tag = match tag_result {
        Ok(tag) => tag,
        Err(err) => {
            tracing::warn!(
                "failed to read id3 tag from file '{}': {}",
                path.display(),
                err
            );
            return None;
        }
    };

    for extended in tag.extended_texts() {
        if extended.description == TAG_SPOTIFY_TRACK_ID {
            match spotdl::id::parse_id(&extended.value) {
                Ok(id) => {
                    tracing::debug!("found spotify id '{}' in file '{}'", id, path.display());
                    return Some(id);
                }
                Err(err) => {
                    tracing::warn!("failed to parse spotify id '{}': {}", extended.value, err);
                }
            }
        }
    }
    tracing::debug!(
        "skipping file '{}' because it has no spotify id",
        path.display()
    );
    None
}

async fn helper_scan_track_ids_in_dir(dir: &Path) -> Result<Vec<SpotifyId>> {
    let mut ids = Vec::new();
    let mut dirs = Vec::new();
    dirs.push(dir.to_path_buf());
    while let Some(dir) = dirs.pop() {
        let mut readdir = tokio::fs::read_dir(dir)
            .await
            .context("reading directory")?;
        while let Some(entry) = readdir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if let Some(id) = helper_scan_track_id_in_file(&path).await {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

fn helper_resource_selector_to_id(selector: &ResourceSelector) -> Result<ResourceId> {
    let kind = match selector.kind {
        Some(ResourceKind::Artist) => Some(Resource::Artist),
        Some(ResourceKind::Album) => Some(Resource::Album),
        Some(ResourceKind::Track) => Some(Resource::Track),
        Some(ResourceKind::Playlist) => Some(Resource::Playlist),
        None => None,
    };
    spotdl::id::parse(&selector.id, kind).context("parsing resource id")
}

fn helper_get_cache_dir(dir: &EnvCacheDir) -> Result<PathBuf> {
    match dir.cache_dir {
        Some(ref dir) => Ok(dir.clone()),
        None => {
            let home = std::env::var("HOME").context("getting $HOME")?;
            let dir = PathBuf::from(home).join(".cache/spotdl");
            Ok(dir)
        }
    }
}

fn helper_get_credentials_dir(dir: &EnvCacheDir) -> Result<PathBuf> {
    let cache_dir = helper_get_cache_dir(dir)?;
    Ok(cache_dir.join("credentials"))
}

fn helper_get_metadata_dir(dir: &EnvCacheDir) -> Result<PathBuf> {
    let cache_dir = helper_get_cache_dir(dir)?;
    Ok(cache_dir.join("metadata"))
}

async fn helper_get_credentials(dir: &EnvCacheDir, auth: &EnvAuth) -> Result<Credentials> {
    let credentials_dir = helper_get_credentials_dir(dir)?;
    tokio::fs::create_dir_all(&credentials_dir)
        .await
        .context("creating credentials dir")?;
    if let Some(username) = auth.username.clone() {
        let credentials_path = credentials_dir.join(&username);
        if credentials_path.is_file() && !auth.password.is_some() {
            let credentials = helper_read_json(&credentials_path)
                .await
                .context("reading credentials")?;
            return Ok(credentials);
        } else {
            let credentials = spotdl::session::login(&LoginCredentials {
                username: username.clone(),
                password: auth
                    .password
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("missing password"))?,
            })
            .await?;
            helper_write_json(&credentials_path, &credentials)
                .await
                .context("writing credentials")?;
            return Ok(credentials);
        }
    } else {
        for entry in credentials_dir.read_dir()? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                tracing::debug!("found credentials file: {:?}", path);
                let credentials = helper_read_json(&path)
                    .await
                    .context("reading credentials")?;
                return Ok(credentials);
            }
        }
    }

    Err(anyhow::anyhow!("no credentials found"))
}

async fn helper_read_json<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned,
{
    let json = tokio::fs::read_to_string(path)
        .await
        .context("reading json")?;
    let obj = serde_json::from_str(&json).context("deserializing json")?;
    Ok(obj)
}

async fn helper_write_json<T>(path: &Path, v: &T) -> Result<()>
where
    T: Serialize,
{
    let serialized = serde_json::to_string_pretty(v).context("serializing json")?;
    tokio::fs::write(path, serialized)
        .await
        .context("writing json")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    match args.subcmd {
        SubCmd::Login(args) => subcmd_login(args).await?,
        SubCmd::Logout(args) => subcmd_logout(args).await?,
        SubCmd::Info(args) => subcmd_info(args).await?,
        SubCmd::Download(args) => subcmd_download(args).await?,
    };

    Ok(())
}
