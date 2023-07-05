#![feature(io_error_other)]
#![feature(try_blocks)]

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use indicatif::ProgressStyle;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use spotdl::{
    fetcher::{
        FsCache, FsCacheMetadataFetcher, FsCacheMetadataFetcherParams, MetadataFetcher,
        SpotifyMetadataFetcher,
    },
    pipeline::{
        FFmpegStage, OrganizeStage, PipelineBuilder, PipelineEvents, PipelineSource,
        PipelineSourceAction, TagStage,
    },
    scan::{ScanItem, ScanParams},
    session::Session,
    Credentials, LoginCredentials, Resource, ResourceId, SpotifyId,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufWriter},
    net::{UnixListener, UnixStream},
};

// TODO: remove some unwraps

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    subcmd: SubCmd,
}

#[derive(Debug, Parser)]
struct GroupCache {
    /// Cache directory.
    ///
    /// This directory will be used to save login credentials and metadata from spotify api
    /// requests.
    #[clap(long, env = "SPOTDL_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Artist TTL.
    ///
    /// The time to live for artist metadata in the cache, in seconds(-1 for MAX).
    #[clap(long, env = "SPOTDL_CACHE_ARTIST_TTL", default_value = "86400")]
    artist_ttl: i64,

    /// Album TTL.
    ///
    /// The time to live for album metadata in the cache, in seconds(-1 for MAX).
    #[clap(long, env = "SPOTDL_CACHE_ALBUM_TTL", default_value = "-1")]
    album_ttl: i64,

    /// Track TTL.
    ///
    /// The time to live for track metadata in the cache, in seconds(-1 for MAX).
    #[clap(long, env = "SPOTDL_CACHE_TRACK_TTL", default_value = "-1")]
    track_ttl: i64,

    /// Playlist TTL.
    ///
    /// The time to live for playlist metadata in the cache, in seconds(-1 for MAX).
    #[clap(long, env = "SPOTDL_CACHE_PLAYLIST_TTL", default_value = "3600")]
    playlist_ttl: i64,

    /// Image TTL.
    ///
    /// The time to live for image data in the cache, in seconds(-1 for MAX).
    #[clap(long, env = "SPOTDL_CACHE_IMAGE_TTL", default_value = "-1")]
    image_ttl: i64,
}

impl GroupCache {
    fn create_fs_fetcher_param(&self) -> FsCacheMetadataFetcherParams {
        FsCacheMetadataFetcherParams {
            artist_ttl: self.artist_ttl(),
            album_ttl: self.album_ttl(),
            track_ttl: self.track_ttl(),
            playlist_ttl: self.playlist_ttl(),
            image_ttl: self.image_ttl(),
        }
    }

    fn get_credentials_dir(&self) -> PathBuf {
        let cache_dir = self.get_cache_dir();
        cache_dir.join("credentials")
    }

    fn get_metadata_dir(&self) -> PathBuf {
        let cache_dir = self.get_cache_dir();
        cache_dir.join("metadata")
    }

    fn get_cache_dir(&self) -> PathBuf {
        match self.cache_dir {
            Some(ref dir) => dir.clone(),
            None => {
                let home = std::env::var("HOME")
                    .context("getting $HOME")
                    .expect("getting $HOME");

                PathBuf::from(home).join(".cache/spotdl")
            }
        }
    }

    fn artist_ttl(&self) -> Duration {
        Self::i64_to_dur(self.artist_ttl)
    }

    fn album_ttl(&self) -> Duration {
        Self::i64_to_dur(self.album_ttl)
    }

    fn track_ttl(&self) -> Duration {
        Self::i64_to_dur(self.track_ttl)
    }

    fn playlist_ttl(&self) -> Duration {
        Self::i64_to_dur(self.playlist_ttl)
    }

    fn image_ttl(&self) -> Duration {
        Self::i64_to_dur(self.image_ttl)
    }

    fn i64_to_dur(i: i64) -> Duration {
        if i == -1 {
            Duration::MAX
        } else {
            Duration::from_secs(i as u64)
        }
    }
}

#[derive(Debug, Parser)]
struct GroupAuth {
    /// Spotify username.
    ///
    /// If this username is set then its credentials will be used.
    #[clap(long, env = "SPOTDL_USER")]
    username: Option<String>,

    /// Spotify password.
    ///
    /// If this password is set then the username must also be set.
    /// Already existing credentials are ignored.
    #[clap(long, env = "SPOTDL_PASS")]
    password: Option<String>,
}

#[derive(Debug, Parser)]
struct GroupManifest {
    /// Path to the manifest file.
    #[clap(long, env = "SPOTDL_MANIFEST", default_value = "spotdl")]
    manifest: PathBuf,
}

