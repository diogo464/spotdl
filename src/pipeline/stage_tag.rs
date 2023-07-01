use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{fetcher::MetadataFetcher, tag};

use super::{PipelineStage, StageArtifact};

#[derive(Debug, Default)]
pub struct TagStage;

impl TagStage {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl<F> PipelineStage<F> for TagStage
where
    F: MetadataFetcher,
{
    fn name(&self) -> &'static str {
        "tag"
    }

    async fn process(
        &self,
        fetcher: &Arc<F>,
        _work_dir: &Path,
        artifact: StageArtifact,
    ) -> std::io::Result<PathBuf> {
        if id3::Tag::read_from_path(&artifact.file_path).is_ok() {
            tracing::debug!(
                "{} already has a tag, not modifying metadata...",
                artifact.file_path.display()
            );
            return Ok(artifact.file_path);
        }

        let tag = match tag::fetch_metadata_to_tag(artifact.resource_id.id, &**fetcher).await {
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

        tracing::debug!("writing tag to {}", artifact.file_path.display());
        tracing::trace!("tag: {:?}", tag);

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
