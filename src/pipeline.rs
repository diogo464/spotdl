use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::{
    download::{self, DownloadSink},
    fetcher::MetadataFetcher,
    scan,
    session::Session,
    Resource, ResourceId, SpotifyId,
};

mod stage_ffmpeg;
pub use stage_ffmpeg::FFmpegStage;

mod stage_tag;
pub use stage_tag::TagStage;

mod stage_organize;
pub use stage_organize::OrganizeStage;

pub type PipelineInputSender = tokio::sync::mpsc::Sender<ResourceId>;
pub type PipelineInputReceiver = tokio::sync::mpsc::Receiver<ResourceId>;
pub type PipelineEventSender = tokio::sync::mpsc::Sender<PipelineEvent>;
pub type PipelineEventReceiver = tokio::sync::mpsc::Receiver<PipelineEvent>;
type PipelineDownloadSender = flume::Sender<SpotifyId>;
type PipelineDownloadReceiver = flume::Receiver<SpotifyId>;
type PipelinePostProcessSender = flume::Sender<DownloadArtifact>;
type PipelinePostProcessReceiver = flume::Receiver<DownloadArtifact>;
type PipelineStages<F> = Vec<Arc<dyn PipelineStage<F>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipelineSourceAction {
    Copy,
    SymLink,
    HardLink,
}

#[derive(Debug, Clone)]
pub struct PipelineSource {
    pub source: PathBuf,
    pub action: PipelineSourceAction,
}

pub enum PipelineEvent {
    DownloadStarted {
        track_id: SpotifyId,
    },
    DownloadProgress {
        track_id: SpotifyId,
        progress: f32,
    },
    DownloadFinished {
        track_id: SpotifyId,
    },
    PostProcessStarted {
        track_id: SpotifyId,
        stage_count: usize,
    },
    PostProcessProgress {
        track_id: SpotifyId,
        stage: String,
        stage_idx: usize,
        stage_count: usize,
    },
    PostProcessFailed {
        track_id: SpotifyId,
    },
    PostProcessFinished {
        track_id: SpotifyId,
    },
}

#[derive(Debug, Clone)]
pub struct StageArtifact {
    /// The track id that is being processed
    pub resource_id: ResourceId,
    /// The relative path to the stage working directory
    pub file_path: PathBuf,
}

#[async_trait::async_trait]
pub trait PipelineStage<F>
where
    Self: Send + Sync + 'static,
    F: MetadataFetcher,
{
    fn name(&self) -> &'static str;

    async fn process(
        &self,
        fetcher: &Arc<F>,
        work_dir: &Path,
        artifact: StageArtifact,
    ) -> std::io::Result<PathBuf>;
}

#[derive(Debug, Clone)]
pub struct PipelineInput {
    sender: PipelineInputSender,
}

impl PipelineInput {
    pub async fn send(&self, rid: ResourceId) {
        let _ = self.sender.send(rid).await;
    }

    pub fn close(self) {
        let _ = self;
    }
}

#[derive(Debug)]
pub struct PipelineEvents {
    receiver: PipelineEventReceiver,
}

impl PipelineEvents {
    pub async fn recv(&mut self) -> Option<PipelineEvent> {
        self.receiver.recv().await
    }

    pub fn recv_blocking(&mut self) -> Option<PipelineEvent> {
        self.receiver.blocking_recv()
    }
}

pub struct PipelineBuilder<F> {
    fetcher: Arc<F>,
    sessions: Vec<Session>,
    excluded: HashSet<ResourceId>,
    sources: Vec<PipelineSource>,
    stages: Vec<Arc<dyn PipelineStage<F>>>,
    workers: usize,
}
impl<F> PipelineBuilder<F> {
    pub fn new(fetcher: Arc<F>) -> Self {
        Self {
            fetcher,
            sessions: Default::default(),
            excluded: Default::default(),
            sources: Default::default(),
            stages: Default::default(),
            workers: 1,
        }
    }
}

struct DownloadArtifact {
    pub resource_id: ResourceId,
    pub working_dir: tempfile::TempDir,
    pub file_path: PathBuf,
}

#[derive(Default)]
struct SourceMap {
    map: HashMap<SpotifyId, (PipelineSourceAction, PathBuf)>,
}

impl SourceMap {
    async fn scan_dir(&mut self, path: &Path, action: PipelineSourceAction) -> std::io::Result<()> {
        for item in scan::scan(path).await? {
            self.map.insert(item.id, (action, item.path));
        }
        Ok(())
    }
}