#[derive(Debug, Parser)]
struct GroupScanDir {
    /// Directories to include during scanning.
    ///
    /// Scanned directories are searched for audio files containing spotify ids in their tags.
    /// Any ids found will not be downloaded again.
    #[clap(long)]
    scan: Vec<PathBuf>,

    /// Directories to exclude during scanning.
    ///
    /// Excluded directories take precedence over included directories.
    /// Any audio file excluded or under an excluded directory is not searched for tags.
    #[clap(long)]
    scan_exclude: Vec<PathBuf>,
}

impl GroupScanDir {
    fn create_params(&self) -> ScanParams {
        let mut params = ScanParams::default();
        for path in self.scan.iter() {
            params.include(path);
        }
        for path in self.scan_exclude.iter() {
            params.exclude(path);
        }
        params
    }
}

#[derive(Debug, Parser)]
struct GroupSource {
    /// Music source directories.
    ///
    /// These directories are searched for audio files containing spotify ids in their tags.
    /// When downloading a track, these directories are searched for the track's id to prevent redownloading.
    /// A source is specified as an `<action>:<path>` pair, where `<action>` can be one of:
    ///     - `copy`: copy the files to the destination.
    ///     - `symlink`: symlink the files to the destination.
    ///     - `hardlink`: hardlink the files to the destination.
    ///
    /// Examples:
    ///    - `copy:/home/user/Music`
    ///    - `symlink:../music`
    ///    - `hardlink:/mnt/music`
    #[clap(long, verbatim_doc_comment)]
    source: Vec<String>,
}

impl GroupSource {
    fn get_sources(&self) -> Result<Vec<PipelineSource>> {
        let mut sources = Vec::new();
        for source in self.source.iter() {
            let source = Self::parse_source(source)?;
            sources.push(source);
        }
        Ok(sources)
    }

    fn parse_source(source: &str) -> Result<PipelineSource> {
        let (action, path) = source.rsplit_once(':').context("missing ':' in source")?;
        let action = match action {
            "copy" => PipelineSourceAction::Copy,
            "symlink" => PipelineSourceAction::SymLink,
            "hardlink" => PipelineSourceAction::HardLink,
            _ => anyhow::bail!("invalid source action"),
        };
        let path = PathBuf::from(path);
        Ok(PipelineSource {
            source: path,
            action,
        })
    }
}

#[derive(Debug, Parser)]
struct GroupFFmpeg {
    /// The format to convert the audio files to.
    ///
    /// This requires ffmpeg to be installed.
    #[clap(long, env = "SPOTDL_FORMAT")]
    format: Option<String>,

    /// Path to ffmpeg.
    ///
    /// If this is not set then the default path is used.
    /// It is only required if a format conversion is required.
    #[clap(long, env = "SPOTDL_FFMPEG_PATH")]
    ffmpeg_path: Option<PathBuf>,
}

impl GroupFFmpeg {
    fn create_stage(&self) -> Option<FFmpegStage> {
        let format = self.format.clone()?;
        let ffmpeg_path = self
            .ffmpeg_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("ffmpeg"));
        Some(FFmpegStage::new(ffmpeg_path, format))
    }
}

#[derive(Debug, Parser)]
struct GroupWatchSocket {
    /// Path to the socket.
    #[clap(long, env = "SPOTDL_WATCH_SOCKET")]
    socket: Option<PathBuf>,
}

impl GroupWatchSocket {
    fn socket_path(&self) -> PathBuf {
        self.socket
            .clone()
            .unwrap_or_else(|| PathBuf::from("/tmp/spotdl.sock"))
    }
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
    Cache(CacheArgs),
    Watcher(WatcherArgs),
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
    /// Artist identifier.
    ///
    /// This can be a Spotify URI, URL or ID.
    #[clap(long)]
    artist: Vec<String>,

    /// Album identifier.
    ///
    /// This can be a Spotify URI, URL or ID.
    #[clap(long)]
    album: Vec<String>,

    /// Track identifier.
    ///
    /// This can be a Spotify URI, URL or ID.
    #[clap(long)]
    track: Vec<String>,

    /// Playlist identifier.
    ///
    /// This can be a Spotify URI, URL or ID.
    #[clap(long)]
    playlist: Vec<String>,

    /// Path to the manifest file.
    ///
    /// This is a path to a manifest file created by spotdl.
    #[clap(long)]
    manifest: Vec<PathBuf>,

    /// Identifier.
    ///
    /// This can be a Spotify URI, URL.
    /// ID is not support since it is ambiguous.
    /// For manifest files, use the `manifest` flag.
    /// Ex:
    ///     - spotify:track:5ujWPN9c4lh8pohnf4iVYK
    ///     - https://open.spotify.com/track/5ujWPN9c4lh8pohnf4iVYK
    #[clap(verbatim_doc_comment)]
    identifiers: Vec<String>,
}

