//! Lock-free stream state management.
//!
//! Provides atomic state transitions (Running, Stopped) for real-time safety.

use anyhow::Result;
use std::sync::atomic::{AtomicU8, Ordering};

/// Stream lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamState {
    Running = 0,
    Stopped = 1,
}

/// Lock-free atomic wrapper for stream state transitions.
pub struct AtomicStreamState {
    state: AtomicU8,
}

impl AtomicStreamState {
    /// Creates a new atomic stream state with initial value.
    pub fn new(initial: StreamState) -> Self {
        Self {
            state: AtomicU8::new(initial as u8),
        }
    }

    /// Sets the stream state atomically with Release ordering for synchronization.
    pub fn set_state(&self, state: StreamState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// Gets the current stream state with Acquire ordering for synchronization.
    pub fn get_state(&self) -> StreamState {
        match self.state.load(Ordering::Acquire) {
            0 => StreamState::Running,
            1 => StreamState::Stopped,
            _ => StreamState::Stopped, // Default to stopped on invalid
        }
    }

    /// Verifies that the target platform supports lock-free atomics.
    pub fn verify_lock_free_atomics() -> Result<()> {
        #[cfg(not(target_has_atomic = "8"))]
        return Err(anyhow::anyhow!("AtomicU8 not supported on this platform"));
        #[cfg(not(target_has_atomic = "32"))]
        return Err(anyhow::anyhow!("AtomicU32 not supported on this platform"));
        #[cfg(not(target_has_atomic = "64"))]
        return Err(anyhow::anyhow!("AtomicU64 not supported on this platform"));
        Ok(())
    }

    #[cfg(test)]
    pub fn set_state_raw(&self, value: u8) {
        self.state.store(value, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_state_transitions() {
        let state = AtomicStreamState::new(StreamState::Stopped);
        assert_eq!(state.get_state(), StreamState::Stopped);
        state.set_state(StreamState::Running);
        assert_eq!(state.get_state(), StreamState::Running);
        state.set_state(StreamState::Stopped);
        assert_eq!(state.get_state(), StreamState::Stopped);
    }

    #[test]
    fn test_startup_atomic_check() {
        assert!(AtomicStreamState::verify_lock_free_atomics().is_ok());
    }

    #[test]
    fn test_invalid_state_defaults_to_stopped() {
        let state = AtomicStreamState::new(StreamState::Stopped);
        state.set_state_raw(99);
        assert_eq!(state.get_state(), StreamState::Stopped);
    }
}
