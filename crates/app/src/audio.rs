use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub struct AudioPlayer {
    player: Player,
    _stream: MixerDeviceSink,
}

impl AudioPlayer {
    pub fn open(path: &Path) -> Result<Self, String> {
        let stream = DeviceSinkBuilder::open_default_sink().map_err(|err| err.to_string())?;
        let player = Player::connect_new(stream.mixer());
        let file = File::open(path).map_err(|err| err.to_string())?;
        let decoder = Decoder::new(BufReader::new(file))
            .map_err(|err| format!("오디오 디코딩 실패: {err}"))?;
        player.append(decoder);
        Ok(Self {
            player,
            _stream: stream,
        })
    }

    pub fn toggle(&self) {
        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn is_paused(&self) -> bool {
        self.player.is_paused()
    }
}