impl ResourceSelector {
    async fn resolve_rids(&self) -> Result<Vec<ResourceId>> {
        let mut resource_ids = Vec::new();
        Self::parse_identifiers(
            &mut resource_ids,
            Some(Resource::Artist),
            self.artist.iter(),
        )?;
        Self::parse_identifiers(&mut resource_ids, Some(Resource::Album), self.album.iter())?;
        Self::parse_identifiers(&mut resource_ids, Some(Resource::Track), self.track.iter())?;
        Self::parse_identifiers(
            &mut resource_ids,
            Some(Resource::Playlist),
            self.playlist.iter(),
        )?;
        Self::parse_identifiers(&mut resource_ids, None, self.identifiers.iter())?;
        for manifest in self.manifest.iter() {
            let manifest = helper_read_manifest(manifest).await?;
            resource_ids.extend(manifest.resource_ids());
        }
        Ok(resource_ids)
    }

    fn parse_identifiers(
        resource_ids: &mut Vec<ResourceId>,
        resource: Option<Resource>,
        identifiers: impl Iterator<Item = impl AsRef<str>>,
    ) -> Result<()> {
        for identifier in identifiers {
            let rid = spotdl::id::parse(identifier.as_ref(), resource)
                .context("parsing resource id (possibily ambiguous)")?;
            resource_ids.push(rid);
        }
        Ok(())
    }
}

/// Login to spotify.
#[derive(Debug, Parser)]
struct LoginArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    /// Spotify username
    username: String,

    /// Spotify password
    password: String,
}

/// Logout from spotify.
#[derive(Debug, Parser)]
struct LogoutArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    username: String,
}

/// Get information about a resource.
#[derive(Debug, Parser)]
struct InfoArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    resource: ResourceSelector,

    /// Output pretty JSON.
    #[clap(long)]
    pretty: bool,

    /// Output as a JSON array.
    #[clap(long)]
    array: bool,
}

/// Download a resource.
///
/// If the resource is an artist, then all tracks from all albums, singles and compilations are
/// downloaded. This does not included "appears on" albums.
///
/// If the resource is a playlist, then all tracks in the playlist are downloaded.
#[derive(Debug, Parser)]
struct DownloadArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    group_ffmpeg: GroupFFmpeg,

    #[clap(flatten)]
    group_scan: GroupScanDir,

    #[clap(flatten)]
    group_source: GroupSource,

    /// Output directory
    #[clap(long, default_value = ".")]
    output_dir: PathBuf,

    #[clap(flatten)]
    resource: ResourceSelector,
}

/// Scan a directory and print all spotify ids found.
#[derive(Debug, Parser)]
struct ScanArgs {
    #[clap(default_value = ".")]
    dir: PathBuf,
}

/// Manifest commands
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

/// Add a resource to the manifest.
#[derive(Debug, Parser)]
struct SyncAddArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    manifest: GroupManifest,

    #[clap(flatten)]
    resource: ResourceSelector,
}

/// Remove a resource from the manifest.
#[derive(Debug, Parser)]
struct SyncRemoveArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    manifest: GroupManifest,

    #[clap(flatten)]
    resource: ResourceSelector,
}

/// List all resources in the manifest.
#[derive(Debug, Parser)]
struct SyncListArgs {
    #[clap(flatten)]
    manifest: GroupManifest,
}

/// Download all resources in the manifest.
#[derive(Debug, Parser)]
struct SyncDownloadArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    group_scan: GroupScanDir,

    #[clap(flatten)]
    group_ffmpeg: GroupFFmpeg,

    #[clap(long, env = "SPOTDL_SYNC_DIR")]
    output_dir: Option<PathBuf>,

    #[clap(flatten)]
    manifest: GroupManifest,
}

/// Update metadata for all audio files in a directory.
#[derive(Debug, Parser)]
struct UpdateArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    #[clap(flatten)]
    group_auth: GroupAuth,

    /// Number of workers to use.
    #[clap(long, default_value = "4")]
    workers: usize,

    /// Replace existing metadata.
    #[clap(long)]
    replace: bool,

    /// Don't download images.
    #[clap(long)]
    no_images: bool,

    /// Directory to scan for files to update.
    path: PathBuf,
}

/// Interact with the cache.
#[derive(Debug, Parser)]
struct CacheArgs {
    #[clap(subcommand)]
    action: CacheAction,
}

#[derive(Debug, Parser)]
enum CacheAction {
    List(CacheListArgs),
    Clear(CacheClearArgs),
    Remove(CacheRemoveArgs),
}

