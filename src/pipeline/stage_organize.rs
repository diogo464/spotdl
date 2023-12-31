use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::fetcher::MetadataFetcher;

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
impl<F> PipelineStage<F> for OrganizeStage
where
    F: MetadataFetcher,
{
    fn name(&self) -> &'static str {
        "organize"
    }

    async fn process(
        &self,
        fetcher: &Arc<F>,
        _work_dir: &Path,
        artifact: StageArtifact,
    ) -> std::io::Result<PathBuf> {
        let organize_result: anyhow::Result<PathBuf> = try {
            let track = fetcher.get_track(artifact.resource_id.id).await?;
            let album = fetcher.get_album(track.album.id).await?;
            let artist = fetcher.get_artist(track.artists[0].id).await?;

            let output_dir = {
                let mut output_dir = self.output_dir.clone();
                output_dir.push(&artist.name);
                output_dir.push(&album.name);
                if album.discs.len() > 1 {
                    output_dir.push(format!("Disc {}", track.disc_number));
                }
                output_dir
            };
            tokio::fs::create_dir_all(&output_dir).await?;

            let output_path = {
                let track_name = track.name.replace('/', "-").replace('.', "");

                let mut output_path = output_dir.clone();
                output_path.push(format!("{} - {}", track.track_number, track_name));
                if let Some(ext) = artifact.file_path.extension() {
                    output_path.set_extension(ext);
                }

                if output_path.exists() {
                    tracing::warn!(
                        "file already exists at {}, renaming...",
                        output_path.display()
                    );

                    let new_name =
                        format!("{} - {} - {}", track.track_number, track_name, track.rid.id);
                    output_path = output_dir.clone();
                    output_path.push(&new_name);
                    if let Some(ext) = artifact.file_path.extension() {
                        output_path.set_extension(ext);
                    }

                    tracing::debug!("renaming file using\n\tnew: {}\n\ttrack_number = {}\n\ttrack_name = {}\n\ttrack_id = {}\n\tnew_name = {}",output_path.display(),track.track_number,track_name,track.rid.id,new_name);
                }

                output_path
            };

            tracing::debug!(
                "organizing {} to {}",
                artifact.file_path.display(),
                output_path.display()
            );
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
