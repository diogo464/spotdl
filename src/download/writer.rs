use super::DownloadSink;

/// A download sink that writes to a `std::io::Write` implementor.
/// The samples are written as little-endian signed 16-bit integers.
/// The writer is not buffered by default.
#[derive(Debug)]
pub struct WriterDownloadSink<W> {
    writer: W,
}

impl<W> WriterDownloadSink<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W> DownloadSink for WriterDownloadSink<W>
where
    W: std::io::Write + Send + 'static,
{
    fn write_samples(&mut self, samples: &[i16]) -> std::io::Result<()> {
        for sample in samples {
            self.writer.write_all(&sample.to_le_bytes())?;
        }
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}