/// List all cached keys.
#[derive(Debug, Parser)]
struct CacheListArgs {
    #[clap(flatten)]
    group_cache: GroupCache,
}

/// Clear the cache.
#[derive(Debug, Parser)]
struct CacheClearArgs {
    #[clap(flatten)]
    group_cache: GroupCache,
}

/// Remove a key from the cache.
#[derive(Debug, Parser)]
struct CacheRemoveArgs {
    #[clap(flatten)]
    group_cache: GroupCache,

    /// Key to remove.
    keys: Vec<String>,
}

/// Watch for track ids.
///
/// This command is useful when working with large music collections.
/// It allows you to track which tracks are present in a set of directories without have to rescan
/// them each time since that would be slow.
#[derive(Debug, Parser)]
struct WatcherArgs {
    #[clap(subcommand)]
    action: WatcherAction,
}

#[derive(Debug, Parser)]
enum WatcherAction {
    Watch(WatcherWatchArgs),
    List(WatcherListArgs),
    Contains(WatcherContainsArgs),
}

/// Watch a set of directories.
#[derive(Debug, Parser)]
struct WatcherWatchArgs {
    #[clap(flatten)]
    group_scan: GroupScanDir,

    #[clap(flatten)]
    group_socket: GroupWatchSocket,
}

/// List all track ids being watched.
#[derive(Debug, Parser)]
struct WatcherListArgs {
    #[clap(flatten)]
    group_socket: GroupWatchSocket,
}

/// Check if a track id is being watched.
#[derive(Debug, Parser)]
struct WatcherContainsArgs {
    #[clap(flatten)]
    group_socket: GroupWatchSocket,

    /// Track id to check.
    /// Can be a URL, URI or ID.
    track_id: String,
}

async fn subcmd_login(args: LoginArgs) -> Result<()> {
    let credentials_dir = args.group_cache.get_credentials_dir();
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
    let credentials_dir = args.group_cache.get_credentials_dir();
    let credentials_path = credentials_dir.join(args.username);
    if credentials_path.exists() {
        tokio::fs::remove_file(&credentials_path)
            .await
            .context("removing credentials")?;
    }
    Ok(())
}

async fn subcmd_info(args: InfoArgs) -> Result<()> {
    let rids = args.resource.resolve_rids().await?;
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let fetcher = helper_create_fetcher_delayed(credentials, &args.group_cache).await?;

    fn to_writer<T>(v: &mut Vec<u8>, value: &T, pretty: bool) -> Result<()>
    where
        T: Serialize,
    {
        if pretty {
            serde_json::to_writer_pretty(v, value)?;
        } else {
            serde_json::to_writer(v, value)?;
        }
        Ok(())
    }

    let mut output = Vec::new();
    if args.array {
        output.extend(b"[\n");
    }
    for rid in rids {
        match rid.resource {
            Resource::Artist => {
                let artist = fetcher.get_artist(rid.id).await?;
                to_writer(&mut output, &artist, args.pretty)?;
            }
            Resource::Album => {
                let album = fetcher.get_album(rid.id).await?;
                to_writer(&mut output, &album, args.pretty)?;
            }
            Resource::Track => {
                let track = fetcher.get_track(rid.id).await?;
                to_writer(&mut output, &track, args.pretty)?;
            }
            Resource::Playlist => {
                let playlist = fetcher.get_playlist(rid.id).await?;
                to_writer(&mut output, &playlist, args.pretty)?;
            }
        }

        output.extend(b"\n");
        tokio::io::stdout().write_all(&output).await?;
        output.clear();
        if args.array {
            output.extend(b",\n");
        }
    }

    if args.array {
        output.clear();
        output.extend(b"]\n");
        tokio::io::stdout().write_all(&output).await?;
    }

    Ok(())
}

async fn subcmd_download(args: DownloadArgs) -> Result<()> {
    let rids = args.resource.resolve_rids().await?;
    let sources = args.group_source.get_sources().context("getting sources")?;
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = helper_create_fetcher(session.clone(), &args.group_cache).await?;

    helper_download_rids(
        session,
        fetcher,
        args.group_ffmpeg,
        args.group_scan,
        args.output_dir,
        rids,
        sources,
    )
    .await?;

    tracing::info!("download complete");

    Ok(())
}

