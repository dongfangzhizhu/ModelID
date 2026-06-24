//! ComfyUI Workflow Parser (Phase 4)
//!
//! Parses ComfyUI `workflow.json` files to extract model dependencies.
//!
//! ## ComfyUI JSON Format
//!
//! ```json
//! {
//!   "nodes": [
//!     {
//!       "id": 4,
//!       "type": "CheckpointLoaderSimple",
//!       "inputs": [["ckpt_name", 0, "v1-5-pruned.safetensors"]],
//!       "widgets_values": ["v1-5-pruned.safetensors", ...]
//!     }
//!   ]
//! }
//! ```
//!
//! Note: ComfyUI has no formal JSON schema. This parser uses heuristics based
//! on known node types and handles multiple format variants.

use crate::db::Database;
use crate::hash::Blake3Hash;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Known node types → model ref type mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Maps ComfyUI node type → (ref_type, input_key, widget_index)
/// widget_index: which widgets_values element holds the model name (-1 = first string found)
static KNOWN_LOADERS: &[(&str, &str, &str)] = &[
    // Checkpoints
    ("CheckpointLoaderSimple", "checkpoint", "ckpt_name"),
    ("CheckpointLoader", "checkpoint", "ckpt_name"),
    ("unCLIPCheckpointLoader", "checkpoint", "ckpt_name"),
    // LoRA
    ("LoraLoader", "lora", "lora_name"),
    ("LoraLoaderModelOnly", "lora", "lora_name"),
    ("Power Lora Loader (rgthree)", "lora", "lora_name"),
    // VAE
    ("VAELoader", "vae", "vae_name"),
    // CLIP
    ("CLIPLoader", "clip", "clip_name"),
    ("DualCLIPLoader", "clip", "clip_name1"),
    // ControlNet
    ("ControlNetLoader", "controlnet", "control_net_name"),
    ("DiffControlNetLoader", "controlnet", "control_net_name"),
    // IPAdapter
    ("IPAdapterModelLoader", "ipadapter", "ipadapter_file"),
    ("IPAdapterUnifiedLoader", "ipadapter", "preset"),
    // UNET / Diffusion model
    ("UNETLoader", "unet", "unet_name"),
    ("ModelSamplingFlux", "unet", "unet_name"),
    // Upscale models
    ("UpscaleModelLoader", "upscale_model", "model_name"),
];

fn loader_for_node_type(node_type: &str) -> Option<(&'static str, &'static str)> {
    KNOWN_LOADERS
        .iter()
        .find(|(t, _, _)| t.eq_ignore_ascii_case(node_type))
        .map(|(_, ref_type, input_key)| (*ref_type, *input_key))
}

// ─────────────────────────────────────────────────────────────────────────────
// Parsed model reference
// ─────────────────────────────────────────────────────────────────────────────

/// A model reference extracted from a workflow
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRef {
    /// Node type (e.g. "CheckpointLoaderSimple")
    pub node_type: String,
    /// Categorized ref type (e.g. "checkpoint", "lora")
    pub ref_type: String,
    /// Raw model name from the workflow (e.g. "v1-5-pruned.safetensors")
    pub model_name: String,
    /// Whether the ref was matched to a known node type
    pub is_known: bool,
}

/// Result of parsing a single workflow file
#[derive(Debug)]
pub struct ParsedWorkflow {
    pub path: PathBuf,
    pub title: Option<String>,
    pub refs: Vec<ModelRef>,
    pub unresolved: Vec<ModelRef>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Parser
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a ComfyUI workflow JSON file and return all model references.
pub fn parse_workflow(path: &Path) -> Result<ParsedWorkflow> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read workflow: {}", path.display()))?;

    let json: Value = serde_json::from_str(&content)
        .with_context(|| format!("Invalid JSON in workflow: {}", path.display()))?;

    let title = extract_title(&json);
    let refs = extract_model_refs(&json);

    let (resolved, unresolved): (Vec<_>, Vec<_>) = refs.into_iter().partition(|r| r.is_known);

