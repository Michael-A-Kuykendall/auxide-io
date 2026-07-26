//! Channel routing utilities for mono-to-multi-channel expansion and arbitrary mapping.

/// Routing strategy for expanding a mono runtime signal onto a device's output channels.
///
/// The audio graph produces a single mono stream; the host device may expose any
/// number of output channels. `ChannelMap` decides how the mono source is placed
/// across the device's interleaved output frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ChannelMap {
    /// Duplicate the mono source to the first two output channels (left + right).
    /// Channels beyond the first two are left at silence. This is the historical
    /// default behaviour and preserves back-compatibility.
    #[default]
    MonoToStereo,
    /// Route the mono source to an explicit set of destination channels.
    ///
    /// Each `(src_ch, dst_ch)` pair maps the source channel `src_ch` onto device
    /// output channel `dst_ch`. Because the runtime emits a single mono stream,
    /// `src_ch` is always `0`; the destination may be any valid channel index.
    /// When a frame is written, every channel is zeroed first, then each mapped
    /// destination receives the source sample — so unmapped channels stay silent.
    Explicit(Vec<(usize, usize)>),
}

impl ChannelMap {
    /// Applies `sample` (the mono source for this frame) onto an interleaved
    /// output `frame` according to the routing strategy.
    ///
    /// `frame` must be one device frame (length == device channel count). Unmapped
    /// channels are expected to already be zeroed by the caller (see
    /// [`crate::buffer_size_adapter::BufferSizeAdapter`]).
    pub fn apply(&self, sample: f32, frame: &mut [f32]) {
        match self {
            ChannelMap::MonoToStereo => {
                if !frame.is_empty() {
                    frame[0] = sample;
                }
                if frame.len() > 1 {
                    frame[1] = sample;
                }
            }
            ChannelMap::Explicit(map) => {
                for &(_, dst) in map {
                    if dst < frame.len() {
                        frame[dst] = sample;
                    }
                }
            }
        }
    }

    /// Number of source channels this map expects (always 1 — the runtime is mono).
    pub fn source_channels(&self) -> usize {
        1
    }
}

/// Duplicates a mono signal to left and right stereo channels.
///
/// # Errors
/// Returns an error if channel lengths don't match the source.
pub fn duplicate_mono_to_stereo(
    src: &[f32],
    left: &mut [f32],
    right: &mut [f32],
) -> Result<(), &'static str> {
    if left.len() != src.len() || right.len() != src.len() {
        return Err("Channel length mismatch");
    }
    for ((l, r), &s) in left.iter_mut().zip(right.iter_mut()).zip(src.iter()) {
        *l = s;
        *r = s;
    }
    Ok(())
}

/// Duplicates a mono signal to any number of channels.
///
/// # Errors
/// Returns an error if any channel length doesn't match the source.
pub fn duplicate_mono_to_channels(
    src: &[f32],
    dst_channels: &mut [&mut [f32]],
) -> Result<(), &'static str> {
    for channel in dst_channels {
        if channel.len() != src.len() {
            return Err("Channel length mismatch");
        }
        channel.copy_from_slice(src);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_preserves_samples() {
        let src = [1.0, 2.0, 3.0];
        let mut left = [0.0; 3];
        let mut right = [0.0; 3];
        assert!(duplicate_mono_to_stereo(&src, &mut left, &mut right).is_ok());
        assert_eq!(left, [1.0, 2.0, 3.0]);
        assert_eq!(right, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_duplicate_mono_to_channels() {
        let src = [1.0, 2.0, 3.0];
        let mut ch1 = [0.0; 3];
        let mut ch2 = [0.0; 3];
        let mut channels: [&mut [f32]; 2] = [&mut ch1, &mut ch2];
        assert!(duplicate_mono_to_channels(&src, &mut channels).is_ok());
        assert_eq!(ch1, [1.0, 2.0, 3.0]);
        assert_eq!(ch2, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_duplicate_mono_to_channels_length_mismatch() {
        let src = [1.0, 2.0, 3.0];
        let mut ch1 = [0.0; 2];
        let mut ch2 = [0.0; 3];
        let mut channels: [&mut [f32]; 2] = [&mut ch1, &mut ch2];
        assert!(duplicate_mono_to_channels(&src, &mut channels).is_err());
    }

    #[test]
    fn test_duplicate_mono_to_stereo_length_mismatch() {
        let src = [1.0, 2.0, 3.0];
        let mut left = [0.0; 2];
        let mut right = [0.0; 3];
        assert!(duplicate_mono_to_stereo(&src, &mut left, &mut right).is_err());
    }

    #[test]
    fn test_channel_map_default_is_monostereo() {
        assert_eq!(ChannelMap::default(), ChannelMap::MonoToStereo);
    }

    #[test]
    fn channel_map_routes_correctly() {
        let mut frame = [0.0f32; 4];
        ChannelMap::MonoToStereo.apply(0.5, &mut frame);
        assert_eq!(frame[0], 0.5);
        assert_eq!(frame[1], 0.5);
        assert_eq!(frame[2], 0.0);
        assert_eq!(frame[3], 0.0);

        let mut frame2 = [0.0f32; 4];
        ChannelMap::Explicit(vec![(0, 3)]).apply(0.7, &mut frame2);
        assert_eq!(frame2[0], 0.0);
        assert_eq!(frame2[1], 0.0);
        assert_eq!(frame2[2], 0.0);
        assert_eq!(frame2[3], 0.7);
    }
}