async fn subcmd_scan(args: ScanArgs) -> Result<()> {
    for track in spotdl::scan::scan(&args.dir).await? {
        println!("{} {}", track.id, track.path.display());
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

    fn resource_ids(&self) -> impl Iterator<Item = ResourceId> + '_ {
        self.0.iter().map(|entry| &entry.resource_id).copied()
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
    let rids = args
        .resource
        .resolve_rids()
        .await?
        .into_iter()
        .filter(|rid| !manifest.contains(rid))
        .collect::<Vec<_>>();
    if rids.is_empty() {
        return Ok(());
    }

    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = helper_create_fetcher(session.clone(), &args.group_cache).await?;

    for rid in rids {
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
    }

    helper_write_manifest(&args.manifest.manifest, &manifest).await?;

    Ok(())
}

async fn subcmd_sync_remove(args: SyncRemoveArgs) -> Result<()> {
    let mut manifest = helper_read_manifest(&args.manifest.manifest).await?;
    let rids = args.resource.resolve_rids().await?;
    for rid in rids {
        manifest.remove(&rid);
    }
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
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = helper_create_fetcher(session.clone(), &args.group_cache).await?;
    let rids = manifest.entries().map(|entry| entry.resource_id).collect();

    helper_download_rids(
        session,
        fetcher,
        args.group_ffmpeg,
        args.group_scan,
        args.output_dir.unwrap_or_else(|| PathBuf::from(".")),
        rids,
        vec![],
    )
    .await?;

    tracing::info!("sync complete");

    Ok(())
}

async fn subcmd_update(args: UpdateArgs) -> Result<()> {
    let files = helper_collect_files(&args.path).await?;
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = Arc::new(helper_create_fetcher(session.clone(), &args.group_cache).await?);

    let (tx, rx) = flume::bounded::<PathBuf>(args.workers);
    let mut handles = Vec::new();

    let fetch_metadata_params = spotdl::tag::FetchMetadataParams {
        download_images: !args.no_images,
    };

    for _ in 0..args.workers {
        let rx = rx.clone();
        let fetcher = fetcher.clone();
        let fetch_metadata_params = fetch_metadata_params.clone();
        let replace = args.replace;
        let handle = tokio::spawn(async move {
            while let Ok(file) = rx.recv_async().await {
                let update_result: Result<()> = try {
                    let id = match spotdl::scan::scan_file(&file).await {
                        Some(id) => id,
                        None => continue,
                    };

                    tracing::info!("updating metadata for {}", file.display());
                    let track = fetcher.get_track(id).await?;
                    let mut tag = if replace {
                        Default::default()
                    } else {
                        id3::Tag::read_from_path(&file).unwrap_or_default()
                    };
                    spotdl::tag::fetch_metadata_to_existing_tag_with(
                        &mut tag,
                        track.rid.id,
                        &*fetcher,
                        &fetch_metadata_params,
                    )
                    .await?;

                    if file.extension() == Some(OsStr::new("wav")) {
                        tag.write_to_wav_path(&file, id3::Version::Id3v24)
                            .with_context(|| {
                                format!(
                                    "failed to write metadata to file: {}",
                                    file.to_string_lossy()
                                )
                            })?;
                    } else {
                        tag.write_to_path(&file, id3::Version::Id3v24)
                            .with_context(|| {
                                format!(
                                    "failed to write metadata to file: {}",
                                    file.to_string_lossy()
                                )
                            })?;
                    }
                };

                match update_result {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::error!("failed to update metadata for {}: {}", file.display(), e);
                    }
                }
            }
        });
        handles.push(handle);
    }

    for file in files {
        tx.send_async(file).await?;
    }
    drop(tx);

    for handle in handles {
        handle.await?;
    }

    Ok(())
}

async fn subcmd_cache(args: CacheArgs) -> Result<()> {
    match args.action {
        CacheAction::List(args) => subcmd_cache_list(args).await,
        CacheAction::Clear(args) => subcmd_cache_clear(args).await,
        CacheAction::Remove(args) => subcmd_cache_remove(args).await,
    }
}

async fn subcmd_cache_list(args: CacheListArgs) -> Result<()> {
    let cache = helper_create_fs_cache(&args.group_cache);
    let entries = cache.list().await?;
    for entry in entries {
        println!("{}", entry);
    }
    Ok(())
}

async fn subcmd_cache_clear(args: CacheClearArgs) -> Result<()> {
    let cache = helper_create_fs_cache(&args.group_cache);
    cache.clear().await?;
    Ok(())
}

