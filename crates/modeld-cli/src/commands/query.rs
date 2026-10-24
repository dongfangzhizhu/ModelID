//! Read-only queries: `status`, `stats`, `dupes`, `list`, `info`.

use crate::{csv_escape, parse_size, require_store_db};
use anyhow::{Context, Result};
use colored::*;
use modeld_core::{get_tags_for_model, t, tf, CasStore, Database, DedupEngine, QuarantineManager};
use std::path::PathBuf;

pub fn status_command(store_path: PathBuf) -> Result<()> {
    let db_path = store_path.join("modeld.db");

    if !db_path.exists() {
        eprintln!("{}", t("store.not_initialized").red());
        std::process::exit(1);
    }

    let db = Database::open(&db_path)?;

    let count = db.count_models()?;
    let total_size = db.total_size()?;

    println!("{}", t("status.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  {}", tf("status.store_path", &[("path", &store_path.display())]));
    println!("  {}", tf("status.total_models", &[("count", &count)]).bold());
    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("  {}", tf("status.total_size", &[("gb", &gb)]).bold());
    println!("{}", "─".repeat(40).cyan());

    if count > 0 {
        println!("\n{}", t("status.recent").cyan());
        let models = db.list_models(Some(5))?;
        for model in models {
            let hash = model.blake3_hash.as_hex()[..16].to_string();
            let mb = format!("{:.2}", model.size_bytes as f64 / 1_048_576.0);
            println!("  {}", tf("status.recent_line", &[("hash", &hash), ("mb", &mb)]).dimmed());
        }
    }

    // Show quarantine stats
    let qm = QuarantineManager::new(&store_path);
    if let Ok(qstats) = qm.stats() {
        if qstats.total_files > 0 {
            println!("\n{}", t("status.quarantine").yellow());
            println!("  {}", tf("status.q_files", &[("count", &qstats.total_files)]).bold());
            let mb = format!("{:.2}", qstats.total_size as f64 / 1_048_576.0);
            println!("  {}", tf("status.q_size", &[("mb", &mb)]));
            if qstats.expired_files > 0 {
                println!("  {}", tf("status.q_expired", &[("count", &qstats.expired_files)]).red());
            }
        }
    }

    Ok(())
}

pub fn stats_command(store_path: PathBuf) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let count = db.count_models()?;
    let total_size = db.total_size()?;
    let aliases = db.list_models(None)?.iter().try_fold(0i64, |acc, model| {
        Ok::<_, anyhow::Error>(acc + db.count_aliases(&model.blake3_hash)?)
    })?;

    let engine = DedupEngine::new(db, store_path.clone());
    let groups = engine.find_duplicates()?;
    let duplicate_bytes = engine.calculate_savings(&groups);

    println!("{}", t("stats.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  {}", tf("stats.store_path", &[("path", &store_path.display())]));
    println!("  {}", tf("stats.models", &[("count", &count)]).bold());
    println!("  {}", tf("stats.aliases", &[("count", &aliases)]).bold());
    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("  {}", tf("stats.indexed_size", &[("gb", &gb)]));
    println!("  {}", tf("stats.dup_groups", &[("count", &groups.len())]).bold());
    let gb = format!("{:.2}", duplicate_bytes as f64 / 1_073_741_824.0);
    println!("  {}", tf("stats.savings", &[("gb", &gb)]));
    println!("{}", "─".repeat(40).cyan());

    Ok(())
}

pub fn dupes_command(
    store_path: PathBuf,
    min_size: Option<String>,
    json_output: bool,
) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let engine = DedupEngine::new(db, store_path);
    let min_size_bytes = min_size.as_deref().map(parse_size).transpose()?.unwrap_or(0);

    let groups: Vec<_> =
        engine.find_duplicates()?.into_iter().filter(|g| g.total_size >= min_size_bytes).collect();
    let duplicate_bytes = engine.calculate_savings(&groups);

    if json_output {
        let groups_json: Vec<_> = groups
            .iter()
            .map(|group| {
                serde_json::json!({
                    "hash": group.hash.as_hex(),
                    "size_bytes": group.total_size,
                    "duplicate_count": group.files.len(),
                    "potential_savings_bytes": group.total_size.saturating_mul(group.files.len().saturating_sub(1) as u64),
                    "files": group.files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "duplicate_groups": groups_json,
                "total_groups": groups.len(),
                "potential_savings_bytes": duplicate_bytes,
            })
        );
        return Ok(());
    }

    println!("{}", t("dupes.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    if groups.is_empty() {
        println!("{}", t("dupes.none").green());
        return Ok(());
    }

    for group in &groups {
        let hash = group.hash.as_hex()[..16].to_string();
        let mb = format!("{:.2}", group.total_size as f64 / 1_048_576.0);
        println!(
            "\n{}",
            tf("dupes.group_line", &[("hash", &hash), ("mb", &mb), ("count", &group.files.len())])
                .bold()
        );
        for file in &group.files {
            let path = file.path.display().to_string();
            println!("  {}", tf("dupes.file_line", &[("path", &path)]));
        }
    }
    println!("{}", "─".repeat(40).cyan());
    let gb = format!("{:.2}", duplicate_bytes as f64 / 1_073_741_824.0);
    println!("{}", tf("dupes.savings", &[("gb", &gb)]));

    Ok(())
}

pub fn list_command(
    store_path: PathBuf,
    limit: Option<i64>,
    json_output: bool,
    export_csv: Option<PathBuf>,
) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let models = db.list_models(limit)?;

    // ── CSV export ──────────────────────────────────────────────────────────
    if let Some(csv_path) = export_csv {
        use std::io::Write as IoWrite;
        let mut file = std::fs::File::create(&csv_path)
            .with_context(|| format!("Failed to create CSV file: {}", csv_path.display()))?;

        // Header
        writeln!(file, "hash,size,format,source_type,hf_repo_id,license,downloaded_at")?;

        for m in &models {
            let hash = m.blake3_hash.as_hex();
            let size = m.size_bytes;
            let format = csv_escape(m.format.as_deref().unwrap_or(""));
            let source_type = csv_escape(m.source_type.as_deref().unwrap_or("local"));
            let hf_repo_id = csv_escape(m.hf_repo_id.as_deref().unwrap_or(""));
            let license = csv_escape(m.license.as_deref().unwrap_or(""));
            let downloaded_at = m.downloaded_at.map(|d| d.to_rfc3339()).unwrap_or_default();
            writeln!(
                file,
                "{},{},{},{},{},{},{}",
                hash, size, format, source_type, hf_repo_id, license, downloaded_at
            )?;
        }

        println!(
            "{} Exported {} model(s) to {}",
            "✓".green().bold(),
            models.len(),
            csv_path.display()
        );
        return Ok(());
    }

    if json_output {
        let rows: Vec<_> = models
            .iter()
            .map(|m| {
                serde_json::json!({
                    "hash": m.blake3_hash.as_hex(),
                    "size_bytes": m.size_bytes,
                    "format": m.format,
                    "arch": m.arch,
                    "category": m.category,
                    "base_model": m.base_model,
                    "created_at": m.created_at.to_rfc3339(),
                    "last_seen": m.last_seen.to_rfc3339(),
                    "quarantined_at": m.quarantined_at.map(|d| d.to_rfc3339()),
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "models": rows, "total": rows.len() }));
        return Ok(());
    }

    println!("{}", t("list.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    if models.is_empty() {
        println!("{}", t("list.none").yellow());
        return Ok(());
    }
    for model in models {
        let hash = model.blake3_hash.as_hex()[..16].to_string();
        let mb = format!("{:>9.2}", model.size_bytes as f64 / 1_048_576.0);
        let format = model.format.as_deref().unwrap_or("unknown");
        println!(
            "{}",
            tf("list.line", &[("hash", &hash), ("mb", &mb), ("format", &format)]).bold()
        );
    }

    Ok(())
}

pub fn info_command(store_path: PathBuf, hash_hex: &str, json_output: bool) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let hash = modeld_core::Blake3Hash::from_hex(hash_hex)?;
    let Some(model) = db.get_model(&hash)? else {
        anyhow::bail!("{}", tf("error.model_not_found", &[("hash", &hash_hex)]));
    };
    let aliases = db.get_aliases_for_model(&hash)?;
    let cas_path = CasStore::new(&store_path).get(&hash);
    let tags = get_tags_for_model(&db, &hash)?;

    if json_output {
        let aliases_json: Vec<_> = aliases
            .iter()
            .map(|a| {
                serde_json::json!({
                    "path": a.path,
                    "frontend": a.frontend.as_str(),
                    "alias_type": a.alias_type.as_str(),
                    "created_at": a.created_at.to_rfc3339(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "hash": model.blake3_hash.as_hex(),
                "size_bytes": model.size_bytes,
                "format": model.format,
                "arch": model.arch,
                "category": model.category,
                "base_model": model.base_model,
                "created_at": model.created_at.to_rfc3339(),
                "last_seen": model.last_seen.to_rfc3339(),
                "quarantined_at": model.quarantined_at.map(|d| d.to_rfc3339()),
                "cas_path": cas_path.map(|p| p.display().to_string()),
                "aliases": aliases_json,
                // Governance fields
                "note": model.note,
                "pinned": model.pinned,
                "favorited": model.favorited,
                "tags": tags,
                // Provenance fields
                "source_type": model.source_type,
                "hf_repo_id": model.hf_repo_id,
                "revision": model.revision,
                "download_url": model.download_url,
                "license": model.license,
                "downloaded_at": model.downloaded_at.map(|d| d.to_rfc3339()),
                "original_filename": model.original_filename,
                "model_card_url": model.model_card_url,
            })
        );
        return Ok(());
    }

    println!("{}", t("info.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    let hash_str = model.blake3_hash.as_hex().to_string();
    println!("  {}", tf("info.hash", &[("hash", &hash_str)]).bold());
    let mb = format!("{:.2}", model.size_bytes as f64 / 1_048_576.0);
    println!("  {}", tf("info.size", &[("mb", &mb)]));
    let format = model.format.as_deref().unwrap_or("unknown");
    println!("  {}", tf("info.format", &[("value", &format)]));
    let category = model.category.as_deref().unwrap_or("unknown");
    println!("  {}", tf("info.category", &[("value", &category)]));
    let last_seen = model.last_seen.to_rfc3339();
    println!("  {}", tf("info.last_seen", &[("value", &last_seen)]));
    if let Some(path) = cas_path {
        let p = path.display().to_string();
        println!("  {}", tf("info.cas_path", &[("path", &p)]));
    }

    // ── Governance ──────────────────────────────────────────────────────────
    println!("\n{}", "Governance".cyan());
    let pin_icon = if model.pinned {
        "📌 pinned".green().to_string()
    } else {
        "  not pinned".dimmed().to_string()
    };
    let fav_icon = if model.favorited {
        "⭐ favorited".yellow().to_string()
    } else {
        "  not favorited".dimmed().to_string()
    };
    println!("  {}", pin_icon);
    println!("  {}", fav_icon);
    if let Some(ref note) = model.note {
        println!("  note: {}", note);
    }
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }

    // ── Provenance ───────────────────────────────────────────────────────────
    let has_provenance = model.hf_repo_id.is_some()
        || model.source_type.as_deref().map(|s| s != "local").unwrap_or(false)
        || model.original_filename.is_some()
        || model.license.is_some()
        || model.downloaded_at.is_some();

    if has_provenance {
        println!("\n{}", "Provenance".cyan());
        if let Some(ref src) = model.source_type {
            println!("  source:   {}", src);
        }
        if let Some(ref repo) = model.hf_repo_id {
            println!("  hf_repo:  {}", repo);
        }
        if let Some(ref rev) = model.revision {
            println!("  revision: {}", rev);
        }
        if let Some(ref fname) = model.original_filename {
            println!("  filename: {}", fname);
        }
        if let Some(ref lic) = model.license {
            println!("  license:  {}", lic);
        }
        if let Some(ref dl_at) = model.downloaded_at {
            println!("  downloaded: {}", dl_at.to_rfc3339());
        }
        if let Some(ref url) = model.model_card_url {
            println!("  model card: {}", url.dimmed());
        }
    }

    println!("\n{}", t("info.aliases").cyan());
    if aliases.is_empty() {
        println!("{}", t("info.aliases_none"));
    } else {
        for alias in aliases {
            let path = alias.path.clone();
            let frontend = alias.frontend.as_str().to_string();
            let kind = alias.alias_type.as_str().to_string();
            println!(
                "  {}",
                tf("info.alias_line", &[("path", &path), ("frontend", &frontend), ("kind", &kind)])
            );
        }
    }

    Ok(())
}
