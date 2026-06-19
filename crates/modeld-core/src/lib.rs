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
//! - Workflow reference graph + safe GC (Phase 4)

pub mod cas;
pub mod db;
pub mod dedup;
pub mod downloader;
pub mod gc;
pub mod hash;
pub mod hf_cache;
pub mod i18n;
pub mod links;
pub mod quarantine;
pub mod scanner;
pub mod workflow;

pub use cas::CasStore;
pub use db::{Database, Download, DownloadStatus, HfMapping, Model, WorkflowRecord, WorkflowRef};
pub use dedup::{DedupEngine, DedupMode, DedupStats, DuplicateGroup};
pub use downloader::{DownloadResult, Downloader, HfFileMetadata};
pub use gc::{GcCandidate, GcEngine, GcPreview, GcResult};
pub use hash::{hash_file, Blake3Hash};
pub use hf_cache::{HfCache, HfCacheStats};
pub use i18n::{detect, t, tf, Lang};
pub use quarantine::QuarantineManager;
pub use scanner::{ScannedFile, Scanner};
pub use workflow::{
    build_model_lookup, find_workflow_files, index_workflow, parse_workflow, ModelRef,
    ParsedWorkflow,
};
