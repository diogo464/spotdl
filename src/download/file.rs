use std::{fs::File, io::BufWriter, path::Path};

use super::{DownloadSink, WriterDownloadSink};

#[derive(Debug)]
pub struct FileDownloadSink(WriterDownloadSink<BufWriter<File>>);

impl FileDownloadSink {
    /// The file will be created if it does not exist.
    /// If the file already exists, it will be overwritten.
    pub fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self::from_file(file))
    }

    pub fn from_file(file: std::fs::File) -> Self {
        Self(WriterDownloadSink::new(BufWriter::new(file)))
    }
}

impl DownloadSink for FileDownloadSink {
    fn start(&mut self) -> std::io::Result<()> {
        self.0.start()
    }

    fn write_samples(&mut self, samples: &[i16]) -> std::io::Result<()> {
        self.0.write_samples(samples)
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.0.finish()
    }
}
