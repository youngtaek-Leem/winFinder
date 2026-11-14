use rodio::{Decoder, OutputStream, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub struct AudioPlayer {
    sink: Sink,
    _stream: OutputStream,
}

impl AudioPlayer {
    pub fn open(path: &Path) -> Result<Self, String> {
        let (stream, handle) = OutputStream::try_default().map_err(|err| err.to_string())?;
        let sink = Sink::try_new(&handle).map_err(|err| err.to_string())?;
        let file = File::open(path).map_err(|err| err.to_string())?;
        let decoder = Decoder::new(BufReader::new(file))
            .map_err(|err| format!("오디오 디코딩 실패: {err}"))?;
        sink.append(decoder);
        Ok(Self {
            sink,
            _stream: stream,
        })
    }

    pub fn toggle(&self) {
        if self.sink.is_paused() {
            self.sink.play();
        } else {
            self.sink.pause();
        }
    }

    pub fn stop(&self) {
        self.sink.stop();
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }
}
