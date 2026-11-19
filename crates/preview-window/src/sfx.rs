//! A sound that is played on its own, outside of a map: what the logo in the toolbar plays (the hit sound of the picked skin).
use std::collections::HashMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};

use rodio::Source;

/// A decoded recording: interleaved samples with their channel count and rate.
#[derive(Clone)]
struct Clip {
    channels: NonZero<u16>,
    rate: NonZero<u32>,
    data: Vec<f32>,
}

fn decode_file(path: &Path) -> Option<Clip> {
    let file = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::try_from(file).ok()?;
    let (channels, rate) = (dec.channels(), dec.sample_rate());
    Some(Clip { channels, rate, data: dec.collect() })
}

/// Plays sound files. The audio device is opened the first time something is played, and a file is decoded only once.
#[derive(Default)]
pub struct Sfx {
    sink: Option<rodio::MixerDeviceSink>,
    tried: bool,
    cache: HashMap<PathBuf, Option<Clip>>,
}

impl Sfx {
    pub fn new() -> Sfx {
        Sfx::default()
    }

    /// Play the sound in `path` at `volume` (0 to 1). False when there is no audio output or the file cannot be played.
    pub fn play_file(&mut self, path: &Path, volume: f32) -> bool {
        if self.sink.is_none() && !self.tried {
            self.tried = true;
            self.sink = rodio::DeviceSinkBuilder::open_default_sink().ok();
        }
        let Some(sink) = &self.sink else { return false };
        let clip = self.cache.entry(path.to_path_buf()).or_insert_with(|| decode_file(path)).clone();
        let Some(c) = clip else { return false };
        sink.mixer().add(rodio::buffer::SamplesBuffer::new(c.channels, c.rate, c.data).amplify(volume.clamp(0.0, 1.0)));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_file_decodes_to_its_samples() {
        let dir = std::env::temp_dir().join(format!("ss-sfx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rate = 22_050u32;
        let pcm: Vec<i16> = (0..rate as usize / 10).map(|i| ((i as f32 * 0.2).sin() * 12000.0) as i16).collect();
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + pcm.len() as u32 * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((pcm.len() as u32 * 2).to_le_bytes());
        for v in &pcm {
            wav.extend(v.to_le_bytes());
        }
        let file = dir.join("hit.wav");
        std::fs::write(&file, &wav).unwrap();
        let c = decode_file(&file).expect("decodes");
        assert_eq!((c.channels.get(), c.rate.get(), c.data.len()), (1, rate, pcm.len()));
        assert!(decode_file(&dir.join("missing.wav")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
