//! Audio device enumeration and configuration.
//!
//! Provides device discovery and trait extensions for querying supported configurations.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SupportedStreamConfig};

/// Returns the system's default audio output device.
pub fn default_output_device() -> Result<Device> {
    let host = cpal::default_host();
    host.default_output_device()
        .ok_or_else(|| anyhow::anyhow!("No default output device"))
}

/// Enumerates all audio output devices on the system.
pub fn enumerate_output_devices() -> Vec<Device> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|iter| iter.collect())
        .unwrap_or_default()
}

/// Extension trait for querying device audio configuration support.
pub trait DeviceExt {
    /// Returns all supported audio configurations for this device.
    fn supported_configs(&self) -> Result<Vec<SupportedStreamConfig>>;
}

impl DeviceExt for Device {
    fn supported_configs(&self) -> Result<Vec<SupportedStreamConfig>> {
        Ok(self
            .supported_output_configs()?
            .map(|range| range.with_max_sample_rate())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_devices() {
        // Just check it doesn't panic
        let _devices = enumerate_output_devices();
    }

    #[test]
    fn test_config_validation() {
        // Skip in CI environments where no audio hardware is available
        if std::env::var("CI").is_ok() {
            return;
        }
        if let Ok(device) = default_output_device() {
            let configs = device.supported_configs();
            assert!(configs.is_ok());
        }
    }
}
