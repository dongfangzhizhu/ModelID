//! Virtual filesystem for AI frontends (RFC 0006, C7).
//!
//! Builds the directory shapes ComfyUI / Forge / A1111 expect under
//! `<store>/virtual/<frontend>/` and fills them with zero-copy hardlinks (or
//! symlinks when the CAS is on another volume). CAS stays the single source of
//! truth; existing links are left untouched, so a sync is safe to repeat.

use crate::cas::CasStore;
use crate::db::{AliasType, Database, Frontend, Model};
use crate::hash::Blake3Hash;
use crate::links::{is_same_volume, replace_with_link, LinkCapability, LinkResult};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Built-in frontend names accepted by `modeld frontend` / `modeld sync-virtual`.
pub const BUILTIN_FRONTENDS: [&str; 3] = ["comfyui", "forge", "a1111"];

/// A minimal as-built template: canonical category → frontend-relative path.
#[derive(Debug, Clone)]
pub struct FrontendTemplate {
    pub name: &'static str,
    pub display_name: &'static str,
    pub frontend: Frontend,
    categories: &'static [(&'static str, &'static str)],
    pub setup: &'static str,
}

impl FrontendTemplate {
    pub fn category_path(&self, canonical: &str) -> &'static str {
        self.categories
            .iter()
            .find(|(key, _)| *key == canonical)
            .map(|(_, path)| *path)
            .unwrap_or("checkpoints")
    }
}

const COMFYUI_CATEGORIES: &[(&str, &str)] = &[
    ("checkpoint", "checkpoints"),
    ("lora", "loras"),
    ("vae", "vae"),
    ("embedding", "embeddings"),
    ("controlnet", "controlnet"),
    ("upscale", "upscale_models"),
    ("clip", "clip"),
];

const FORGE_CATEGORIES: &[(&str, &str)] = &[
    ("checkpoint", "models/Stable-diffusion"),
    ("lora", "models/Lora"),
    ("vae", "models/VAE"),
    ("embedding", "embeddings"),
    ("controlnet", "models/ControlNet"),
    ("upscale", "models/ESRGAN"),
    ("clip", "models/CLIP"),
];

const A1111_CATEGORIES: &[(&str, &str)] = &[
    ("checkpoint", "models/Stable-diffusion"),
    ("lora", "models/Lora"),
    ("vae", "models/VAE"),
    ("embedding", "embeddings"),
    ("controlnet", "models/ControlNet"),
    ("upscale", "models/ESRGAN"),
    ("clip", "models/CLIP"),
];

/// Resolve a built-in frontend template by name.
pub fn template(name: &str) -> Option<FrontendTemplate> {
    match name.to_ascii_lowercase().as_str() {
        "comfyui" => Some(FrontendTemplate {
            name: "comfyui",
            display_name: "ComfyUI",
            frontend: Frontend::ComfyUI,
            categories: COMFYUI_CATEGORIES,
            setup: "Point ComfyUI's extra_model_paths.yaml at <store>/virtual/comfyui, \
                    or symlink its models directory to that folder.",
        }),
        "forge" => Some(FrontendTemplate {
            name: "forge",
            display_name: "Stable Diffusion WebUI Forge",
            frontend: Frontend::Forge,
            categories: FORGE_CATEGORIES,
            setup: "Point Forge's models directory at <store>/virtual/forge/models.",
        }),
        "a1111" => Some(FrontendTemplate {
            name: "a1111",
            display_name: "Automatic1111 WebUI",
            frontend: Frontend::A1111,
            categories: A1111_CATEGORIES,
            setup: "Point stable-diffusion-webui/models at <store>/virtual/a1111/models.",
        }),
        _ => None,
    }
}

/// Outcome of one virtual-filesystem sync.
#[derive(Debug, Default)]
pub struct VirtualSyncReport {
    pub created: usize,
    pub valid: usize,
    pub missing_cas: usize,
    pub failed: Vec<String>,
}

