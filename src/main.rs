#![feature(io_error_other)]

use std::{
    collections::{HashSet, VecDeque},
    ffi::OsStr,
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
    Scan(ScanArgs),
    Sync(SyncArgs),
    Update(UpdateArgs),
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

    #[clap(long, env = "SPOTDL_FORMAT")]
    format: Option<String>,

    #[clap(long, env = "SPOTDL_FFMPEG_PATH")]
    ffmpeg_path: Option<PathBuf>,

    #[clap(flatten)]
    resource: ResourceSelector,
}

#[derive(Debug, Parser)]
struct ScanArgs {
    #[clap(default_value = ".")]
    dir: PathBuf,
}

#[derive(Debug, Parser)]
struct EnvManifest {
    #[clap(long, env = "SPOTDL_MANIFEST", default_value = "spotdl")]
    manifest: PathBuf,
}

#[derive(Debug, Parser)]
struct SyncArgs {
    #[clap(subcommand)]
    action: SyncAction,
}

#[derive(Debug, Parser)]
enum SyncAction {
    Add(SyncAddArgs),
    Remove(SyncRemoveArgs),
    List(SyncListArgs),
    Download(SyncDownloadArgs),
}

#[derive(Debug, Parser)]
struct SyncAddArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(flatten)]
    manifest: EnvManifest,

    #[clap(flatten)]
    resource: ResourceSelector,
}

#[derive(Debug, Parser)]
struct SyncRemoveArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(flatten)]
    manifest: EnvManifest,

    #[clap(flatten)]
    resource: ResourceSelector,
}

#[derive(Debug, Parser)]
struct SyncListArgs {
    #[clap(flatten)]
    manifest: EnvManifest,
}

#[derive(Debug, Parser)]
struct SyncDownloadArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(long)]
    output_dir: Option<PathBuf>,

    #[clap(long, env = "SPOTDL_FORMAT")]
    format: Option<String>,

    #[clap(long, env = "SPOTDL_FFMPEG_PATH")]
    ffmpeg_path: Option<PathBuf>,

    #[clap(flatten)]
    manifest: EnvManifest,
}

#[derive(Debug, Parser)]
struct UpdateArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    path: PathBuf,
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

async fn subcmd_download(args: DownloadArgs) -> Result<()> {
    let rid = helper_resource_selector_to_id(&args.resource)?;
    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let session = Session::connect(credentials).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::new(session.clone(), cache);
    helper_download_rids(
        session,
        fetcher,
        vec![rid],
        args.output_dir.unwrap_or_else(|| PathBuf::from(".")),
        args.format,
        args.ffmpeg_path,
    )
    .await?;
    Ok(())
}