impl<F> PipelineBuilder<F>
where
    F: MetadataFetcher,
{
    pub fn with_session(&mut self, session: Session) -> &mut Self {
        self.sessions.push(session);
        self
    }

    pub fn with_stage(&mut self, stage: impl PipelineStage<F>) -> &mut Self {
        self.stages.push(Arc::new(stage));
        self
    }

    pub fn with_excluded(&mut self, resource_id: ResourceId) -> &mut Self {
        self.excluded.insert(resource_id);
        self
    }

    pub fn with_excluded_iter(
        &mut self,
        resource_ids: impl IntoIterator<Item = ResourceId>,
    ) -> &mut Self {
        self.excluded.extend(resource_ids);
        self
    }

    pub fn with_workers(&mut self, workers: usize) -> &mut Self {
        self.workers = workers;
        self
    }

    pub fn with_source(&mut self, source: PipelineSource) -> &mut Self {
        self.sources.push(source);
        self
    }

    pub fn with_sources(&mut self, sources: impl IntoIterator<Item = PipelineSource>) -> &mut Self {
        self.sources.extend(sources);
        self
    }

    pub async fn build(self) -> std::io::Result<(PipelineInput, PipelineEvents)> {
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(128);
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(128);
        let (exclude_tx, exclude_rx) = flume::bounded(128);
        let (download_tx, download_rx) = flume::bounded(0);
        let (postprocess_tx, postprocess_rx) = flume::bounded(0);
        let fetcher = self.fetcher;

        let mut source_map = SourceMap::default();
        for source in self.sources {
            source_map.scan_dir(&source.source, source.action).await?;
        }
        let source_map = Arc::new(source_map);

        tokio::spawn(worker_fetcher(input_rx, fetcher.clone(), exclude_tx));
        tokio::spawn(worker_exclude(
            exclude_rx,
            download_tx,
            fetcher.clone(),
            self.excluded,
        ));
        for session in self.sessions {
            tokio::spawn(worker_download(
                download_rx.clone(),
                session,
                fetcher.clone(),
                source_map.clone(),
                postprocess_tx.clone(),
                event_tx.clone(),
            ));
        }
        for _ in 0..self.workers {
            tokio::spawn(worker_postprocess(
                postprocess_rx.clone(),
                fetcher.clone(),
                self.stages.clone(),
                event_tx.clone(),
            ));
        }

        Ok((
            PipelineInput { sender: input_tx },
            PipelineEvents { receiver: event_rx },
        ))
    }
}

#[derive(Debug)]
struct EventSinkInner {
    track_id: SpotifyId,
    total_samples: usize,
    last_event: usize,
    buffer: Vec<i16>,
    sender: PipelineEventSender,
}

#[derive(Debug, Clone)]
struct EventSink(Arc<Mutex<EventSinkInner>>);

impl EventSink {
    fn new(track_id: SpotifyId, total_samples: usize, sender: PipelineEventSender) -> Self {
        Self(Arc::new(Mutex::new(EventSinkInner {
            track_id,
            total_samples,
            last_event: 0,
            buffer: Vec::new(),
            sender,
        })))
    }

    fn take_buffer(&self) -> Vec<i16> {
        let mut inner = self.0.lock().unwrap();
        std::mem::take(&mut inner.buffer)
    }
}

impl DownloadSink for EventSink {
    fn write_samples(&mut self, samples: &[i16]) -> std::io::Result<()> {
        let mut inner = self.0.lock().unwrap();
        inner.buffer.extend_from_slice(samples);
        let buffer_len = inner.buffer.len();
        if buffer_len - inner.last_event > 10_000 {
            let total_samples = inner.total_samples;
            let track_id = inner.track_id;
            let progress = buffer_len as f32 / total_samples as f32;
            let sender = inner.sender.clone();
            tokio::spawn(async move {
                evs(
                    &sender,
                    PipelineEvent::DownloadProgress { track_id, progress },
                )
                .await;
            });
        }
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        let inner = self.0.lock().unwrap();
        let buffer_len = inner.buffer.len();
        let total_samples = inner.total_samples;
        let track_id = inner.track_id;
        let progress = buffer_len as f32 / total_samples as f32;
        let sender = inner.sender.clone();
        tokio::spawn(async move {
            evs(
                &sender,
                PipelineEvent::DownloadProgress { track_id, progress },
            )
            .await;
            evs(&sender, PipelineEvent::DownloadFinished { track_id }).await;
        });
        Ok(())
    }
}

