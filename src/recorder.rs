//! Input recording support: persists captured input frames to an in-memory
//! buffer and (optionally) a WAV file via `hound`.

use std::sync::{Arc, Mutex};

/// Accumulates interleaved input samples for later analysis or WAV export.
///
/// `push_block` is called from the audio input callback (one interleaved block
/// per host buffer). `save_wav` exports the accumulated buffer to a 32-bit
/// float WAV file. Recording is intentionally simple: the buffer grows as
/// needed, which is acceptable for capture workloads.
pub struct Recorder {
    buf: Vec<f32>,
    sample_rate: u32,
    channels: usize,
}

impl Recorder {
    /// Creates a recorder for the given device sample rate and channel count.
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        Self {
            buf: Vec::new(),
            sample_rate,
            channels: channels.max(1),
        }
    }

    /// Appends one interleaved block of input samples.
    pub fn push_block(&mut self, block: &[f32]) {
        self.buf.extend_from_slice(block);
    }

    /// Number of frames currently buffered (samples / channels).
    pub fn len_frames(&self) -> usize {
        self.buf.len() / self.channels
    }

    /// True when no samples have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Exports the recorded samples to a 32-bit float WAV file at `path`.
    pub fn save_wav(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        use hound::{SampleFormat, WavSpec, WavWriter};
        let spec = WavSpec {
            channels: self.channels as u16,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer = WavWriter::create(path, spec)?;
        for &s in &self.buf {
            writer.write_sample(s)?;
        }
        writer.finalize()?;
        Ok(())
    }

    /// Returns the raw interleaved buffer (for tests/analysis).
    pub fn samples(&self) -> &[f32] {
        &self.buf
    }
}

/// A recorder safe to share with the audio input callback.
pub type SharedRecorder = Arc<Mutex<Recorder>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_save_roundtrip() {
        let mut rec = Recorder::new(44100, 1);
        let data: Vec<f32> = (0..100).map(|x| (x as f32) * 0.01 - 0.5).collect();
        rec.push_block(&data);
        assert_eq!(rec.len_frames(), 100);
        let path = std::env::temp_dir().join("auxide_io_rec_test.wav");
        rec.save_wav(path.to_str().unwrap()).expect("wav write");
        let mut reader = hound::WavReader::open(&path).expect("wav read");
        assert_eq!(reader.spec().sample_rate, 44100);
        let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), data.len());
        for (a, b) in data.iter().zip(samples.iter()) {
            assert!((a - b).abs() < 1e-6, "round-trip must preserve samples");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn push_block_appends() {
        let mut rec = Recorder::new(48000, 2);
        rec.push_block(&[0.1, 0.2, 0.3, 0.4]);
        rec.push_block(&[0.5, 0.6, 0.7, 0.8]);
        assert_eq!(
            rec.samples(),
            &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
        );
        assert_eq!(rec.len_frames(), 4);
    }
}