async fn subcmd_cache_remove(args: CacheRemoveArgs) -> Result<()> {
    let cache = helper_create_fs_cache(&args.group_cache);
    for key in args.keys {
        cache.remove(&key).await?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
enum WatcherRequest {
    List,
    Contains(SpotifyId),
}

#[derive(Debug)]
struct WatcherInner {
    paths: HashMap<PathBuf, SpotifyId>,
}

#[derive(Debug, Clone)]
struct Watcher(Arc<Mutex<WatcherInner>>);

impl Watcher {
    async fn new(params: ScanParams) -> Result<(Self, notify::INotifyWatcher)> {
        use notify::Watcher;

        let this = Self(Arc::new(Mutex::new(WatcherInner {
            paths: Default::default(),
        })));

        let mut watcher = notify::recommended_watcher({
            let params = params.clone();
            let watcher = this.clone();
            move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    if ev.need_rescan() {
                        // TODO: implement i guess
                        unimplemented!();
                    }
                    match ev.kind {
                        notify::EventKind::Create(_) | notify::EventKind::Modify(_) => {
                            for path in ev.paths {
                                if params.should_exclude(&path) {
                                    continue;
                                }
                                match futures::executor::block_on(spotdl::scan::scan_file(&path)) {
                                    Some(id) => {
                                        let mut inner = watcher.0.lock().unwrap();
                                        tracing::info!("found new item: {} {}", id, path.display());
                                        inner.paths.insert(path, id);
                                    }
                                    None => {
                                        let mut inner = watcher.0.lock().unwrap();
                                        if let Some(id) = inner.paths.remove(&path) {
                                            tracing::info!(
                                                "removed item: {} {}",
                                                id,
                                                path.display()
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        notify::EventKind::Remove(_) => {
                            let mut inner = watcher.0.lock().unwrap();
                            for path in ev.paths {
                                if let Some(id) = inner.paths.remove(&path) {
                                    tracing::info!("removed item: {} {}", id, path.display());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        })
        .context("failed to create watcher")?;

        for path in params.include.iter() {
            watcher
                .watch(&path, notify::RecursiveMode::Recursive)
                .context("failed to watch path")?;
        }

        tracing::debug!("scanning for existing files");
        let items = spotdl::scan::scan_with(params).await?;
        let mut inner = this.0.lock().unwrap();
        for item in items {
            inner.paths.insert(item.path, item.id);
        }
        drop(inner);
        tracing::debug!("done scanning for existing files");

        Ok((this, watcher))
    }

    fn contains(&self, track_id: SpotifyId) -> bool {
        self.0
            .lock()
            .unwrap()
            .paths
            .values()
            .any(|id| id == &track_id)
    }

    fn list(&self) -> Result<Vec<ScanItem>> {
        let inner = self.0.lock().unwrap();
        let mut items = Vec::with_capacity(inner.paths.len());
        for (path, id) in &inner.paths {
            items.push(ScanItem {
                path: path.clone(),
                id: *id,
            });
        }
        Ok(items)
    }
}

async fn subcmd_watcher(args: WatcherArgs) -> Result<()> {
    match args.action {
        WatcherAction::Watch(args) => subcmd_watcher_watch(args).await,
        WatcherAction::List(args) => subcmd_watcher_list(args).await,
        WatcherAction::Contains(args) => subcmd_watcher_contains(args).await,
    }
}

async fn subcmd_watcher_watch(args: WatcherWatchArgs) -> Result<()> {
    async fn handle_request(mut stream: UnixStream, watcher: Watcher) -> Result<()> {
        let request = {
            let mut buffer = Vec::new();
            let len = stream.read_u16().await?;
            buffer.resize(len as usize, 0);
            stream.read_exact(&mut buffer).await?;
            serde_json::from_slice::<WatcherRequest>(&buffer)?
        };

        match request {
            WatcherRequest::List => {
                tracing::debug!("handling list request");
                let mut writer = BufWriter::new(stream);
                let items = watcher.list()?;
                for item in items {
                    let line = format!("{} {}\n", item.id, item.path.display());
                    writer.write_all(line.as_bytes()).await?;
                }
                writer.flush().await?;
            }
            WatcherRequest::Contains(id) => {
                tracing::debug!("handling contains request");
                let mut writer = BufWriter::new(stream);
                let contains = watcher.contains(id);
                writer.write_i8(if contains { 1 } else { 0 }).await?;
                writer.flush().await?;
            }
        }

        Ok(())
    }

    let socket_path = args.group_socket.socket_path();
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path).await?;
    }
    let listener = UnixListener::bind(socket_path).context("failed to bind socket")?;
    let (watcher, _nofity) = Watcher::new(args.group_scan.create_params()).await?;

    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(err) => {
                tracing::error!("failed to accept connection: {}", err);
                // just in case this is constantly failing, sleep for a bit
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };

        tracing::debug!("accepted connection");
        tokio::spawn({
            let watcher = watcher.clone();
            async move {
                if let Err(err) = handle_request(stream, watcher).await {
                    tracing::error!("failed to handle request: {}", err);
                }
            }
        });
    }
}

async fn subcmd_watcher_list(args: WatcherListArgs) -> Result<()> {
    let socket_path = args.group_socket.socket_path();
    let mut stream = UnixStream::connect(socket_path)
        .await
        .context("failed to connect to socket")?;
    let request = WatcherRequest::List;
    let request = serde_json::to_vec(&request)?;
    stream.write_u16(request.len() as u16).await?;
    stream.write_all(&request).await?;
    tokio::io::copy(&mut stream, &mut tokio::io::stdout()).await?;
    Ok(())
}

async fn subcmd_watcher_contains(args: WatcherContainsArgs) -> Result<()> {
    let id = spotdl::id::parse(&args.track_id, Some(Resource::Track))
        .context("failed to parse key")?
        .id;
    let socket_path = args.group_socket.socket_path();
    let mut stream = UnixStream::connect(socket_path)
        .await
        .context("failed to connect to socket")?;
    let request = WatcherRequest::Contains(id);
    let request = serde_json::to_vec(&request)?;
    stream.write_u16(request.len() as u16).await?;
    stream.write_all(&request).await?;
    let contains = stream.read_i8().await?;
    if contains == 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn helper_download_rids<F>(
    session: Session,
    fetcher: F,
    ffmpeg: GroupFFmpeg,
    scan: GroupScanDir,
    output_dir: PathBuf,
    rids: Vec<ResourceId>,
    sources: Vec<PipelineSource>,
) -> Result<()>
where
    F: MetadataFetcher,
{
    const WORKERS: usize = 4;

    let fetcher = Arc::new(fetcher);
    let mut builder = PipelineBuilder::new(fetcher.clone());
    tracing::info!("scanning for existing files");
    let mut scan_params = scan.create_params();
    scan_params.include(&output_dir);
    tracing::debug!("scan params: {:?}", scan_params);
    let excluded = spotdl::scan::scan_with(scan_params).await?;
    tracing::info!("found {} existing tracks", excluded.len());
    builder
        .with_workers(WORKERS)
        .with_excluded_iter(
            excluded
                .into_iter()
                .map(|item| ResourceId::new(Resource::Track, item.id)),
        )
        .with_session(session)
        .with_stage(TagStage);
    if let Some(ffmpeg) = ffmpeg.create_stage() {
        builder.with_stage(ffmpeg);
    }
    builder.with_sources(sources);
    builder.with_stage(OrganizeStage::new(output_dir));

    let (input, events) = builder
        .build()
        .await
        .context("failed to build download pipeline")?;
    let handle = tokio::spawn(helper_download_rids_progress(events, fetcher, WORKERS));
    for rid in rids {
        input.send(rid).await;
    }
    input.close();
    handle.await??;
    Ok(())
}

async fn helper_download_rids_progress<F>(
    mut events: PipelineEvents,
    fetcher: Arc<F>,
    workers: usize,
) -> Result<()>
where
    F: MetadataFetcher,
{
    let disable_progress = std::env::var_os("SPOTDL_DISABLE_PROGRESS").is_some();
    if disable_progress {
        while events.recv().await.is_some() {}
    } else {
        let mp = indicatif::MultiProgress::new();

        // create progress bars
        let download_pb = indicatif::ProgressBar::new_spinner();
        let worker_pbs = (0..workers)
            .map(|_| mp.add(indicatif::ProgressBar::new_spinner()))
            .collect::<Vec<_>>();

        // register progress bars
        mp.add(download_pb.clone());
        for pb in &worker_pbs {
            mp.add(pb.clone());
        }

        download_pb.enable_steady_tick(Duration::from_millis(200));
        for pb in &worker_pbs {
            pb.enable_steady_tick(Duration::from_millis(200));
        }

        // set styles
        let style = ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )
        .unwrap()
        .progress_chars("##-");
        download_pb.set_style(style.clone());
        for pb in &worker_pbs {
            pb.set_style(style.clone());
        }

        let mut track_names = HashMap::new();
        let mut track_workers = HashMap::new();
        let mut avail_workers = (0..workers).rev().collect::<Vec<_>>();

        while let Some(ev) = events.recv().await {
            match ev {
                spotdl::pipeline::PipelineEvent::DownloadStarted { track_id } => {
                    let track = fetcher.get_track(track_id).await?;
                    let album = fetcher.get_album(track.album.id).await?;
                    let artist = fetcher.get_artist(track.artists[0].id).await?;
                    let name = format!("{}/{}/{}", artist.name, album.name, track.name);
                    track_names.insert(track_id, name.clone());

                    let message = format!("Downloading {}", name);
                    download_pb.set_message(message);
                    download_pb.set_length(100);
                    download_pb.set_position(0);
                }
                spotdl::pipeline::PipelineEvent::DownloadProgress { progress, .. } => {
                    download_pb.set_position((progress * 100.0) as u64);
                }
                spotdl::pipeline::PipelineEvent::DownloadFinished { track_id } => {
                    let name = &track_names[&track_id];
                    let message = format!("Finished downloading {name}");

                    download_pb.set_message(message);
                    download_pb.set_position(100);

                    let worker = avail_workers.pop().expect("no available workers");
                    track_workers.insert(track_id, worker);
                }
                spotdl::pipeline::PipelineEvent::PostProcessStarted {
                    track_id,
                    stage_count,
                } => {
                    let name = &track_names[&track_id];
                    let message = format!("Post-processing {}", name);
                    let pb = &worker_pbs[track_workers[&track_id]];
                    pb.set_message(message);
                    pb.set_position(0);
                    pb.set_length(stage_count as u64);
                }
                spotdl::pipeline::PipelineEvent::PostProcessProgress {
                    track_id,
                    stage,
                    stage_idx,
                    stage_count,
                } => {
                    let name = &track_names[&track_id];
                    let message = format!("Post-processing ({stage}) {name}");
                    let pb = &worker_pbs[track_workers[&track_id]];
                    pb.set_message(message);
                    pb.set_position(stage_idx as u64 + 1);
                    pb.set_length(stage_count as u64);
                }
                spotdl::pipeline::PipelineEvent::PostProcessFailed { track_id } => {
                    let name = &track_names[&track_id];
                    let message = format!("Failed to post-process {name}");
                    let pb = &worker_pbs[track_workers[&track_id]];
                    pb.set_message(message);
                    avail_workers.push(track_workers[&track_id]);
                }
                spotdl::pipeline::PipelineEvent::PostProcessFinished { track_id } => {
                    let name = &track_names[&track_id];
                    let message = format!("Finished post-processing {name}");
                    let pb = &worker_pbs[track_workers[&track_id]];
                    pb.set_message(message);
                    avail_workers.push(track_workers[&track_id]);
                }
            }
        }
    }

    Ok(())
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

fn helper_create_fs_cache(group_cache: &GroupCache) -> FsCache {
    FsCache::new(group_cache.get_metadata_dir())
}

async fn helper_create_fetcher(
    session: Session,
    group_cache: &GroupCache,
) -> Result<impl MetadataFetcher> {
    let fetcher = FsCacheMetadataFetcher::with(
        SpotifyMetadataFetcher::new(session.clone()),
        group_cache.get_metadata_dir(),
        group_cache.create_fs_fetcher_param(),
    )
    .await?;
    Ok(fetcher)
}

async fn helper_create_fetcher_delayed(
    credentials: Credentials,
    group_cache: &GroupCache,
) -> Result<impl MetadataFetcher> {
    let fetcher = FsCacheMetadataFetcher::with(
        SpotifyMetadataFetcher::delayed(credentials),
        group_cache.get_metadata_dir(),
        group_cache.create_fs_fetcher_param(),
    )
    .await?;
    Ok(fetcher)
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

async fn helper_get_credentials(
    group_cache: &GroupCache,
    group_auth: &GroupAuth,
) -> Result<Credentials> {
    let credentials_dir = group_cache.get_credentials_dir();
    tokio::fs::create_dir_all(&credentials_dir)
        .await
        .context("creating credentials dir")?;
    if let Some(username) = group_auth.username.clone() {
        let credentials_path = credentials_dir.join(&username);
        if credentials_path.is_file() && group_auth.password.is_none() {
            let credentials = helper_read_json(&credentials_path)
                .await
                .context("reading credentials")?;
            return Ok(credentials);
        } else {
            let credentials = spotdl::session::login(&LoginCredentials {
                username: username.clone(),
                password: group_auth
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
    let args = Args::parse();

    let filter = if (std::matches!(args.subcmd, SubCmd::Download(_))
        || std::matches!(
            args.subcmd,
            SubCmd::Sync(SyncArgs {
                action: SyncAction::Download(_)
            })
        ))
        && std::env::var_os("SPOTDL_DISABLE_PROGRESS").is_none()
    {
        EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new("error"))
            .unwrap()
    } else {
        EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new("warn,spotdl=info"))
            .unwrap()
    };

    tracing_subscriber::fmt::fmt()
        .with_env_filter(filter)
        .init();

    match args.subcmd {
        SubCmd::Login(args) => subcmd_login(args).await?,
        SubCmd::Logout(args) => subcmd_logout(args).await?,
        SubCmd::Info(args) => subcmd_info(args).await?,
        SubCmd::Download(args) => subcmd_download(args).await?,
        SubCmd::Scan(args) => subcmd_scan(args).await?,
        SubCmd::Sync(args) => subcmd_sync(args).await?,
        SubCmd::Update(args) => subcmd_update(args).await?,
        SubCmd::Cache(args) => subcmd_cache(args).await?,
        SubCmd::Watcher(args) => subcmd_watcher(args).await?,
    };

    Ok(())
}
