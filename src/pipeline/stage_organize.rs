use std::path::{Path, PathBuf};

use crate::metadata::MetadataFetcher;

use super::{PipelineStage, StageArtifact};

#[derive(Debug)]
pub struct OrganizeStage {
    output_dir: PathBuf,
}

impl OrganizeStage {
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
        }
    }
}

#[async_trait::async_trait]
impl PipelineStage for OrganizeStage {
    fn name(&self) -> &'static str {
        "organize"
    }

    async fn process(
        &self,
        fetcher: &MetadataFetcher,
        _work_dir: &Path,
        artifact: StageArtifact,
    ) -> std::io::Result<PathBuf> {
        let organize_result: anyhow::Result<PathBuf> = try {
            let track = fetcher.get_track(artifact.resource_id.id).await?;
            let album = fetcher.get_album(track.album.id).await?;
            let artist = fetcher.get_artist(track.artists[0].id).await?;

            let mut output_path = self.output_dir.clone();
            let track_name = track.name.replace("/", "-");
            output_path.push(&artist.name);
            output_path.push(&album.name);
            if album.discs.len() > 1 {
                output_path.push(format!("Disc {}", track.disc_number));
            }
            output_path.push(format!("{} - {}", track.track_number, track_name));
            if let Some(ext) = artifact.file_path.extension() {
                output_path.set_extension(ext);
            }

            if let Some(parent) = output_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            match tokio::fs::rename(&artifact.file_path, &output_path).await {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
                    tokio::fs::copy(&artifact.file_path, &output_path).await?;
                }
                Err(err) => return Err(err),
            }

            output_path
        };

        match organize_result {
            Ok(output_path) => Ok(output_path),
            Err(err) => {
                tracing::error!("failed to organize {}: {}", artifact.resource_id, err);
                Err(std::io::Error::other("failed to organize"))
            }
        }
    }
}
