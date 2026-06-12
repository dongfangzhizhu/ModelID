//! modeld-core: Core library for Content-Addressable Storage (CAS)
//!
//! This crate provides the foundational functionality for modeld:
//! - BLAKE3 hashing with performance optimizations
//! - CAS storage layer
//! - SQLite metadata index
//! - File scanning
//! - Deduplication engine (two-phase commit)
//! - Quarantine mechanism
//! - HuggingFace interception layer (Phase 3)

pub mod cas;
pub mod db;
pub mod dedup;
pub mod downloader;
pub mod hash;
pub mod hf_cache;
pub mod links;
pub mod quarantine;
pub mod scanner;

pub use cas::CasStore;
pub use db::{Database, Download, DownloadStatus, HfMapping, Model};
pub use downloader::{DownloadResult, Downloader, HfFileMetadata};
pub use hash::{hash_file, Blake3Hash};
pub use hf_cache::{HfCache, HfCacheStats};
pub use scanner::{ScannedFile, Scanner};
pub use dedup::{DedupEngine, DedupMode, DedupStats, DuplicateGroup};
pub use quarantine::QuarantineManager;
