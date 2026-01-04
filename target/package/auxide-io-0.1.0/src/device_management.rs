use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SupportedStreamConfig};

pub fn default_output_device() -> Result<Device> {
    let host = cpal::default_host();
    host.default_output_device()
        .ok_or_else(|| anyhow::anyhow!("No default output device"))
}

pub fn enumerate_output_devices() -> Vec<Device> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|iter| iter.collect())
        .unwrap_or_default()
}

pub trait DeviceExt {
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
        if let Ok(device) = default_output_device() {
            let configs = device.supported_configs();
            assert!(configs.is_ok());
        }
    }
}
