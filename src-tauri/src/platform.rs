//! Platform-specific implementations
//!
//! This module provides abstractions for platform-specific functionality,
//! allowing the rest of the codebase to use a consistent API regardless
//! of the underlying operating system or display server.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub use linux::*;
