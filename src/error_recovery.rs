pub fn handle_process_error(out: &mut [f32]) {
    out.fill(0.0);
}

pub fn handle_device_error() {
    // Device errors are handled by setting atomic flags in StreamController
    // This function is kept for API consistency but is not directly called
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_produces_silence() {
        let mut out = [1.0, 2.0, 3.0];
        handle_process_error(&mut out);
        assert_eq!(out, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_handle_device_error() {
        // Test that handle_device_error doesn't panic
        handle_device_error();
    }
}
