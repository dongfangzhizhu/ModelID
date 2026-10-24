//! One module per command family.
//!
//! `main.rs` keeps the clap definitions and the dispatch; everything else lives
//! here, so a command can be read, tested and changed on its own.

pub mod config;
pub mod db;
pub mod hf;
pub mod store;
pub mod tx;
