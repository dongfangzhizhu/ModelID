//! One module per command family.
//!
//! `main.rs` keeps the clap definitions and the dispatch; everything else lives
//! here, so a command can be read, tested and changed on its own.

pub mod config;
pub mod db;
pub mod dedup;
pub mod gc;
pub mod hf;
pub mod hfstore;
pub mod meta;
pub mod proxy;
pub mod query;
pub mod repair;
pub mod scan;
pub mod serve;
pub mod store;
pub mod tx;
