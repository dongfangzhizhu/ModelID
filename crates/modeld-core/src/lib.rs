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
//! - Append-only NDJSON audit log

pub mod audit;
pub mod cas;
pub mod config;
pub mod db;
pub mod dedup;
pub mod doctor;
pub mod downloader;
pub mod fsck;
pub mod gc;
pub mod governance;
pub mod hash;
pub mod hf_cache;
pub mod i18n;
pub mod ingest;
pub mod links;
pub mod platform;
pub mod quarantine;
pub mod refs;
pub mod scanner;
pub mod store_path;
pub mod tx;
pub mod unlink;
pub mod workflow;

pub use audit::{AuditEntry, AuditLogger};
pub use cas::CasStore;
pub use config::{
    load_config, save_config, AuthConfig, DedupConfig, GcConfig, ModeldConfig, ServeConfig,
    StoreConfig,
};
pub use db::{
    Database, Download, DownloadStatus, FileIndexEntry, HfMapping, Model, ScanStatus,
    TransactionStatus, WalTransaction, WorkflowRecord, WorkflowRef,
};
pub use dedup::{
    compute_risk_level, DedupEngine, DedupGroupPlan, DedupMode, DedupPlan, DedupStats,
    DedupStrategy, DuplicateGroup, RiskLevel,
};
pub use doctor::{run_doctor, CheckStatus, DoctorCheck, DoctorReport};
pub use downloader::{
    token_get, token_remove, token_set, token_status, DownloadResult, Downloader, HfFileMetadata,
    Provenance,
};
pub use fsck::{
    run_fsck, run_repair, run_verify, DanglingAlias, FsckReport, HashMismatch, MissingCasObject,
    OrphanRecord, RepairResult, SizeMismatch, VerifyReport,
};
pub use gc::{classify_refs, GcCandidate, GcEngine, GcPreview, GcPreviewItem, GcResult, RefStatus};
pub use governance::{
    add_tag, export_tags_json, favorite_model, get_note, get_provenance, get_tags_for_model,
    import_tags_json, is_pinned, list_all_tags, list_pinned, pin_model, remove_tag, set_note,
    set_provenance, unfavorite_model, unpin_model, ProvenanceInfo,
};
pub use hash::{hash_file, Blake3Hash};
pub use hf_cache::{HfCache, HfCacheStats};
pub use i18n::{detect, t, tf, Lang};
pub use ingest::{ingest_scan_results, IngestError, IngestResult};
pub use platform::{check_file_locked, detect_capabilities, is_same_volume, PlatformCapabilities};
pub use quarantine::QuarantineManager;
pub use refs::{explain_refs, list_orphans, scan_workflow_refs};
pub use scanner::{ScanOptions, ScannedFile, Scanner};
pub use store_path::{default_store_path, resolve_store_path};
pub use tx::{
    OpType, PathEntry, RecoveryAction, RollbackPlan, TransactionManager, TxFilter, TxHandle,
    TxPlan, TxRecord, TxRecoveryResult, TxStatus,
};
pub use unlink::{unlink_path, UnlinkResult};
pub use workflow::{
    build_model_lookup, find_workflow_files, index_workflow, parse_workflow, ModelRef,
    ParsedWorkflow,
};