async fn subcmd_scan(args: ScanArgs) -> Result<()> {
    helper_scan_track_ids_in_dir(&args.dir).await?;
    for track in helper_scan_track_ids_in_dir(&args.dir).await? {
        println!("{}", track);
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ManifestEntry {
    resource_id: ResourceId,
    description: String,
}

#[derive(Debug, Clone)]
struct Manifest(Vec<ManifestEntry>);

impl Manifest {
    fn contains(&self, resource_id: &ResourceId) -> bool {
        self.0.iter().any(|entry| entry.resource_id == *resource_id)
    }

    fn add(&mut self, resource_id: ResourceId, description: String) {
        self.0.push(ManifestEntry {
            resource_id,
            description,
        });
    }

    fn remove(&mut self, resource_id: &ResourceId) {
        self.0.retain(|entry| entry.resource_id != *resource_id);
    }

    fn entries(&self) -> impl Iterator<Item = &ManifestEntry> {
        self.0.iter()
    }
}

async fn subcmd_sync(args: SyncArgs) -> Result<()> {
    match args.action {
        SyncAction::Add(args) => subcmd_sync_add(args).await?,
        SyncAction::Remove(args) => subcmd_sync_remove(args).await?,
        SyncAction::List(args) => subcmd_sync_list(args).await?,
        SyncAction::Download(args) => subcmd_sync_download(args).await?,
    }
    Ok(())
}

async fn subcmd_sync_add(args: SyncAddArgs) -> Result<()> {
    let mut manifest = helper_read_manifest(&args.manifest.manifest).await?;
    let rid = helper_resource_selector_to_id(&args.resource)?;
    if manifest.contains(&rid) {
        return Ok(());
    }

    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let session = Session::connect(credentials).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::new(session.clone(), cache);

    let description = match rid.resource {
        Resource::Artist => {
            let artist = fetcher.get_artist(rid.id).await?;
            artist.name
        }
        Resource::Album => {
            let album = fetcher.get_album(rid.id).await?;
            let artist = fetcher.get_artist(album.artists[0].id).await?;
            format!("{} - {}", artist.name, album.name)
        }
        Resource::Track => {
            let track = fetcher.get_track(rid.id).await?;
            let album = fetcher.get_album(track.album.id).await?;
            let artist = fetcher.get_artist(track.artists[0].id).await?;
            format!("{} - {} - {}", artist.name, album.name, track.name)
        }
        Resource::Playlist => fetcher.get_playlist(rid.id).await?.name,
    };

    manifest.add(rid, description);
    helper_write_manifest(&args.manifest.manifest, &manifest).await?;

    Ok(())
}

async fn subcmd_sync_remove(args: SyncRemoveArgs) -> Result<()> {
    let mut manifest = helper_read_manifest(&args.manifest.manifest).await?;
    let rid = helper_resource_selector_to_id(&args.resource)?;
    manifest.remove(&rid);
    helper_write_manifest(&args.manifest.manifest, &manifest).await?;
    Ok(())
}

async fn subcmd_sync_list(args: SyncListArgs) -> Result<()> {
    let manifest = helper_read_manifest(&args.manifest.manifest).await?;
    for entry in manifest.entries() {
        println!("{} {}", entry.resource_id, entry.description);
    }
    Ok(())
}

async fn subcmd_sync_download(args: SyncDownloadArgs) -> Result<()> {
    let manifest = helper_read_manifest(&args.manifest.manifest).await?;
    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let session = Session::connect(credentials).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::new(session.clone(), cache);
    let rids = manifest.entries().map(|entry| entry.resource_id).collect();
    helper_download_rids(
        session,
        fetcher,
        rids,
        args.output_dir.unwrap_or_else(|| PathBuf::from(".")),
        args.format,
        args.ffmpeg_path,
    )
    .await?;
    Ok(())
}

async fn subcmd_update(args: UpdateArgs) -> Result<()> {
    let files = helper_collect_files(&args.path).await?;
    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let session = Session::connect(credentials).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::new(session.clone(), cache);

    for file in files {
        let id = match helper_scan_track_id_in_file(&file).await {
            Some(id) => id,
            None => continue,
        };

        tracing::info!("updating metadata for {}", file.display());
        let track = fetcher.get_track(id).await?;
        let tags = helper_metadata_to_tag(&fetcher, &track).await?;
        if file.extension() == Some(OsStr::new("wav")) {
            tags.write_to_wav_path(&file, id3::Version::Id3v24)
                .with_context(|| {
                    format!(
                        "failed to write metadata to file: {}",
                        file.to_string_lossy()
                    )
                })?;
        } else {
            tags.write_to_path(&file, id3::Version::Id3v24)
                .with_context(|| {
                    format!(
                        "failed to write metadata to file: {}",
                        file.to_string_lossy()
                    )
                })?;
        }
    }

    Ok(())
}

#[allow(dead_code)]
type TrackIdSender = tokio::sync::mpsc::Sender<SpotifyId>;
type TrackIdReceiver = tokio::sync::mpsc::Receiver<SpotifyId>;

async fn helper_download_rids(
    session: Session,
    fetcher: MetadataFetcher,
    rids: Vec<ResourceId>,
    output_dir: PathBuf,
    format: Option<String>,
    ffmpeg_path: Option<PathBuf>,
) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let handle = tokio::spawn(helper_download_task(
        rx,
        output_dir,
        fetcher.clone(),
        session,
        format,
        ffmpeg_path,
    ));

    for rid in rids {
        match rid.resource {
            Resource::Artist => {
                let artist = fetcher.get_artist(rid.id).await?;
                let albums = artist.albums.into_iter().chain(artist.singles.into_iter());
                for album in albums {
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
    }
    drop(tx);

    handle.await??;

    Ok(())
}

async fn helper_download_task(
    mut rx: TrackIdReceiver,
    out_dir: PathBuf,
    fetcher: MetadataFetcher,
    session: Session,
    format: Option<String>,
    ffmpeg_path: Option<PathBuf>,
) -> Result<()> {
    let tempdir = tempfile::tempdir().context("creating temporary directory")?;
    let known_tracks = helper_scan_track_ids_in_dir(&out_dir).await?;
    let known_tracks: HashSet<SpotifyId> = HashSet::from_iter(known_tracks.into_iter());

    let ffmpeg_path = match ffmpeg_path {
        Some(ref path) => path.clone(),
        None => PathBuf::new().join("ffmpeg"),
    };

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
            spotdl::download::BITS_PER_SAMPLE as u16,
        );

        tracing::info!(
            "downloading track {}: {} - {} - {}",
            track.rid,
            artist.name,
            album.name,
            track.name
        );

        let (wav_path, final_path) = {
            let track_path = helper_make_track_path(&artist, &album, &track);
            let wav_path = tempdir.path().join(&track_path);
            let final_path = out_dir.join(&track_path);
            (wav_path, final_path)
        };

        if let Some(wav_parent) = wav_path.parent() {
            tokio::fs::create_dir_all(wav_parent)
                .await
                .context("creating track directory")?;
        }

        if let Some(final_parent) = final_path.parent() {
            tokio::fs::create_dir_all(final_parent)
                .await
                .context("creating track directory")?;
        }

        buffer.clear();
        let mut cursor = std::io::Cursor::new(&mut buffer);
        wav::write(header, &wav::BitDepth::Sixteen(samples), &mut cursor)
            .context("writing wav file")?;
        let mut wav_file = tokio::fs::File::create(&wav_path)
            .await
            .with_context(|| format!("creating track file {}", wav_path.display()))?;
        wav_file
            .write_all(&buffer)
            .await
            .context("writing track file")?;

        let tag = helper_metadata_to_tag(&fetcher, &track)
            .await
            .context("getting metadata")?;
        tag.write_to_wav_path(&wav_path, id3::Version::Id3v24)
            .context("writing metadata")?;

        if let Some(ref format) = format {
            let mut ffmpeg = tokio::process::Command::new(&ffmpeg_path);
            let output = final_path.with_extension(format);
            ffmpeg
                .arg("-i")
                .arg(&wav_path)
                .arg("-b:a")
                .arg("320k")
                .arg("-y")
                .arg(&output);
            let status = ffmpeg.status().await.context("running ffmpeg")?;
            if !status.success() {
                return Err(anyhow::anyhow!("ffmpeg failed"));
            }
            tokio::fs::remove_file(wav_path)
                .await
                .context("removing wav file")?;
        } else {
            tokio::fs::copy(&wav_path, final_path)
                .await
                .context("copying wav file")?;
            tokio::fs::remove_file(&wav_path)
                .await
                .context("removing wav file")?;
        }
    }

    Ok(())
}

fn helper_make_track_path(
    artist: &spotdl::metadata::Artist,
    album: &spotdl::metadata::Album,
    track: &spotdl::metadata::Track,
) -> PathBuf {
    let mut track_rel_path = PathBuf::default();
    let track_path_name = track.name.replace("/", "-");
    track_rel_path.push(&artist.name);
    track_rel_path.push(&album.name);
    if album.discs.len() > 1 {
        track_rel_path.push(format!("Disc {}", track.disc_number));
    }
    track_rel_path.push(format!("{} - {}.wav", track.track_number, track_path_name));
    track_rel_path
}

async fn helper_read_manifest(path: &Path) -> Result<Manifest> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Manifest(Vec::new())),
        Err(err) => return Err(err).context("reading manifest file"),
    };
    let mut entries = Vec::new();
    for line in contents.lines() {
        let (uri, desc) = line
            .split_once(' ')
            .ok_or_else(|| anyhow::anyhow!("invalid manifest line: '{}'", line))?;
        let resource_id = ResourceId::from_uri(uri)
            .with_context(|| anyhow::anyhow!("invalid resource URI in manifest: '{}'", uri))?;
        entries.push(ManifestEntry {
            resource_id,
            description: desc.to_owned(),
        });
    }
    Ok(Manifest(entries))
}