    Ok(ParsedWorkflow { path: path.to_path_buf(), title, refs: resolved, unresolved })
}

fn extract_title(json: &Value) -> Option<String> {
    // Try various title locations
    json.get("extra")
        .and_then(|e| e.get("ds"))
        .and_then(|ds| ds.get("title"))
        .or_else(|| json.get("title"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn extract_model_refs(json: &Value) -> Vec<ModelRef> {
    let mut refs = Vec::new();

    // Support both {"nodes": [...]} and flat array formats
    let nodes = if let Some(nodes) = json.get("nodes").and_then(|n| n.as_array()) {
        nodes
    } else if let Some(arr) = json.as_array() {
        arr
    } else {
        return refs;
    };

    for node in nodes {
        extract_refs_from_node(node, &mut refs);
    }

    refs
}

fn extract_refs_from_node(node: &Value, refs: &mut Vec<ModelRef>) {
    let node_type = match node.get("type").and_then(|t| t.as_str()) {
        Some(t) => t.to_string(),
        None => return,
    };

    if let Some((ref_type, input_key)) = loader_for_node_type(&node_type) {
        // Try inputs array: [[key, link_id, value], ...]
        if let Some(name) = extract_from_inputs_array(node, input_key) {
            refs.push(ModelRef {
                node_type: node_type.clone(),
                ref_type: ref_type.to_string(),
                model_name: normalize_model_name(&name),
                is_known: true,
            });
            return;
        }

        // Try inputs object: {"key": "value"}
        if let Some(name) = extract_from_inputs_object(node, input_key) {
            refs.push(ModelRef {
                node_type: node_type.clone(),
                ref_type: ref_type.to_string(),
                model_name: normalize_model_name(&name),
                is_known: true,
            });
            return;
        }

        // Try widgets_values (first string that looks like a model name)
        if let Some(name) = extract_first_model_from_widgets(node) {
            refs.push(ModelRef {
                node_type: node_type.clone(),
                ref_type: ref_type.to_string(),
                model_name: normalize_model_name(&name),
                is_known: true,
            });
        }
    }
    // Unknown node type — don't add, heuristic extraction would be too noisy
}

fn extract_from_inputs_array(node: &Value, input_key: &str) -> Option<String> {
    node.get("inputs").and_then(|v| v.as_array()).and_then(|arr| {
        arr.iter().find_map(|item| {
            if let Some(sub) = item.as_array() {
                if sub.first().and_then(|k| k.as_str()) == Some(input_key) {
                    return sub.get(2).and_then(|v| v.as_str()).map(String::from);
                }
            }
            None
        })
    })
}

fn extract_from_inputs_object(node: &Value, input_key: &str) -> Option<String> {
    node.get("inputs")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get(input_key))
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn extract_first_model_from_widgets(node: &Value) -> Option<String> {
    node.get("widgets_values").and_then(|v| v.as_array()).and_then(|arr| {
        arr.iter().find_map(|v| {
            v.as_str().and_then(|s| {
                let s = s.trim();
                if looks_like_model_filename(s) {
                    Some(s.to_string())
                } else {
                    None
                }
            })
        })
    })
}

/// Heuristic: does this string look like a model filename?
fn looks_like_model_filename(s: &str) -> bool {
    const EXTS: &[&str] = &[".safetensors", ".ckpt", ".pt", ".pth", ".gguf", ".bin", ".pkl"];
    let lower = s.to_lowercase();
    EXTS.iter().any(|ext| lower.ends_with(ext))
}

/// Normalize a model name: convert backslashes, trim whitespace
fn normalize_model_name(name: &str) -> String {
    name.trim().replace('\\', "/")
}

// ─────────────────────────────────────────────────────────────────────────────
// Workflow Scanner
// ─────────────────────────────────────────────────────────────────────────────

/// Scan a directory for ComfyUI workflow JSON files.
/// Returns paths to all `.json` files found.
pub fn find_workflow_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    find_workflow_files_recursive(dir, &mut found)?;
    Ok(found)
}

fn find_workflow_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden dirs
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.starts_with('.') {
                find_workflow_files_recursive(&path, out)?;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// DB Integration
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a workflow file and store its refs in the database.
/// Returns (workflow_id, resolved_count, unresolved_count).
pub fn index_workflow(
    db: &Database,
    path: &Path,
    model_lookup: &HashMap<String, Blake3Hash>,
) -> Result<(i64, usize, usize)> {
    let parsed = parse_workflow(path)?;

    let path_str = path.to_string_lossy().to_string();

    // Compute file hash for change detection
    let file_hash = crate::hash::hash_file(path).ok().map(|h| h.as_hex().to_string());

    let wf_id = db.upsert_workflow(&path_str, file_hash.as_deref(), parsed.title.as_deref())?;

    // Clear old refs and re-insert
    db.clear_workflow_refs(wf_id)?;

    let mut resolved_count = 0;
    let mut unresolved_count = 0;

    for model_ref in parsed.refs.iter().chain(parsed.unresolved.iter()) {
        // Try to find in model lookup (by filename)
        let (hash, model_path, resolved) = resolve_model_ref(&model_ref.model_name, model_lookup);

        if resolved {
            resolved_count += 1;
        } else {
            unresolved_count += 1;
        }

        db.insert_workflow_ref(
            wf_id,
            hash.as_deref(),
            &model_ref.ref_type,
            &model_ref.model_name,
            model_path.as_deref(),
            resolved,
        )?;
    }

    db.update_workflow_ref_count(wf_id)?;

    Ok((wf_id, resolved_count, unresolved_count))
}

/// Try to match a model name to a known BLAKE3 hash.
/// The lookup key is the filename (basename), possibly with subdir prefix.
fn resolve_model_ref(
    model_name: &str,
    lookup: &HashMap<String, Blake3Hash>,
) -> (Option<String>, Option<String>, bool) {
    // Try exact match
    if let Some(hash) = lookup.get(model_name) {
        return (Some(hash.as_hex().to_string()), None, true);
    }

    // Try basename match (strip subdir prefix)
    let basename = model_name.rsplit('/').next().unwrap_or(model_name);
    if let Some(hash) = lookup.get(basename) {
        return (Some(hash.as_hex().to_string()), None, true);
    }

    // Try case-insensitive basename match
    let lower = basename.to_lowercase();
    if let Some((_, hash)) = lookup
        .iter()
        .find(|(k, _)| k.rsplit('/').next().map(|b| b.to_lowercase()) == Some(lower.clone()))
    {
        return (Some(hash.as_hex().to_string()), None, true);
    }

    (None, None, false)
}

/// Build a lookup map: model name (or alias basename) → BLAKE3 hash.
///
/// Used by `index_workflow` to resolve raw model names from workflow JSON to
/// their CAS entries.
///
/// ## Fix (audit 2.2)
///
/// The previous implementation only indexed by `model.format` (which is almost
/// always `None` because `scan` never parses filenames into that field).  This
/// meant every workflow model reference was "unresolved".
///
/// The fix reads the `aliases` table and indexes by **basename of each alias
/// path** — that is the value that workflow JSON files actually reference
/// (e.g. `"v1-5-pruned.safetensors"`).
pub fn build_model_lookup(db: &Database) -> Result<HashMap<String, Blake3Hash>> {
    let mut map = HashMap::new();

    // 1. Index by the raw BLAKE3 hash string (for direct hash lookups)
    let models = db.list_models(None)?;
    for model in &models {
        map.insert(model.blake3_hash.as_hex().to_string(), model.blake3_hash.clone());

        // Fallback: format field when present (may hold a filename-like string)
        if let Some(ref fmt) = model.format {
            map.entry(fmt.clone()).or_insert_with(|| model.blake3_hash.clone());
            if let Some(base) = std::path::Path::new(fmt).file_name() {
                map.entry(base.to_string_lossy().to_string())
                    .or_insert_with(|| model.blake3_hash.clone());
            }
        }
    }

    // 2. Primary source: index by basename of every alias path.
    //    Workflow JSON typically references model files by filename only
    //    (e.g. "v1-5-pruned.safetensors"), which matches the last component
    //    of whatever alias path `scan` recorded.
    let aliases = db.list_all_aliases()?;
    for alias in &aliases {
        let path = std::path::Path::new(&alias.path);

        // Full path → hash (exact match)
        map.entry(alias.path.clone()).or_insert_with(|| alias.model_hash.clone());

        // Basename → hash (most common workflow reference style)
        if let Some(fname) = path.file_name() {
            let name = fname.to_string_lossy().to_string();
            map.entry(name).or_insert_with(|| alias.model_hash.clone());
        }
    }

    Ok(map)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn write_workflow(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(f, "{}", content).unwrap();
        f
    }

    const SIMPLE_WORKFLOW: &str = r#"{
        "last_node_id": 9,
        "nodes": [
            {
                "id": 4,
                "type": "CheckpointLoaderSimple",
                "inputs": [["ckpt_name", 0, "v1-5-pruned-emaonly.safetensors"]],
                "widgets_values": ["v1-5-pruned-emaonly.safetensors", "normal"]
            },
            {
                "id": 5,
                "type": "LoraLoader",
                "inputs": [["lora_name", 0, "epi_noiseoffset2.safetensors"]],
                "widgets_values": ["epi_noiseoffset2.safetensors", 1.0, 1.0]
            },
            {
                "id": 6,
                "type": "VAELoader",
                "widgets_values": ["vae-ft-mse-840000-ema-pruned.safetensors"]
            },
            {
                "id": 7,
                "type": "KSampler",
                "inputs": [],
                "widgets_values": [42, "euler", "normal", "positive", 20, 7.0]
            }
        ]
    }"#;

    const OBJECT_INPUTS_WORKFLOW: &str = r#"{
        "nodes": [
            {
                "id": 1,
                "type": "CheckpointLoaderSimple",
                "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" },
                "widgets_values": []
            },
            {
                "id": 2,
                "type": "ControlNetLoader",
                "inputs": { "control_net_name": "control_v11p_sd15_openpose.pth" },
                "widgets_values": []
            }
        ]
    }"#;

    #[test]
    fn test_parse_simple_workflow() {
        let f = write_workflow(SIMPLE_WORKFLOW);
        let result = parse_workflow(f.path()).unwrap();

        assert_eq!(result.refs.len(), 3);

        let checkpoint = result.refs.iter().find(|r| r.ref_type == "checkpoint").unwrap();
        assert_eq!(checkpoint.model_name, "v1-5-pruned-emaonly.safetensors");

        let lora = result.refs.iter().find(|r| r.ref_type == "lora").unwrap();
        assert_eq!(lora.model_name, "epi_noiseoffset2.safetensors");

        let vae = result.refs.iter().find(|r| r.ref_type == "vae").unwrap();
        assert_eq!(vae.model_name, "vae-ft-mse-840000-ema-pruned.safetensors");
    }

    #[test]
    fn test_parse_object_inputs() {
        let f = write_workflow(OBJECT_INPUTS_WORKFLOW);
        let result = parse_workflow(f.path()).unwrap();

        assert_eq!(result.refs.len(), 2);

        let checkpoint = result.refs.iter().find(|r| r.ref_type == "checkpoint").unwrap();
        assert_eq!(checkpoint.model_name, "sd_xl_base_1.0.safetensors");

        let controlnet = result.refs.iter().find(|r| r.ref_type == "controlnet").unwrap();
        assert_eq!(controlnet.model_name, "control_v11p_sd15_openpose.pth");
    }

    #[test]
    fn test_unknown_nodes_excluded() {
        let workflow = r#"{
            "nodes": [
                {
                    "type": "KSampler",
                    "inputs": [],
                    "widgets_values": [42, "euler"]
                },
                {
                    "type": "CLIPTextEncode",
                    "inputs": { "text": "a beautiful landscape" },
                    "widgets_values": []
                }
            ]
        }"#;
        let f = write_workflow(workflow);
        let result = parse_workflow(f.path()).unwrap();

        // Unknown node types should not generate refs
        assert_eq!(result.refs.len(), 0);
    }

    #[test]
    fn test_normalize_model_name() {
        assert_eq!(
            normalize_model_name("models\\loras\\my_lora.safetensors"),
            "models/loras/my_lora.safetensors"
        );
        assert_eq!(normalize_model_name("  checkpoint.safetensors  "), "checkpoint.safetensors");
    }

    #[test]
    fn test_looks_like_model_filename() {
        assert!(looks_like_model_filename("model.safetensors"));
        assert!(looks_like_model_filename("model.ckpt"));
        assert!(looks_like_model_filename("model.gguf"));
        assert!(!looks_like_model_filename("some text"));
        assert!(!looks_like_model_filename("42"));
        assert!(!looks_like_model_filename("euler"));
    }

    #[test]
    fn test_find_workflow_files() {
        let tmp = TempDir::new().unwrap();
        let wf_dir = tmp.path().join("workflows");
        fs::create_dir_all(&wf_dir).unwrap();

        // Create some JSON files
        fs::write(wf_dir.join("workflow1.json"), "{}").unwrap();
        fs::write(wf_dir.join("workflow2.json"), "{}").unwrap();
        fs::write(wf_dir.join("not_a_workflow.txt"), "text").unwrap();

        let found = find_workflow_files(&wf_dir).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.extension().unwrap() == "json"));
    }

    #[test]
    fn test_index_workflow_with_db() {
        use crate::db::Database;
        use tempfile::NamedTempFile;

        let db_file = NamedTempFile::new().unwrap();
        let db = Database::open(db_file.path()).unwrap();

        let workflow_file = write_workflow(SIMPLE_WORKFLOW);
        let lookup = HashMap::new(); // Empty lookup → all unresolved

        let (wf_id, resolved, unresolved) =
            index_workflow(&db, workflow_file.path(), &lookup).unwrap();

        assert!(wf_id > 0);
        assert_eq!(resolved, 0);
        assert_eq!(unresolved, 3); // 3 model refs, none resolved

        let wf = db.get_workflow(&workflow_file.path().to_string_lossy()).unwrap().unwrap();
        assert_eq!(wf.ref_count, 3);

        let refs = db.get_refs_for_workflow(&workflow_file.path().to_string_lossy()).unwrap();
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_build_model_lookup() {
        use crate::db::Database;
        use tempfile::NamedTempFile;

        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        // Insert a model with a name
        let hash = Blake3Hash::from_hex("a".repeat(64).as_str()).unwrap();
        // insert_or_update_model(hash, size, format, arch, category, base_model)
        // format = file extension/type like "safetensors"
        // To index by filename, we store it in format field
        db.insert_or_update_model(&hash, 1234, Some("v1-5-pruned.safetensors"), None, None, None)
            .unwrap();

        let lookup = build_model_lookup(&db).unwrap();
        assert!(lookup.contains_key("v1-5-pruned.safetensors"));
    }

    #[test]
    fn test_orphan_models() {
        use crate::db::Database;
        use tempfile::NamedTempFile;

        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        // Insert two models — format field stores the filename for lookup
        let hash1 = Blake3Hash::from_hex(&"a".repeat(64)).unwrap();
        let hash2 = Blake3Hash::from_hex(&"b".repeat(64)).unwrap();
        db.insert_or_update_model(&hash1, 100, Some("model1.safetensors"), None, None, None)
            .unwrap();
        db.insert_or_update_model(&hash2, 200, Some("model2.safetensors"), None, None, None)
            .unwrap();

        // Index a workflow that references only model1
        let workflow_file = write_workflow(
            r#"{"nodes":[{
            "type":"CheckpointLoaderSimple",
            "inputs":{"ckpt_name":"model1.safetensors"},
            "widgets_values":[]
        }]}"#,
        );

        let mut lookup = HashMap::new();
        lookup.insert("model1.safetensors".to_string(), hash1.clone());
        index_workflow(&db, workflow_file.path(), &lookup).unwrap();

        // model2 should be an orphan
        let orphans = db.orphan_models().unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].blake3_hash.as_hex(), hash2.as_hex());
    }
}
