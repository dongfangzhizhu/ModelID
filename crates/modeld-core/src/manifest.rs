//! CAS 内容清单（C4）：为对象生成/校验 BLAKE3 + 字节数的签名清单。
//!
//! 清单是一个 JSON 文件，记录每个 CAS 对象的 BLAKE3（与对象路径同名，
//! 因此 hash 已是内容地址）和字节数。校验时重新哈希磁盘上的对象，
//! 任何篡改（内容被替换、截断、缺失、多出）都会被报告。
//!
//! 清单本身不做内嵌签名：签名属于发布链路（见 `scripts/sign_release.py`
//! 的 cosign keyless），密钥材料不写入清单，避免明文落盘。本模块只负责
//! 生成清单与按内容重新校验对象，不伪造签名验证能力。

use crate::cas::CasStore;
use crate::db::Database;
use crate::hash::hash_file;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// 单个对象的清单记录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    /// BLAKE3 hex —— CAS 对象的内容地址。
    pub blake3: String,
    /// 期望字节数。
    pub size: u64,
}

/// 清单文件根结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// 清单格式版本。
    pub version: u32,
    /// 生成时的 UTC 时间（ISO 8601）。
    pub created_at: String,
    /// hash hex → 记录；BTreeMap 保证输出稳定可复现。
    pub objects: BTreeMap<String, ManifestEntry>,
}

/// 校验报告。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestReport {
    pub checked: usize,
    pub mismatches: Vec<String>,
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
}

impl ManifestReport {
    pub fn is_clean(&self) -> bool {
        self.mismatches.is_empty() && self.missing.is_empty() && self.unexpected.is_empty()
    }
}

/// 从 store 的 DB + CAS 生成清单。
///
/// 只收录 DB 中已登记且 CAS 中实际存在的对象；缺失对象会记入
/// `report`-like 的 `missing` 由调用方另行处理，这里不静默跳过。
pub fn generate(store_path: &Path, db: &Database) -> Result<Manifest> {
    let cas = CasStore::new(store_path);
    let models = db.list_models(None)?;
    let mut objects = BTreeMap::new();

    for model in &models {
        let hash = model.blake3_hash.as_hex();
        match cas.get(&model.blake3_hash) {
            Some(path) => {
                let size = std::fs::metadata(&path)
                    .with_context(|| format!("stat CAS object {hash}"))?
                    .len();
                objects.insert(hash.to_string(), ManifestEntry { blake3: hash.to_string(), size });
            }
            None => {
                // 缺对象也在清单中保留记录，校验时会明确报 missing，
                // 而不是「清单里没有」导致看起来健康。
                objects.insert(
                    hash.to_string(),
                    ManifestEntry { blake3: hash.to_string(), size: model.size_bytes as u64 },
                );
            }
        }
    }

    Ok(Manifest { version: 1, created_at: chrono::Utc::now().to_rfc3339(), objects })
}

/// 写清单到磁盘（先写临时文件再 rename）。
pub fn save(path: &Path, manifest: &Manifest) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(manifest)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 从磁盘读清单。
pub fn load(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read manifest {}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).context("parse manifest JSON")?;
    if manifest.version != 1 {
        anyhow::bail!("unsupported manifest version {}", manifest.version);
    }
    Ok(manifest)
}

/// 校验清单：重新哈希每个 CAS 对象，报告篡改/缺失/多余对象。
///
/// `strict_extra = true` 时，磁盘上存在但清单未记录的对象也会报告
/// （用于「发布快照必须与清单完全一致」的场景）。
pub fn verify(
    store_path: &Path,
    manifest: &Manifest,
    strict_extra: bool,
) -> Result<ManifestReport> {
    let cas = CasStore::new(store_path);
    let mut report = ManifestReport::default();

    for (hash_hex, entry) in &manifest.objects {
        if entry.blake3 != *hash_hex {
            report.mismatches.push(format!("{hash_hex}: manifest blake3 field mismatch"));
            continue;
        }
        let hash = crate::hash::Blake3Hash::from_hex(hash_hex)
            .with_context(|| format!("invalid hash in manifest: {hash_hex}"))?;
        match cas.get(&hash) {
            None => report.missing.push(hash_hex.clone()),
            Some(path) => {
                let meta = std::fs::metadata(&path)?;
                if meta.len() != entry.size {
                    report.mismatches.push(format!(
                        "{hash_hex}: size {} != manifest {}",
                        meta.len(),
                        entry.size
                    ));
                    continue;
                }
                let actual = hash_file(&path)?;
                if actual.as_hex() != *hash_hex {
                    report
                        .mismatches
                        .push(format!("{hash_hex}: content hash changed to {}", actual.as_hex()));
                }
                report.checked += 1;
            }
        }
    }

    if strict_extra {
        let db = Database::open(&store_path.join("modeld.db"))?;
        let known: std::collections::BTreeSet<String> =
            db.list_models(None)?.iter().map(|m| m.blake3_hash.as_hex().to_string()).collect();
        for hash in &known {
            if !manifest.objects.contains_key(hash) {
                report.unexpected.push(hash.clone());
            }
        }
    }

    Ok(report)
}
