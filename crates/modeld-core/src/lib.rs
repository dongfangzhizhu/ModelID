//! modeld-core: Core library for Content-Addressable Storage (CAS)
//!
//! This crate provides the foundational functionality for modeld:
//! - BLAKE3 hashing with performance optimizations
//! - CAS storage layer
//! - SQLite metadata index
//! - File scanning
//! - Deduplication engine (two-phase commit)
//! - Quarantine mechanism

pub mod cas;
pub mod db;
pub mod dedup;
pub mod hash;
pub mod links;
pub mod quarantine;
pub mod scanner;

pub use cas::CasStore;
pub use db::{Database, Model};
pub use hash::{hash_file, Blake3Hash};
pub use scanner::{ScannedFile, Scanner};
pub use dedup::{DedupEngine, DedupMode, DedupStats, DuplicateGroup};
pub use quarantine::QuarantineManager;