async fn worker_fetcher<F>(
    mut rx: PipelineInputReceiver,
    fetcher: Arc<F>,
    tx: PipelineDownloadSender,
) where
    F: MetadataFetcher,
{
    use anyhow::Context;

    while let Some(rid) = rx.recv().await {
        tracing::debug!("worker_fetcher: {rid}");
        let process_result: anyhow::Result<()> = try {
            match rid.resource {
                Resource::Artist => {
                    let artist = fetcher
                        .get_artist(rid.id)
                        .await
                        .context("Failed to fetch artist")?;
                    let albums = artist.albums.into_iter().chain(artist.singles.into_iter());
                    for album in albums {
                        let album = fetcher
                            .get_album(album.id)
                            .await
                            .context("Failed to fetch album")?;
                        for disc in album.discs {
                            for track in disc.tracks {
                                tx.send_async(track.id).await?;
                            }
                        }
                    }
                }
                Resource::Album => {
                    let album = fetcher
                        .get_album(rid.id)
                        .await
                        .context("Failed to fetch album")?;
                    for disc in album.discs {
                        for track in disc.tracks {
                            tx.send_async(track.id).await?;
                        }
                    }
                }
                Resource::Track => {
                    tx.send_async(rid.id).await?;
                }
                Resource::Playlist => {
                    let playlist = fetcher
                        .get_playlist(rid.id)
                        .await
                        .context("Failed to fetch playlist")?;
                    for track in playlist.tracks {
                        tx.send_async(track.id).await?;
                    }
                }
            }
        };

        if let Err(e) = process_result {
            tracing::error!("Failed to process resource {}: {}", rid, e);
        }
    }
}

async fn worker_exclude<F>(
    rx: PipelineDownloadReceiver,
    tx: PipelineDownloadSender,
    fetcher: Arc<F>,
    excluded: HashSet<ResourceId>,
) where
    F: MetadataFetcher,
{
    // TODO: exclude is a set of ResourceId but we only support excluding tracks right now
    // this could be moved to the step before
    while let Ok(track_id) = rx.recv_async().await {
        let rid = ResourceId::new(Resource::Track, track_id);
        if !excluded.contains(&rid) {
            tracing::debug!("worker_exclude: allowing {}", rid);
            let track = match fetcher.get_track(track_id).await {
                Ok(track) => track,
                Err(err) => {
                    tracing::error!("Failed to fetch track {}: {}", track_id, err);
                    continue;
                }
            };
            for alternative_id in track.alternatives {
                if excluded.contains(&alternative_id) {
                    tracing::debug!(
                        "worker_exclude: excluding alternative to {} (alternative = {})",
                        rid,
                        alternative_id,
                    );
                    continue;
                }
            }
            tx.send_async(track_id).await.unwrap();
        }
    }
}

async fn worker_download<F>(
    rx: PipelineDownloadReceiver,
    session: Session,
    fetcher: Arc<F>,
    source_map: Arc<SourceMap>,
    tx: PipelinePostProcessSender,
    ev: PipelineEventSender,
) where
    F: MetadataFetcher,
{
    use anyhow::Context;

    while let Ok(track_id) = rx.recv_async().await {
        tracing::info!("worker_download: {:?}", track_id);
        evs(&ev, PipelineEvent::DownloadStarted { track_id }).await;

        // TODO: organize this a bit more and improve error handling
        let (tempdir, file_path) = match source_map.map.get(&track_id) {
            // Copy/SymLink/HardLink an existing track
            Some((PipelineSourceAction::Copy, path)) => {
                let tempdir = tempfile::tempdir().expect("Failed to create temporary directory");
                let filename = path.file_name().expect("Failed to get filename");
                let output_path = tempdir.path().join(filename);
                tokio::fs::copy(path, &output_path)
                    .await
                    .context("Failed to copy file")
                    .unwrap();
                (tempdir, output_path)
            }
            Some((PipelineSourceAction::SymLink, path)) => {
                let tempdir = tempfile::tempdir().expect("Failed to create temporary directory");
                let filename = path.file_name().expect("Failed to get filename");
                let output_path = tempdir.path().join(filename);
                tokio::fs::symlink(path, &output_path)
                    .await
                    .context("Failed to symlink file")
                    .unwrap();
                (tempdir, output_path)
            }
            Some((PipelineSourceAction::HardLink, path)) => {
                let home = std::env::var("HOME").expect("Failed to get home directory");
                let cache = format!("{home}/.cache/spotdl/pipeline");
                tokio::fs::create_dir_all(&cache)
                    .await
                    .context("Failed to create cache directory")
                    .unwrap();
                let tempdir =
                    tempfile::tempdir_in(&cache).expect("Failed to create temporary directory");
                let filename = path.file_name().expect("Failed to get filename");
                let output_path = tempdir.path().join(filename);
                tokio::fs::hard_link(path, &output_path)
                    .await
                    .context("Failed to hard link file")
                    .unwrap();
                (tempdir, output_path)
            }

            // Download the track if it's not already in the source map
            None => {
                let tempdir = tempfile::tempdir().expect("Failed to create temporary directory");
                let track = fetcher.get_track(track_id).await.unwrap();
                let total_samples = (track.duration.as_secs_f32()
                    * download::SAMPLE_RATE as f32
                    * download::NUM_CHANNELS as f32) as usize;
                let event_sink = EventSink::new(track_id, total_samples, ev.clone());
                if let Err(err) = download::download(&session, event_sink.clone(), track_id).await {
                    tracing::error!("Failed to download track {}: {}", track_id, err);
                    continue;
                }
                let samples = event_sink.take_buffer();

                let header = wav::Header::new(
                    wav::WAV_FORMAT_PCM,
                    download::NUM_CHANNELS as u16,
                    download::SAMPLE_RATE,
                    download::BITS_PER_SAMPLE as u16,
                );

                let output_path = tempdir.path().join("output.wav");
                let handle = tokio::task::spawn_blocking({
                    let output_path = output_path.clone();
                    move || -> anyhow::Result<()> {
                        let mut output_file = std::fs::File::create(&output_path).unwrap();
                        wav::write(header, &wav::BitDepth::Sixteen(samples), &mut output_file)
                            .context("writing wav file")?;
                        Ok(())
                    }
                });

                if let Err(err) = handle.await.expect("task should not panic") {
                    tracing::error!("Failed to write wav file: {}", err);
                    continue;
                }

                (tempdir, output_path)
            }
        };

        evs(&ev, PipelineEvent::DownloadFinished { track_id }).await;

        let artifact = DownloadArtifact {
            resource_id: ResourceId {
                resource: Resource::Track,
                id: track_id,
            },
            working_dir: tempdir,
            file_path,
        };

        tx.send_async(artifact)
            .await
            .expect("postprocess worker should not be dropped");
    }
}

