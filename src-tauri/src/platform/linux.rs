//! Linux-specific platform implementations
//!
//! This module provides Linux-specific functionality including display server
//! detection, text insertion, and keyboard shortcuts for both X11 and Wayland.

pub mod display;
pub mod shortcuts;
pub mod text;

pub use text::TextInserter;