/// Classify a model into a canonical category using name/category hints.
/// Unknown models conservatively land in `checkpoint` (the common case).
pub fn canonical_category(model: &Model) -> &'static str {
    let mut haystack = String::new();
    if let Some(name) = &model.original_filename {
        haystack.push_str(name);
        haystack.push(' ');
    }
    if let Some(category) = &model.category {
        haystack.push_str(category);
    }
    let lower = haystack.to_ascii_lowercase();

    if lower.contains("lora") || lower.contains("lycoris") || lower.contains("locon") {
        "lora"
    } else if lower.contains("vae") {
        "vae"
    } else if lower.contains("controlnet") || lower.contains("control_v11") || lower.contains("t2i")
    {
        "controlnet"
    } else if lower.contains("embedding") || lower.contains("textual") || lower.contains("ti_") {
        "embedding"
    } else if lower.contains("esrgan") || lower.contains("upscale") || lower.contains("ultrasharp")
    {
        "upscale"
    } else if lower.contains("clip") || lower.contains("t5") || lower.contains("text_encoder") {
        "clip"
    } else {
        "checkpoint"
    }
}

fn safe_filename(model: &Model, hash: &Blake3Hash) -> String {
    let candidate = model
        .original_filename
        .as_deref()
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != "." && *name != "..");

    let raw: String = candidate.map(str::to_string).unwrap_or_else(|| {
        let ext = model.format.as_deref().unwrap_or("safetensors");
        format!("{}.{}", &hash.as_hex()[..16], ext)
    });
    let mut out: String =
        raw.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
    if out.len() > 160 {
        out.truncate(160);
    }
    out
}

fn unique_name(base: &str, hash: &Blake3Hash, used: &mut HashMap<String, usize>) -> String {
    let count = used.entry(base.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base.to_string()
    } else {
        let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
        let ext = base.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        if ext.is_empty() {
            format!("{}_{}", stem, &hash.as_hex()[..8])
        } else {
            format!("{}_{}.{}", stem, &hash.as_hex()[..8], ext)
        }
    }
}

/// Build / refresh the virtual directories for the selected frontends.
pub fn sync(store_path: &Path, db: &mut Database, names: &[String]) -> Result<VirtualSyncReport> {
    let mut report = VirtualSyncReport::default();
    let templates: Vec<FrontendTemplate> = names
        .iter()
        .map(|name| template(name).with_context(|| format!("unknown frontend template: {name}")))
        .collect::<Result<_>>()?;

    let cas = CasStore::new(store_path);
    let capability = LinkCapability::detect();
    let mut models = db.list_models(None)?;
    models.sort_by(|a, b| a.blake3_hash.as_hex().cmp(b.blake3_hash.as_hex()));

    for frontend in templates {
        let mut used = HashMap::new();
        for model in &models {
            if model.quarantined_at.is_some() {
                continue;
            }
            let Some(cas_path) = cas.get(&model.blake3_hash) else {
                report.missing_cas += 1;
                continue;
            };

            let category = canonical_category(model);
            let filename = unique_name(
                &safe_filename(model, &model.blake3_hash),
                &model.blake3_hash,
                &mut used,
            );
            let mut virtual_path = store_path.join("virtual").join(frontend.name);
            for part in Path::new(frontend.category_path(category)).components() {
                virtual_path.push(part);
            }
            virtual_path.push(filename);

            if virtual_path.exists() || virtual_path.symlink_metadata().is_ok() {
                report.valid += 1;
                continue;
            }

            let kind = if is_same_volume(&virtual_path, &cas_path) {
                AliasType::Hardlink
            } else if capability.has_symlink_privilege {
                AliasType::Symlink
            } else {
                report.failed.push(format!(
                    "{}: cross-volume link needs symlink privilege",
                    virtual_path.display()
                ));
                continue;
            };

            match replace_with_link(&virtual_path, &cas_path, kind) {
                LinkResult::Success(alias_type) => {
                    report.created += 1;
                    let path = virtual_path.to_string_lossy().to_string();
                    if db.get_alias_by_path(&path)?.is_none() {
                        db.insert_alias(
                            &model.blake3_hash,
                            &path,
                            frontend.frontend.clone(),
                            alias_type,
                        )?;
                    }
                }
                LinkResult::Failed(error) => report.failed.push(error),
            }
        }
    }

    Ok(report)
}