async fn worker_postprocess<F>(
    rx: PipelinePostProcessReceiver,
    fetcher: Arc<F>,
    stages: PipelineStages<F>,
    ev: PipelineEventSender,
) where
    F: MetadataFetcher,
{
    while let Ok(download_artifact) = rx.recv_async().await {
        evs(
            &ev,
            PipelineEvent::PostProcessStarted {
                track_id: download_artifact.resource_id.id,
                stage_count: stages.len(),
            },
        )
        .await;

        let mut most_recent_path = download_artifact.file_path;
        'stages: for (idx, stage) in stages.iter().enumerate() {
            tracing::debug!(
                "starting stage '{}' for {}",
                stage.name(),
                download_artifact.resource_id
            );

            evs(
                &ev,
                PipelineEvent::PostProcessProgress {
                    track_id: download_artifact.resource_id.id,
                    stage: stage.name().to_string(),
                    stage_idx: idx,
                    stage_count: stages.len(),
                },
            )
            .await;

            let process_result = stage
                .process(
                    &fetcher,
                    download_artifact.working_dir.path(),
                    StageArtifact {
                        resource_id: download_artifact.resource_id,
                        file_path: most_recent_path,
                    },
                )
                .await;

            match process_result {
                Ok(path) => {
                    most_recent_path = path;
                }
                Err(err) => {
                    tracing::error!("Failed to process artifact: {}", err);
                    evs(
                        &ev,
                        PipelineEvent::PostProcessFailed {
                            track_id: download_artifact.resource_id.id,
                        },
                    )
                    .await;
                    break 'stages;
                }
            }
        }

        evs(
            &ev,
            PipelineEvent::PostProcessFinished {
                track_id: download_artifact.resource_id.id,
            },
        )
        .await;
    }
}

async fn evs(s: &PipelineEventSender, ev: PipelineEvent) {
    match ev {
        PipelineEvent::DownloadStarted { track_id } => tracing::info!("Downloading {}", track_id),
        PipelineEvent::DownloadFinished { track_id } => {
            tracing::info!("Finished downloading {}", track_id)
        }
        PipelineEvent::PostProcessStarted {
            track_id,
            stage_count,
        } => tracing::info!("Postprocessing {} ({} stages)", track_id, stage_count),
        PipelineEvent::PostProcessFailed { track_id } => {
            tracing::error!("Failed to postprocess {}", track_id)
        }
        PipelineEvent::PostProcessFinished { track_id } => {
            tracing::info!("Finished postprocessing {}", track_id)
        }
        _ => {}
    }
    if let Err(err) = s.send(ev).await {
        tracing::error!("Failed to send event: {}", err);
    }
}
