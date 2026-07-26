//! # Auxide IO
//!
//! Real-time audio I/O abstraction for Auxide, providing portable audio streaming via cpal.
//!
//! This crate handles:
//! - Audio device enumeration and configuration
//! - Real-time stream management with lock-free state
//! - Buffer size adaptation between host and runtime
//! - Error recovery and graceful silence-on-error patterns
//! - Channel routing (mono→stereo, etc.)
//!
//! ## Real-time safety
//!
//! The audio callback is **allocation-light**: it updates lock-free
//! diagnostics atomics and performs no logging on the hot path. It does
//! **not** claim `#![forbid(alloc)]` — the underlying cpal/host audio paths
//! may allocate on some platforms — so the guarantee is "no allocation
//! introduced by this crate on the audio path", not a hard `forbid(alloc)`.

#![forbid(unsafe_code)]

pub mod buffer_size_adapter;
pub mod channel_router;
pub mod device_management;
pub mod error_recovery;
pub mod stream_controller;
pub mod stream_state;
