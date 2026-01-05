use crate::buffer_size_adapter::{BufferSizeAdapter, MAX_HOST_FRAMES};
use crate::device_management::default_output_device;
use crate::device_management::DeviceExt;
use crate::error_recovery::handle_process_error;
use crate::stream_state::{AtomicStreamState, StreamState};
use anyhow::Result;
use auxide::rt::Runtime;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct StreamController {
    stream: Option<Stream>,
    state: Arc<AtomicStreamState>,
    error_flag: Arc<AtomicBool>,
}

impl StreamController {
    /// Get the best available sample rate for audio output
    pub fn get_best_sample_rate(requested_rate: f32) -> Result<f32> {
        let device = default_output_device()?;
        let requested_sample_rate = requested_rate as u32;
        
        let supported_configs: Vec<_> = device.supported_configs()?.into_iter().collect();
        
        // First try to find exact match
        if let Some(config) = supported_configs
            .iter()
            .find(|c| {
                c.sample_rate().0 == requested_sample_rate
                    && c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
            }) {
            return Ok(config.sample_rate().0 as f32);
        }
        
        // Find best alternative
        if let Some(config) = supported_configs
            .iter()
            .filter(|c| c.channels() == 2 && c.sample_format() == SampleFormat::F32)
            .min_by_key(|c| {
                let rate = c.sample_rate().0;
                if rate >= requested_sample_rate {
                    rate - requested_sample_rate
                } else {
                    requested_sample_rate - rate
                }
            }) {
            return Ok(config.sample_rate().0 as f32);
        }
        
        // Fallback to any F32 config
        if let Some(config) = supported_configs
            .iter()
            .find(|c| c.sample_format() == SampleFormat::F32) {
            return Ok(config.sample_rate().0 as f32);
        }
        
        Err(anyhow::anyhow!("No suitable audio configuration found"))
    }

    pub fn play(mut runtime: Runtime) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let device = default_output_device()?;
        let sample_rate = runtime.sample_rate() as u32;
        
        // Find a supported configuration that matches our runtime's sample rate
        let config = device
            .supported_configs()?
            .into_iter()
            .find(|c| {
                c.sample_rate().0 == sample_rate
                    && c.channels() == 2
                    && c.sample_format() == SampleFormat::F32
            })
            .ok_or_else(|| anyhow::anyhow!("No suitable config for {}Hz", sample_rate))?;
            
        let sample_format = config.sample_format();
        let config = config.config();

        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        let state_clone = state.clone();
        let error_flag_clone = error_flag.clone();
        let error_flag_clone2 = error_flag.clone();
        let mut adapter = BufferSizeAdapter::new(runtime.plan.block_size);

        let mut callback_count = 0u64;
        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    callback_count += 1;
                    if callback_count % 100 == 0 {
                        println!("Audio callback #{}", callback_count);
                    }
                    if data.len() > MAX_HOST_FRAMES {
                        handle_process_error(data);
                        return;
                    }
                    match state_clone.get_state() {
                        StreamState::Running => {
                            if adapter.fill_host_buffer(data, &mut runtime, 2).is_err() {
                                error_flag_clone.store(true, Ordering::Relaxed);
                                handle_process_error(data);
                            }
                        }
                        _ => {
                            data.fill(0.0);
                        }
                    }
                },
                move |_| {
                    error_flag_clone2.store(true, Ordering::Relaxed);
                },
                None,
            )?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        Ok(Self {
            stream: Some(stream),
            state,
            error_flag,
        })
    }

    pub fn start(&self) -> Result<()> {
        if let Some(stream) = &self.stream {
            stream.play()?;
            self.state.set_state(StreamState::Running);
        }
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(stream) = &self.stream {
            let _ = stream.pause();
        }
        self.state.set_state(StreamState::Stopped);
    }

    pub fn has_error(&self) -> bool {
        self.error_flag.load(Ordering::Relaxed)
    }

    pub fn clear_error(&self) {
        self.error_flag.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_flag() {
        // Test error flag functionality (without creating actual streams)
        let _state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let error_flag = Arc::new(AtomicBool::new(false));
        
        // Simulate the error flag behavior
        assert!(!error_flag.load(Ordering::Relaxed));
        error_flag.store(true, Ordering::Relaxed);
        assert!(error_flag.load(Ordering::Relaxed));
        error_flag.store(false, Ordering::Relaxed);
        assert!(!error_flag.load(Ordering::Relaxed));
    }
}
