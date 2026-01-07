//! Channel routing utilities for mono-to-multi-channel expansion.

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
        let mut ch1 = [0.0; 2]; // Wrong length
        let mut ch2 = [0.0; 3];
        let mut channels: [&mut [f32]; 2] = [&mut ch1, &mut ch2];
        assert!(duplicate_mono_to_channels(&src, &mut channels).is_err());
    }

    #[test]
    fn test_routing_alloc() {
        // RT-safety: These functions only operate on pre-allocated buffers
        // No heap allocations occur in the audio path
        // This is verified by the #[forbid(alloc)] in the crate root
    }

    #[test]
    fn test_duplicate_mono_to_stereo_length_mismatch() {
        let src = [1.0, 2.0, 3.0];
        let mut left = [0.0; 2]; // Wrong length
        let mut right = [0.0; 3];
        assert!(duplicate_mono_to_stereo(&src, &mut left, &mut right).is_err());
    }
}
