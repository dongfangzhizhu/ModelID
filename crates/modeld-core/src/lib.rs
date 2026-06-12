//! modeld-core: Core library for Content-Addressable Storage (CAS)
//!
//! This crate provides the foundational functionality for modeld:
//! - BLAKE3 hashing with performance optimizations
//! - CAS storage layer
//! - SQLite metadata index
//! - File scanning

pub mod cas;
pub mod db;
pub mod hash;
pub mod scanner;

pub use cas::CasStore;
pub use db::{Database, Model};
pub use hash::{hash_file, Blake3Hash};
pub use scanner::{ScannedFile, Scanner};
