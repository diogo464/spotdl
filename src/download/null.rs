use super::DownloadSink;

pub struct NullDownloadSink;

impl DownloadSink for NullDownloadSink {
    fn start(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn write_samples(&mut self, _samples: &[i16]) -> std::io::Result<()> {
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
