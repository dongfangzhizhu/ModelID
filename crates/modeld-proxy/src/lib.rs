//! modeld-proxy: local HTTP proxy server & LAN registry for modeld CAS.
//!
//! Provides a transparent HuggingFace proxy so multiple machines on a LAN can
//! share one modeld CAS store. Implements the HTTP API defined in
//! `docs/proxy-api.md`:
//!
//! - `GET /health`
//! - `GET /v1/models`
//! - `GET /v1/blobs/{blake3_hash}` (Range-supported)
//! - `GET /v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}` (HF-compatible)
//!
//! Uses blocking I/O (`tiny_http`) for consistency with the rest of modeld.

pub mod auth;
pub mod config;
pub mod discovery;
pub mod range;
pub mod server;

pub use auth::{check_auth, check_ip, AuthDecision};
pub use config::{AuthConfig, NetworkConfig, ProxyConfig};
pub use discovery::{discover, discover_services, DiscoveredServer, DiscoveredService, MdnsAnnouncer};
pub use range::parse_range;
pub use server::{ProxyServer, ServerStats};
