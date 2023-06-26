use librespot::playback::{
    audio_backend::{Sink, SinkError, SinkResult},
    config::{Bitrate, PlayerConfig},
    mixer::NoOpVolume,
    player::Player,
};

mod null;
pub use null::NullDownloadSink;

mod memory;
pub use memory::MemoryDownloadSink;

use crate::{session::Session, Resource, ResourceId, SpotifyId};

pub const SAMPLE_RATE: u32 = librespot::playback::SAMPLE_RATE as u32;
pub const NUM_CHANNELS: u32 = librespot::playback::NUM_CHANNELS as u32;
pub const BITS_PER_SAMPLE: u32 = 16;

type ErrSender = tokio::sync::oneshot::Sender<Option<std::io::Error>>;
type ErrReceiver = tokio::sync::oneshot::Receiver<Option<std::io::Error>>;

pub trait DownloadSink: Send + 'static {
    fn start(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn write_samples(&mut self, samples: &[i16]) -> std::io::Result<()>;
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct DownloadSinkWrapper<S> {
    download_sink: S,
    sender: Option<ErrSender>,
}

impl<S> DownloadSinkWrapper<S> {
    pub fn new(sink: S) -> (Self, ErrReceiver) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            Self {
                download_sink: sink,
                sender: Some(tx),
            },
            rx,
        )
    }
}

impl<S: DownloadSink> DownloadSinkWrapper<S> {
    fn handle_result<T, E>(&mut self, result: std::result::Result<T, E>) -> SinkResult<T>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                let ret = SinkResult::Err(SinkError::OnWrite(format!("{}", e)));
                if let Some(sender) = self.sender.take() {
                    let _ = sender.send(Some(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )));
                }
                ret
            }
        }
    }
}

impl<S> Sink for DownloadSinkWrapper<S>
where
    S: DownloadSink,
{
    fn start(&mut self) -> SinkResult<()> {
        let ret = self.download_sink.start();
        self.handle_result(ret)
    }

    fn write(
        &mut self,
        packet: librespot::playback::decoder::AudioPacket,
        converter: &mut librespot::playback::convert::Converter,
    ) -> librespot::playback::audio_backend::SinkResult<()> {
        let samples = {
            let samplesf64 = self.handle_result(packet.samples())?;
            converter.f64_to_s16(samplesf64)
        };
        let result = self.download_sink.write_samples(&samples);
        self.handle_result(result)
    }

    fn stop(&mut self) -> SinkResult<()> {
        let result = self.download_sink.finish();
        self.handle_result(result)
    }
}

pub async fn download<S>(session: &Session, sink: S, track: SpotifyId) -> std::io::Result<()>
where
    S: DownloadSink,
{
    let player_config = PlayerConfig {
        bitrate: Bitrate::Bitrate320,
        passthrough: true,
        ..Default::default()
    };
    let (sink, rx) = DownloadSinkWrapper::new(sink);
    let mut player = Player::new(
        player_config,
        session.librespot().clone(),
        Box::new(NoOpVolume),
        move || Box::new(sink),
    );
    player.load(
        ResourceId::new(Resource::Track, track).to_librespot(),
        true,
        0,
    );
    tokio::select! {
        rx = rx => {
            let err = rx.unwrap().unwrap();
            return Err(err);
        }
        _ = player.await_end_of_track() => {}
    }
    Ok(())
}

pub async fn download_samples(session: &Session, track: SpotifyId) -> std::io::Result<Vec<i16>> {
    let sink = MemoryDownloadSink::default();
    download(session, sink.clone(), track).await?;
    Ok(sink.take_buffer())
}
