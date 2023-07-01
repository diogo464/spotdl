use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::fetcher::MetadataFetcher;

use super::{PipelineStage, StageArtifact};

#[derive(Debug)]
pub struct FFmpegStage {
    ffmpeg_path: PathBuf,
    format: String,
}

impl FFmpegStage {
    pub fn new(ffmpeg_path: impl Into<PathBuf>, format: impl Into<String>) -> Self {
        Self {
            ffmpeg_path: ffmpeg_path.into(),
            format: format.into(),
        }
    }
}

#[async_trait::async_trait]
impl<F> PipelineStage<F> for FFmpegStage
where
    F: MetadataFetcher,
{
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    async fn process(
        &self,
        _fetcher: &Arc<F>,
        work_dir: &Path,
        artifact: StageArtifact,
    ) -> std::io::Result<PathBuf> {
        if let Some(ext) = artifact.file_path.extension() {
            if ext.to_str() == Some(self.format.as_str()) {
                tracing::debug!(
                    "{} already has the correct format, not converting",
                    artifact.file_path.display()
                );
                return Ok(artifact.file_path);
            }
        }

        let mut ffmpeg = tokio::process::Command::new(&self.ffmpeg_path);
        let output_path = work_dir.join("stage_ffmpeg").with_extension(&self.format);
        tracing::debug!(
            "ffmpeg stage converting {} to {}",
            artifact.file_path.display(),
            output_path.display()
        );
        ffmpeg
            .arg("-i")
            .arg(&artifact.file_path)
            .arg("-b:a")
            .arg("320k")
            .arg("-y")
            .arg(&output_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = ffmpeg.output().await?;
        if !output.status.success() {
            tracing::error!(
                "ffmpeg stdout:\n{}",
                String::from_utf8_lossy(&output.stdout)
            );
            tracing::error!(
                "ffmpeg stderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(std::io::Error::other("ffmpeg failed"));
        }

        Ok(output_path)
    }
}
