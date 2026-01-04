pub fn handle_process_error(out: &mut [f32]) {
    out.fill(0.0);
}

pub fn handle_device_error() {
    // Stop the stream - this would be called from the stream controller
    // For now, placeholder
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
}
