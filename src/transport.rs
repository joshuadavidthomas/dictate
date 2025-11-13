//! Transport layer for socket communication
//!
//! This module provides a unified abstraction for socket-based communication
//! with separate implementations for async (tokio) and sync (std) I/O.
//!
//! The transport layer is organized into:
//! - `codec`: NDJSON encoding/decoding for messages
//! - `async_transport`: Tokio-based async client transport
//! - `sync_transport`: Std-based sync client transport

pub mod codec;
pub mod async_transport;
pub mod sync_transport;

pub use async_transport::{AsyncTransport, AsyncConnection};
pub use sync_transport::SyncTransport;
