//! modeld-client: Rust client SDK for a modeld proxy server.
//!
//! Blocking HTTP client (built on `ureq`) for talking to a running
//! `modeld proxy start` instance. Supports health checks, model listing,
//! resumable blob downloads, and HuggingFace-proxy downloads.

pub use client::{HealthInfo, ModelInfo, ModeldClient};

mod client;
