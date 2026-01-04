use crate::stream_state::{AtomicStreamState, StreamState};
use crate::device_management::default_output_device;
use crate::buffer_size_adapter::{BufferSizeAdapter, MAX_HOST_FRAMES};
use crate::error_recovery::handle_process_error;
use auxide::rt::Runtime;
use cpal::{Stream, SampleFormat};
use cpal::traits::{DeviceTrait, StreamTrait};
use crate::device_management::DeviceExt;
use std::sync::Arc;
use anyhow::Result;

pub struct StreamController {
    stream: Option<Stream>,
    state: Arc<AtomicStreamState>,
}

impl StreamController {
    pub fn play(mut runtime: Runtime) -> Result<Self> {
        AtomicStreamState::verify_lock_free_atomics()?;
        let device = default_output_device()?;
        let sample_rate = runtime.sample_rate() as u32;
        let config = device.supported_configs()?.into_iter().find(|c| c.sample_rate().0 == sample_rate && c.channels() == 2 && c.sample_format() == SampleFormat::F32).ok_or_else(|| anyhow::anyhow!("No suitable config"))?;
        let sample_format = config.sample_format();
        let config = config.config();

        let state = Arc::new(AtomicStreamState::new(StreamState::Stopped));
        let state_clone = state.clone();
        let mut adapter = BufferSizeAdapter::new(runtime.plan.block_size);

        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if data.len() > MAX_HOST_FRAMES {
                        handle_process_error(data);
                        return;
                    }
                    match state_clone.get_state() {
                        StreamState::Running => {
                            if let Err(_) = adapter.fill_host_buffer(data, &mut runtime, 2) {
                                handle_process_error(data);
                            }
                        }
                        _ => {
                            data.fill(0.0);
                        }
                    }
                },
                |_| {
                    // TODO: Log error atomically or set flag
                },
                None,
            )?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        Ok(Self {
            stream: Some(stream),
            state,
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

    pub fn pause(&self) {
        if let Some(stream) = &self.stream {
            let _ = stream.pause();
        }
        self.state.set_state(StreamState::Paused);
    }
}
