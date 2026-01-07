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

#![forbid(unsafe_code)]

pub mod buffer_size_adapter;
pub mod channel_router;
pub mod device_management;
pub mod error_recovery;
pub mod stream_controller;
pub mod stream_state;
