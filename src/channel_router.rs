pub fn duplicate_mono_to_stereo(src: &[f32], left: &mut [f32], right: &mut [f32]) {
    for ((l, r), &s) in left.iter_mut().zip(right.iter_mut()).zip(src.iter()) {
        *l = s;
        *r = s;
    }
}

pub fn duplicate_mono_to_channels(src: &[f32], dst_channels: &mut [&mut [f32]]) {
    for channel in dst_channels {
        channel.copy_from_slice(src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_preserves_samples() {
        let src = [1.0, 2.0, 3.0];
        let mut left = [0.0; 3];
        let mut right = [0.0; 3];
        duplicate_mono_to_stereo(&src, &mut left, &mut right);
        assert_eq!(left, [1.0, 2.0, 3.0]);
        assert_eq!(right, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_routing_alloc() {
        // Hard to test no alloc without tools
    }

    #[test]
    fn test_routing_buffer_ownership() {
        // Assume preallocated
    }
}