async fn helper_write_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    let mut contents = String::new();
    for entry in manifest.0.iter() {
        contents.push_str(&entry.resource_id.to_uri());
        contents.push(' ');
        contents.push_str(&entry.description);
        contents.push('\n');
    }
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .context("creating manifest directory")?;
    tokio::fs::write(path, contents)
        .await
        .context("writing manifest file")?;
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

async fn helper_collect_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut dirs = VecDeque::new();
    if path.is_file() {
        files.push(path.to_path_buf());
    } else {
        dirs.push_back(path.to_path_buf());
    }
    while let Some(dir) = dirs.pop_front() {
        let mut readdir = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = readdir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                dirs.push_back(path);
            } else {
                files.push(path);
            }
        }
    }
    Ok(files)
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
            if extended.value.len() < 22 {
                tracing::warn!(
                    "spotify id '{}' in file '{}' is too short",
                    extended.value,
                    path.display()
                );
                continue;
            }

            // the spotify id is always 22 bytes long
            // extended.value can sometimes include a null terminator so remove that.
            let value = &extended.value[..22];

            match spotdl::id::parse_id(value) {
                Ok(id) => {
                    tracing::debug!("found spotify id '{}' in file '{}'", id, path.display());
                    return Some(id);
                }
                Err(err) => {
                    tracing::warn!("failed to parse spotify id '{}': {}", value, err);
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
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("warn,spotdl=info"))
        .unwrap();
    tracing_subscriber::fmt::fmt()
        .with_env_filter(filter)
        .init();

    let args = Args::parse();
    match args.subcmd {
        SubCmd::Login(args) => subcmd_login(args).await?,
        SubCmd::Logout(args) => subcmd_logout(args).await?,
        SubCmd::Info(args) => subcmd_info(args).await?,
        SubCmd::Download(args) => subcmd_download(args).await?,
        SubCmd::Scan(args) => subcmd_scan(args).await?,
        SubCmd::Sync(args) => subcmd_sync(args).await?,
        SubCmd::Update(args) => subcmd_update(args).await?,
    };

    Ok(())
}
