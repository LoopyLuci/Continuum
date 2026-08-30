//! Continuum Core
//!
//! Core traits and types for the Continuum remote desktop platform.
//!
//! This crate defines the fundamental abstractions that all Continuum
//! components are built on. Every trait here is designed to be swappable,
//! allowing the platform to evolve over decades.

// Future goal: `#![no_std]` for embedded/UEFI clients.
// Currently blocked by thiserror/serde std dependency.

pub mod transport;
pub mod video;
pub mod crypto;
pub mod capture;
pub mod audio;
pub mod input;
pub mod protocol;

/// Result type for Continuum operations.
pub type ContinuumResult<T> = std::result::Result<T, ContinuumError>;

/// Error type for Continuum operations.
#[derive(Debug, thiserror::Error)]
pub enum ContinuumError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Video error: {0}")]
    Video(String),

    #[error("Crypto error: {0}")]
    Crypto(String),

    #[error("Capture error: {0}")]
    Capture(String),

    #[error("Audio error: {0}")]
    Audio(String),

    #[error("Input error: {0}")]
    Input(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Not supported: {0}")]
    NotSupported(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
