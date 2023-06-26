use std::sync::{Arc, Mutex};

use super::DownloadSink;

#[derive(Debug, Default)]
struct Inner {
    buffer: Vec<i16>,
}

#[derive(Debug, Default, Clone)]
pub struct MemoryDownloadSink(Arc<Mutex<Inner>>);

impl MemoryDownloadSink {
    pub fn take_buffer(&self) -> Vec<i16> {
        let mut inner = self.0.lock().unwrap();
        std::mem::take(&mut inner.buffer)
    }
}

impl DownloadSink for MemoryDownloadSink {
    fn start(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn write_samples(&mut self, samples: &[i16]) -> std::io::Result<()> {
        let mut inner = self.0.lock().unwrap();
        inner.buffer.extend_from_slice(samples);
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
