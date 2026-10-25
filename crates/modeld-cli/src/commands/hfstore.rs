//! Store-local Hugging Face and workflow commands: they inspect what is already
//! in the store rather than talking to the Hub.

use crate::{format_bytes, open_db};
use anyhow::Result;
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{
    build_model_lookup, find_workflow_files, index_workflow, parse_workflow, t, tf, Database,
    HfCache,
};
use std::path::PathBuf;

/// `modeld hf-check <repo_id> <filename>` — check modeld cache for a HF file
pub fn hf_check_command(
    store: PathBuf,
    repo_id: &str,
    filename: &str,
    revision: &str,
    json_output: bool,
) -> Result<()> {
    let db = open_db(&store)?;
    let hf_cache = HfCache::new(&store);

    // Check snapshot path
    let snapshot_path = hf_cache.snapshot_file_path(repo_id, revision, filename);
    let found = snapshot_path.exists();

    if json_output {
        if found {
            println!(r#"{{"found": true, "path": "{}"}}"#, snapshot_path.display());
        } else {
            println!(r#"{{"found": false}}"#);
        }
    } else if found {
        println!(
            "{} {}",
            "✓".green().bold(),
            tf("hf.check.hit", &[("repo", &repo_id), ("file", &filename), ("rev", &revision)])
        );
        let p = snapshot_path.display().to_string();
        println!("  {}", tf("hf.check.path", &[("path", &p)]).dimmed());
    } else {
        println!(
            "{} {}",
            "✗".red(),
            tf("hf.check.miss", &[("repo", &repo_id), ("file", &filename), ("rev", &revision)])
        );
        std::process::exit(1);
    }

    drop(db);
    Ok(())
}

/// `modeld hf-setup` — configure HF_HOME environment variable
pub fn hf_setup_command(store: PathBuf, print_path: bool) -> Result<()> {
    let hf_cache = HfCache::new(&store);
    hf_cache.init()?;
    let hf_home = hf_cache.hf_home();

    if print_path {
        // Machine-readable: just print the path (used by Python hook)
        println!("{}", hf_home.display());
        return Ok(());
    }

    println!("{}", t("hf.setup.header").bold().cyan());
    println!();
    let p = hf_home.display().to_string();
    println!("{} {}", t("hf.setup.dir"), p.green());
    println!();
    println!("{}", t("hf.setup.activate").bold());
    println!();

    #[cfg(windows)]
    {
        let p = hf_home.display().to_string();
        println!("  {}", t("hf.setup.windows_ps").yellow());
        println!("  {}", tf("hf.setup.set_ps", &[("path", &p)]));
        println!();
        println!("  {}", t("hf.setup.windows_cmd").yellow());
        println!("  {}", tf("hf.setup.set_cmd", &[("path", &p)]));
    }

    #[cfg(unix)]
    {
        let p = hf_home.display().to_string();
        println!("  {}", t("hf.setup.unix").yellow());
        println!("  {}", tf("hf.setup.set_unix", &[("path", &p)]));
    }

    println!();
    println!("{}", t("hf.setup.hook_header").bold());
    println!("  {}", t("hf.setup.hook_pip"));
    println!("  {}", t("hf.setup.hook_use"));
    println!("  {}", t("hf.setup.hook_import"));

    Ok(())
}

/// `modeld hf-status` — show HF cache statistics
pub fn hf_status_command(store: PathBuf) -> Result<()> {
    let hf_cache = HfCache::new(&store);
    let db = open_db(&store)?;

    println!("{}", t("hf.status.header").bold().cyan());
    println!("{}", "─".repeat(40).dimmed());

    if !hf_cache.hf_home().exists() {
        println!("{}", t("hf.status.not_initialized").yellow());
        return Ok(());
    }

    let stats = hf_cache.stats()?;

    let p = hf_cache.hf_home().display().to_string();
    println!("  {} {}", t("hf.status.hf_home"), p.green());
    println!("  {}", tf("hf.status.repos", &[("count", &stats.total_repos)]).bold());
    println!("  {}", tf("hf.status.blobs", &[("count", &stats.total_blobs)]).bold());

    // List repos
    let repos = hf_cache.list_repos()?;
    if !repos.is_empty() {
        println!();
        println!("{}", t("hf.status.cached_repos").bold());
        for repo in &repos {
            println!("  {} {}", "•".cyan(), tf("hf.status.repo_line", &[("repo", repo)]).cyan());
        }
    }

    // Downloads from DB
    let downloads = db.list_downloads(None)?;
    let done_count =
        downloads.iter().filter(|d| matches!(d.status, modeld_core::DownloadStatus::Done)).count();

    if !downloads.is_empty() {
        println!();
        println!(
            "  {}",
            tf("hf.status.downloads", &[("total", &downloads.len()), ("done", &done_count)]).bold()
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 4: Workflow & Reference Graph commands
// ─────────────────────────────────────────────────────────────────────────────

pub fn workflow_scan_command(store: PathBuf, workflow_dir: PathBuf) -> Result<()> {
    let db_path = store.join("modeld.db");
    let db = Database::open(&db_path)?;

    if !workflow_dir.exists() {
        let p = workflow_dir.display().to_string();
        anyhow::bail!("{}", tf("error.workflow_dir_missing", &[("path", &p)]));
    }

    let p = workflow_dir.display().to_string();
    println!("{}", tf("wf.scan.scanning", &[("path", &p)]).bold());

    // Find all .json files
    let files = find_workflow_files(&workflow_dir)?;
    if files.is_empty() {
        println!("{}", t("wf.scan.no_files").yellow());
        return Ok(());
    }

    println!("  {}", tf("wf.scan.found", &[("count", &files.len())]).bold());
    println!();

    // Build model lookup from database
    let lookup = build_model_lookup(&db)?;
    let lookup_size = lookup.len();
    println!("  {}", tf("wf.scan.lookup", &[("count", &lookup_size)]).bold());
    println!();

    let pb = ProgressBar::new(files.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    let mut total_resolved = 0usize;
    let mut total_unresolved = 0usize;
    let mut errors = 0usize;

    for wf_path in &files {
        let fname = wf_path.file_name().unwrap_or_default().to_string_lossy();
        pb.set_message(fname.to_string());

        match index_workflow(&db, wf_path, &lookup) {
            Ok((_, resolved, unresolved)) => {
                total_resolved += resolved;
                total_unresolved += unresolved;
            }
            Err(e) => {
                errors += 1;
                let wp = wf_path.display().to_string();
                let es = format!("{:#}", e);
                pb.println(format!(
                    "  {}",
                    tf("wf.warn", &[("path", &wp), ("error", &es)]).yellow().bold()
                ));
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    println!("{}", t("wf.scan.complete").green().bold());
    println!("  {}", tf("wf.scan.workflows", &[("count", &files.len())]).bold());
    println!("  {}", tf("wf.scan.resolved", &[("count", &total_resolved)]).green().bold());
    println!("  {}", tf("wf.scan.unresolved", &[("count", &total_unresolved)]).yellow().bold());
    if errors > 0 {
        println!("  {}", tf("wf.scan.errors", &[("count", &errors)]).red().bold());
    }

    Ok(())
}

pub fn workflow_deps_command(store: PathBuf, workflow_file: PathBuf) -> Result<()> {
    if !workflow_file.exists() {
        let p = workflow_file.display().to_string();
        anyhow::bail!("{}", tf("error.workflow_file_missing", &[("path", &p)]));
    }

    let db_path = store.join("modeld.db");

    let p = workflow_file.display().to_string();
    println!("{}", tf("wf.deps.workflow", &[("path", &p)]).bold());
    println!();

    let parsed = parse_workflow(&workflow_file)?;

    if let Some(ref title) = parsed.title {
        println!("  {}", tf("wf.deps.title", &[("title", title)]).bold());
        println!();
    }

    // Try to resolve against DB if available
    let lookup = if db_path.exists() {
        let db = Database::open(&db_path)?;
        build_model_lookup(&db)?
    } else {
        std::collections::HashMap::new()
    };

    // Print known refs
    let all_refs: Vec<_> = parsed.refs.iter().chain(parsed.unresolved.iter()).collect();

    if all_refs.is_empty() {
        println!("{}", t("wf.deps.no_refs").yellow());
        return Ok(());
    }

    println!("{}", t("wf.deps.header").bold());
    println!();

    // Group by ref_type
    let mut by_type: std::collections::BTreeMap<&str, Vec<_>> = std::collections::BTreeMap::new();
    for r in &all_refs {
        by_type.entry(r.ref_type.as_str()).or_default().push(r);
    }

    for (ref_type, refs) in &by_type {
        let type_key = format!("wf.deps.type.{}", ref_type);
        let type_label = t(&type_key);
        println!("  {}:", type_label.bold());
        for r in refs {
            let resolved = lookup.contains_key(&r.model_name)
                || lookup.contains_key(
                    std::path::Path::new(&r.model_name)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                        .as_str(),
                );
            let status = if resolved { "✓".green().bold() } else { "?".yellow() };
            println!("    {} {}", status, r.model_name);
        }
        println!();
    }

    println!(
        "  {}",
        tf(
            "wf.deps.total",
            &[
                ("count", &all_refs.len()),
                ("resolved", &parsed.refs.len()),
                ("unresolved", &parsed.unresolved.len()),
            ]
        )
        .bold(),
    );

    Ok(())
}

pub fn refs_orphans_command(store: PathBuf, json_output: bool) -> Result<()> {
    let db_path = store.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("{}", t("store.not_initialized_exit"));
    }

    let db = Database::open(&db_path)?;
    let orphans = db.orphan_models()?;

    if json_output {
        let arr: Vec<serde_json::Value> = orphans
            .iter()
            .map(|m| {
                serde_json::json!({
                    "hash": m.blake3_hash.as_hex(),
                    "size_bytes": m.size_bytes,
                    "format": m.format,
                    "arch": m.arch,
                })
            })
            .collect();
        crate::output::write_json(&arr)?;
        return Ok(());
    }

    if orphans.is_empty() {
        println!("{}", t("orphans.none").green());
        return Ok(());
    }

    let total_size: i64 = orphans.iter().map(|m| m.size_bytes).sum();

    println!("{}", t("orphans.header").bold().yellow());
    println!();
    println!(
        "  {:<66} {:>10}  {}",
        t("orphans.col_hash").dimmed(),
        t("orphans.col_size").dimmed(),
        t("orphans.col_format").dimmed()
    );
    println!("  {}", "─".repeat(90).dimmed());

    for m in &orphans {
        let hash_str = &m.blake3_hash.as_hex()[..16];
        let size_str = format_bytes(m.size_bytes as u64);
        let fmt = m.format.as_deref().unwrap_or("unknown");
        println!("  {}...  {:>10}  {}", hash_str.yellow(), size_str, fmt);
    }

    println!();
    let size = format_bytes(total_size as u64);
    println!("  {}", tf("orphans.summary", &[("count", &orphans.len()), ("size", &size)]).bold());
    println!();
    println!("{}", t("orphans.tip").dimmed());

    Ok(())
}
