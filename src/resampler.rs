//! Real-time-safe linear-resampling bridge between the runtime's fixed sample
//! rate and the device's negotiated sample rate.
//!
//! `cpal` requires an *exact* sample-rate match against a device's supported
//! configurations. [`crate::stream_controller::StreamController::get_best_sample_rate`]
//! negotiates the closest supported device rate, but the audio graph always
//! runs at its own configured rate. When those two rates differ we resample
//! (the fallback path required by `auxide-io-rfi`).
//!
//! The resampler is **allocation-free on the audio path**: all buffers are
//! pre-allocated at construction and `VecDeque`/`Vec` capacity is reused. It
//! performs linear interpolation, which is cheap and glitch-free for the small
//! rate mismatches that occur in practice (e.g. 44100 <-> 48000).

use std::collections::VecDeque;

/// A streaming, monotonic linear interpolator converting a mono input stream
/// sampled at `input_rate` into a mono output stream sampled at `output_rate`.
///
/// State (pending input samples and the sub-sample phase) is carried across
/// calls so resampling is correct across host-buffer boundaries.
pub struct LinearResampler {
    /// input samples per output sample (`input_rate / output_rate`).
    ratio: f64,
    /// Ring of recently pushed input samples awaiting conversion.
    pending: VecDeque<f32>,
    /// Absolute input-sample index of `pending[0]`.
    pending_base: u64,
    /// Absolute input-sample position of the *next* output sample to emit.
    next_idx: f64,
    /// Last input sample observed, held for interpolation past the buffer end.
    last: f32,
    /// High-water mark for `pending` length to bound memory.
    max_pending: usize,
}

impl LinearResampler {
    /// Creates a resampler for the given rate ratio.
    ///
    /// `input_rate` is the runtime (graph) sample rate; `output_rate` is the
    /// device sample rate. `block_size` is the runtime block size, used to size
    /// internal buffers so no allocation happens during streaming.
    pub fn new(input_rate: u32, output_rate: u32, block_size: usize) -> Self {
        let ratio = input_rate as f64 / output_rate.max(1) as f64;
        // Bound pending to hold a full host buffer's worth of input samples
        // (worst case several times the runtime block size) plus margin, so a
        // single large push or a large device buffer never trims live samples.
        let max_pending = 16384 * 8 + block_size;
        Self {
            ratio,
            pending: VecDeque::with_capacity(max_pending),
            pending_base: 0,
            next_idx: 0.0,
            last: 0.0,
            max_pending,
        }
    }

    /// Resets all carried state (used when (re)starting a stream).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_base = 0;
        self.next_idx = 0.0;
        self.last = 0.0;
    }

    /// Pushes a block of input samples into the resampler's pending window.
    pub fn push(&mut self, block: &[f32]) {
        for &s in block {
            self.pending.push_back(s);
            self.last = s;
        }
        // Drop samples that can no longer be referenced: anything strictly
        // before `next_idx - 1` is no longer needed for interpolation.
        let keep_from = (self.next_idx.floor() as i64) - 1;
        while (self.pending_base as i64) < keep_from {
            if self.pending.pop_front().is_some() {
                self.pending_base += 1;
            } else {
                break;
            }
        }
        // Bound absolute memory growth.
        while self.pending.len() > self.max_pending {
            self.pending.pop_front();
            self.pending_base += 1;
        }
    }

    /// Produces `n` output samples into `out` by linear interpolation of the
    /// pending input window.
    ///
    /// Callers must push enough input samples first (see
    /// [`Self::input_needed_for`]).
    pub fn pull(&mut self, n: usize, out: &mut [f32]) {
        let n = n.min(out.len());
        for out_sample in out.iter_mut().take(n) {
            let pos = self.next_idx;
            let i = pos.floor() as i64;
            let frac = pos - i as f64;
            let s0 = self.sample_at(i);
            let s1 = self.sample_at(i + 1);
            *out_sample = s0 * (1.0 - frac as f32) + s1 * (frac as f32);
            self.next_idx += self.ratio;
        }
    }

    /// Number of input samples that must be available (pushed) to safely emit
    /// `output_samples` more output samples.
    pub fn input_needed_for(&self, output_samples: usize) -> usize {
        // Highest input index referenced is for the LAST output, at
        // next_idx + (n-1)*ratio, plus one more for its interpolation
        // look-ahead. We only need to push enough to cover that.
        let required = self.next_idx + (output_samples as f64 - 1.0) * self.ratio + 1.0;
        let needed_abs = required.ceil() as i64;
        let have_abs = (self.pending_base as i64) + self.pending.len() as i64;
        (needed_abs - have_abs).max(0) as usize
    }

    /// Samples the (absolute) input index `idx`, holding the last known sample
    /// when `idx` is past the end and returning 0 before the start.
    fn sample_at(&self, idx: i64) -> f32 {
        let base = self.pending_base as i64;
        let end = base + self.pending.len() as i64;
        if idx < base {
            0.0
        } else if idx >= end {
            self.last
        } else {
            self.pending[(idx - base) as usize]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_downsamples_by_two() {
        // ratio = 2.0: 8 input samples -> 4 output samples at integer positions.
        let mut r = LinearResampler::new(2, 1, 8);
        let input: Vec<f32> = (0..8).map(|x| x as f32).collect();
        r.push(&input);
        let mut out = [0.0f32; 4];
        r.pull(4, &mut out);
        assert_eq!(out, [0.0, 2.0, 4.0, 6.0]);
    }

    #[test]
    fn resampler_upsamples_by_two() {
        // ratio = 0.5: 4 input samples -> 8 output samples interpolated.
        let mut r = LinearResampler::new(1, 2, 4);
        let input: Vec<f32> = (0..4).map(|x| x as f32).collect();
        r.push(&input);
        let mut out = [0.0f32; 8];
        r.pull(8, &mut out);
        // Positions: 0,0.5,1,1.5,2,2.5,3,3(hold)
        assert_eq!(out, [0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.0]);
    }

    #[test]
    fn resampler_preserves_constant_signal() {
        let mut r = LinearResampler::new(44100, 48000, 64);
        let input = vec![0.25f32; 200];
        r.push(&input);
        let mut out = [0.0f32; 220];
        r.pull(220, &mut out);
        for &s in &out {
            assert!((s - 0.25).abs() < 1e-5, "constant signal must be preserved");
        }
    }

    #[test]
    fn resampler_streams_across_calls() {
        let mut r = LinearResampler::new(3, 2, 4);
        let mut produced = Vec::new();
        for chunk in 0..5u32 {
            let block: Vec<f32> = (0..4).map(|x| (chunk * 4 + x) as f32).collect();
            r.push(&block);
            let mut out = [0.0f32; 3];
            r.pull(3, &mut out);
            produced.extend_from_slice(&out);
        }
        assert_eq!(produced.len(), 15);
        assert!(r.pending.len() <= r.max_pending);
    }

    #[test]
    fn resampler_ratio_one_is_passthrough() {
        let mut r = LinearResampler::new(48000, 48000, 8);
        let input: Vec<f32> = (0..8).map(|x| x as f32).collect();
        r.push(&input);
        let mut out = [0.0f32; 8];
        r.pull(8, &mut out);
        assert_eq!(out, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    }
}
