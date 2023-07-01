#![feature(io_error_other)]
#![feature(try_blocks)]

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use indicatif::ProgressStyle;
use serde::{de::DeserializeOwned, Serialize};
use spotdl::{
    fetcher::{FsCacheMetadataFetcher, MetadataFetcher, SpotifyMetadataFetcher},
    pipeline::{FFmpegStage, OrganizeStage, PipelineBuilder, PipelineEvents, TagStage},
    scan::ScanParams,
    session::Session,
    Credentials, LoginCredentials, Resource, ResourceId,
};
use tokio::io::AsyncWriteExt;

// TODO: remove some unwraps

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    subcmd: SubCmd,
}

#[derive(Debug, Parser)]
struct GroupCacheDir {
    /// Cache directory.
    ///
    /// This directory will be used to save login credentials and metadata from spotify api
    /// requests.
    #[clap(long, env = "SPOTDL_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
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
    #[clap(long, default_value = ".")]
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
    ///
    /// This can be a Spotify URI, URL or ID.
    /// Ex:
    ///     - spotify:track:5ujWPN9c4lh8pohnf4iVYK
    ///     - https://open.spotify.com/track/5ujWPN9c4lh8pohnf4iVYK
    ///     - 5ujWPN9c4lh8pohnf4iVYK
    #[clap(verbatim_doc_comment)]
    id: String,
}

/// Login to spotify.
#[derive(Debug, Parser)]
struct LoginArgs {
    #[clap(flatten)]
    group_cache: GroupCacheDir,

    /// Spotify username
    username: String,

    /// Spotify password
    password: String,
}

/// Logout from spotify.
#[derive(Debug, Parser)]
struct LogoutArgs {
    #[clap(flatten)]
    group_cache: GroupCacheDir,

    username: String,
}

/// Get information about a resource.
#[derive(Debug, Parser)]
struct InfoArgs {
    #[clap(flatten)]
    group_cache: GroupCacheDir,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    resource: ResourceSelector,
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
    group_cache: GroupCacheDir,

    #[clap(flatten)]
    group_auth: GroupAuth,

    #[clap(flatten)]
    group_ffmpeg: GroupFFmpeg,

    #[clap(flatten)]
    group_scan: GroupScanDir,

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
    group_cache: GroupCacheDir,

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
    group_cache: GroupCacheDir,

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
    group_cache: GroupCacheDir,

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
    group_cache: GroupCacheDir,

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

async fn subcmd_login(args: LoginArgs) -> Result<()> {
    let credentials_dir = helper_get_credentials_dir(&args.group_cache)?;
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
    let credentials_dir = helper_get_credentials_dir(&args.group_cache)?;
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
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let fetcher = FsCacheMetadataFetcher::new(
        SpotifyMetadataFetcher::delayed(credentials),
        helper_get_metadata_dir(&args.group_cache)?,
    )
    .await?;

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
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = FsCacheMetadataFetcher::new(
        SpotifyMetadataFetcher::new(session.clone()),
        helper_get_metadata_dir(&args.group_cache)?,
    )
    .await?;

    helper_download_rids(
        session,
        fetcher,
        args.group_ffmpeg,
        args.group_scan,
        args.output_dir,
        vec![rid],
    )
    .await?;

    tracing::info!("download complete");

    Ok(())
}

async fn subcmd_scan(args: ScanArgs) -> Result<()> {
    for track in spotdl::scan::scan(&args.dir).await? {
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

    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = FsCacheMetadataFetcher::new(
        SpotifyMetadataFetcher::new(session.clone()),
        helper_get_metadata_dir(&args.group_cache)?,
    )
    .await?;

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
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = FsCacheMetadataFetcher::new(
        SpotifyMetadataFetcher::new(session.clone()),
        helper_get_metadata_dir(&args.group_cache)?,
    )
    .await?;
    let rids = manifest.entries().map(|entry| entry.resource_id).collect();

    helper_download_rids(
        session,
        fetcher,
        args.group_ffmpeg,
        args.group_scan,
        args.output_dir.unwrap_or_else(|| PathBuf::from(".")),
        rids,
    )
    .await?;

    tracing::info!("sync complete");

    Ok(())
}

async fn subcmd_update(args: UpdateArgs) -> Result<()> {
    let files = helper_collect_files(&args.path).await?;
    let credentials = helper_get_credentials(&args.group_cache, &args.group_auth).await?;
    let session = Session::connect(credentials).await?;
    let fetcher = Arc::new(
        FsCacheMetadataFetcher::new(
            SpotifyMetadataFetcher::new(session.clone()),
            helper_get_metadata_dir(&args.group_cache)?,
        )
        .await?,
    );

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
                    let prev_tag = tag.clone();
                    spotdl::tag::fetch_metadata_to_existing_tag_with(
                        &mut tag,
                        track.rid.id,
                        &*fetcher,
                        &fetch_metadata_params,
                    )
                    .await?;
                    if tag == prev_tag {
                        tracing::debug!("no metadata changes for {}", file.display());
                        continue;
                    }
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

async fn helper_download_rids<F>(
    session: Session,
    fetcher: F,
    ffmpeg: GroupFFmpeg,
    scan: GroupScanDir,
    output_dir: PathBuf,
    rids: Vec<ResourceId>,
) -> Result<()>
where
    F: MetadataFetcher,
{
    const WORKERS: usize = 4;

    let fetcher = Arc::new(fetcher);
    let mut builder = PipelineBuilder::new(fetcher.clone());
    tracing::info!("scanning for existing files");
    tracing::debug!("scan params: {:?}", scan.create_params());
    let excluded = spotdl::scan::scan_with(scan.create_params()).await?;
    tracing::info!("found {} existing tracks", excluded.len());
    builder
        .with_workers(WORKERS)
        .with_excluded_iter(
            excluded
                .into_iter()
                .map(|id| ResourceId::new(Resource::Track, id)),
        )
        .with_session(session)
        .with_stage(TagStage);
    if let Some(ffmpeg) = ffmpeg.create_stage() {
        builder.with_stage(ffmpeg);
    }
    builder.with_stage(OrganizeStage::new(output_dir));

    let (input, events) = builder.build();
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
        while let Some(_) = events.recv().await {}
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

fn helper_get_cache_dir(dir: &GroupCacheDir) -> Result<PathBuf> {
    match dir.cache_dir {
        Some(ref dir) => Ok(dir.clone()),
        None => {
            let home = std::env::var("HOME").context("getting $HOME")?;
            let dir = PathBuf::from(home).join(".cache/spotdl");
            Ok(dir)
        }
    }
}

fn helper_get_credentials_dir(dir: &GroupCacheDir) -> Result<PathBuf> {
    let cache_dir = helper_get_cache_dir(dir)?;
    Ok(cache_dir.join("credentials"))
}

fn helper_get_metadata_dir(dir: &GroupCacheDir) -> Result<PathBuf> {
    let cache_dir = helper_get_cache_dir(dir)?;
    Ok(cache_dir.join("metadata"))
}

async fn helper_get_credentials(dir: &GroupCacheDir, auth: &GroupAuth) -> Result<Credentials> {
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
