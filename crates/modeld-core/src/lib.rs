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
pub mod config;
pub mod db;
pub mod dedup;
pub mod doctor;
pub mod downloader;
pub mod fsck;
pub mod gc;
pub mod hash;
pub mod hf_cache;
pub mod i18n;
pub mod links;
pub mod platform;
pub mod quarantine;
pub mod scanner;
pub mod store_path;
pub mod tx;
pub mod unlink;
pub mod workflow;

pub use cas::CasStore;
pub use config::{
    load_config, save_config, AuthConfig, DedupConfig, GcConfig, ModeldConfig, ServeConfig,
    StoreConfig,
};
pub use db::{Database, Download, DownloadStatus, HfMapping, Model, TransactionStatus,
    WalTransaction, WorkflowRecord, WorkflowRef};
pub use dedup::{DedupEngine, DedupMode, DedupStats, DuplicateGroup};
pub use doctor::{run_doctor, CheckStatus, DoctorCheck, DoctorReport};
pub use downloader::{DownloadResult, Downloader, HfFileMetadata};
pub use fsck::{
    run_fsck, run_verify, run_repair,
    DanglingAlias, FsckReport, SizeMismatch,
    HashMismatch, MissingCasObject, OrphanRecord, RepairResult, VerifyReport,
};
pub use gc::{GcCandidate, GcEngine, GcPreview, GcResult};
pub use hash::{hash_file, Blake3Hash};
pub use hf_cache::{HfCache, HfCacheStats};
pub use i18n::{detect, t, tf, Lang};
pub use platform::{detect_capabilities, is_same_volume, check_file_locked, PlatformCapabilities};
pub use quarantine::QuarantineManager;
pub use scanner::{ScannedFile, Scanner};
pub use store_path::{default_store_path, resolve_store_path};
pub use tx::{
    OpType, PathEntry, RecoveryAction, RollbackPlan, TxFilter, TxHandle, TxPlan, TxRecord,
    TxRecoveryResult, TxStatus, TransactionManager,
};
pub use unlink::{unlink_path, UnlinkResult};
pub use workflow::{
    build_model_lookup, find_workflow_files, index_workflow, parse_workflow, ModelRef,
    ParsedWorkflow,
};
