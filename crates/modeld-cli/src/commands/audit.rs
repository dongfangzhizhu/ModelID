//! `modeld audit` — 查询 append-only 审计日志（C2）。
//!
//! 验收场景「上周谁删了什么」：
//!   modeld audit --since 7d --op unlink --json
//! 每条记录带 UTC 时间、操作者（local 或 IP）、资源、结果与可选详情。
//! Token 值永不写入审计日志，查询输出也不会泄露 token。

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use modeld_core::{AuditLogger, AuditQuery};
use std::path::Path;

/// CLI 解析后的查询参数。
pub struct AuditOptions {
    pub since: Option<String>,
    pub until: Option<String>,
    pub op: Option<String>,
    pub actor: Option<String>,
    pub outcome: Option<String>,
    pub resource_contains: Option<String>,
    pub limit: Option<usize>,
    pub json: bool,
}

/// 把 `7d` / `2h` / ISO 8601 解析成 UTC 时间。
fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(iso) = DateTime::parse_from_rfc3339(value) {
        return Ok(iso.with_timezone(&Utc));
    }
    let (num, unit) = if let Some(rest) = value.strip_suffix('d') {
        (rest, 24i64 * 3600)
    } else if let Some(rest) = value.strip_suffix('h') {
        (rest, 3600i64)
    } else if let Some(rest) = value.strip_suffix('m') {
        (rest, 60i64)
    } else {
        anyhow::bail!("invalid time '{value}' (expected ISO 8601 or e.g. 7d / 2h / 30m)");
    };
    let amount: i64 = num.parse().context("invalid duration number")?;
    Ok(Utc::now() - Duration::seconds(amount.saturating_mul(unit)))
}

pub fn audit_command(store_path: &Path, opts: AuditOptions) -> Result<()> {
    let logger = AuditLogger::new(store_path);
    let query = AuditQuery {
        since: opts.since.as_deref().map(parse_time).transpose()?,
        until: opts.until.as_deref().map(parse_time).transpose()?,
        op: opts.op,
        actor_ip: opts.actor,
        outcome: opts.outcome,
        resource_contains: opts.resource_contains,
        limit: opts.limit,
    };
    let entries = logger.query(&query)?;

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("没有匹配的审计记录");
    } else {
        for e in &entries {
            println!(
                "{}  {:<14} {:<16} {:<9} {}",
                e.timestamp.to_rfc3339(),
                e.op,
                e.actor_ip,
                e.outcome,
                e.resource
            );
            if let Some(details) = &e.details {
                println!("    {details}");
            }
        }
        println!("\n共 {} 条记录", entries.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_relative_durations() {
        let now = Utc::now();
        let d7 = parse_time("7d").unwrap();
        assert!(now - d7 >= Duration::days(7) - Duration::seconds(2));
        assert!(now - d7 <= Duration::days(7) + Duration::seconds(2));
        assert!(now - parse_time("2h").unwrap() >= Duration::hours(2) - Duration::seconds(2));
        assert!(now - parse_time("30m").unwrap() >= Duration::minutes(30) - Duration::seconds(2));
    }

    #[test]
    fn parse_iso_and_reject_garbage() {
        let t = parse_time("2026-11-26T00:00:00Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-11-26T00:00:00+00:00");
        assert!(parse_time("yesterday").is_err());
        assert!(parse_time("7x").is_err());
    }
}