/// Point a frontend models directory at `<store>/virtual/<frontend>`.
/// Refuses to replace a non-empty directory so no existing models are hidden.
pub fn connect(store_path: &Path, name: &str, target: &Path) -> Result<PathBuf> {
    let frontend = template(name).with_context(|| format!("unknown frontend template: {name}"))?;
    let virtual_dir = store_path.join("virtual").join(frontend.name);
    std::fs::create_dir_all(&virtual_dir)?;

    if let Ok(existing) = std::fs::read_link(target) {
        if existing == virtual_dir {
            return Ok(virtual_dir);
        }
        bail!("{} already points at {}", target.display(), existing.display());
    }
    if target.exists() {
        let mut entries = std::fs::read_dir(target)?;
        if entries.next().is_some() {
            bail!("refusing to replace non-empty directory {}", target.display());
        }
        std::fs::remove_dir(target)?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&virtual_dir, target)
        .with_context(|| format!("cannot symlink {} (enable Developer Mode)", target.display()))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&virtual_dir, target)
        .with_context(|| format!("cannot symlink {}", target.display()))?;

    Ok(virtual_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_file;
    use tempfile::{NamedTempFile, TempDir};

    fn model_with_name(name: &str, category: &str) -> Model {
        Model {
            id: 1,
            blake3_hash: Blake3Hash::from_hex(&"ab".repeat(32)).unwrap(),
            size_bytes: 4,
            format: Some("safetensors".to_string()),
            arch: None,
            category: Some(category.to_string()),
            base_model: None,
            created_at: chrono::Utc::now(),
            last_seen: chrono::Utc::now(),
            quarantined_at: None,
            note: None,
            pinned: false,
            favorited: false,
            source_type: None,
            hf_repo_id: None,
            revision: None,
            download_url: None,
            license: None,
            downloaded_by: None,
            downloaded_at: None,
            original_filename: Some(name.to_string()),
            model_card_url: None,
        }
    }

    #[test]
    fn templates_cover_all_three_frontends() {
        assert_eq!(BUILTIN_FRONTENDS.len(), 3);
        for name in BUILTIN_FRONTENDS {
            assert!(template(name).is_some());
        }
        assert_eq!(template("comfyui").unwrap().category_path("lora"), "loras");
        assert_eq!(
            template("a1111").unwrap().category_path("checkpoint"),
            "models/Stable-diffusion"
        );
    }

    #[test]
    fn category_detection_uses_filename_hints() {
        assert_eq!(canonical_category(&model_with_name("sdxl-lora-v2.safetensors", "")), "lora");
        assert_eq!(canonical_category(&model_with_name("vae-ft-mse.safetensors", "")), "vae");
        assert_eq!(canonical_category(&model_with_name("model.safetensors", "")), "checkpoint");
    }

    #[test]
    fn sync_creates_zero_copy_links_and_records_aliases() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path();
        let source = NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"model").unwrap();
        let hash = hash_file(source.path()).unwrap();
        let _cas_path = CasStore::new(store).store(source.path(), &hash).unwrap();

        let mut db = Database::open(&store.join("modeld.db")).unwrap();
        let mut model = model_with_name("sdxl-lora-v2.safetensors", "lora");
        model.blake3_hash = hash.clone();
        db.insert_or_update_model(&hash, 5, Some("safetensors"), None, Some("lora"), None).unwrap();
        db.insert_alias(
            &hash,
            source.path().to_str().unwrap(),
            Frontend::User,
            AliasType::Original,
        )
        .unwrap();
        db.set_model_provenance(
            &hash,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("sdxl-lora-v2.safetensors"),
            None,
        )
        .unwrap();

        let names = vec!["comfyui".to_string(), "a1111".to_string()];
        let report = sync(store, &mut db, &names).unwrap();
        assert_eq!(report.created, 2, "one link per selected frontend");
        assert!(report.failed.is_empty());

        let comfy =
            store.join("virtual").join("comfyui").join("loras").join("sdxl-lora-v2.safetensors");
        let a1111 = store
            .join("virtual")
            .join("a1111")
            .join("models")
            .join("Lora")
            .join("sdxl-lora-v2.safetensors");
        assert!(comfy.exists());
        assert!(a1111.exists());
        assert!(db.get_alias_by_path(comfy.to_str().unwrap()).unwrap().is_some());
        assert!(db.get_alias_by_path(a1111.to_str().unwrap()).unwrap().is_some());

        // Re-running must be a no-op, never a second alias row.
        let second = sync(store, &mut db, &names).unwrap();
        assert_eq!(second.created, 0);
        assert_eq!(second.valid, 2);

        // The links must share bytes with CAS, not copy it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&comfy).unwrap().ino(),
                std::fs::metadata(&_cas_path).unwrap().ino()
            );
        }
    }
}
