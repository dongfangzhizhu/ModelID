//! OS keyring token storage (C4).
//!
//! 目标：`modeld.toml` 不再明文保存 bearer token。keyring 中每个 store 用
//! 独立的账户名（store 路径的 SHA256 前 16 字节 hex），避免多个 store 串号。
//!
//! **不静默降级**：keyring 不可用（无头 Linux、无 DBus、无 Credential
//! Manager）时，所有 API 返回明确的错误，由调用方决定是否回退到环境变量
//! 或报错退出；本模块不会偷偷把 token 写回配置文件。
//!
//! 迁移：`migrate_from_config()` 从现有 `modeld.toml` 读取明文 token，写入
//! keyring，并返回是否成功。调用方在成功后必须清掉配置里的 token 字段。

use crate::config::{load_config, save_config, ModeldConfig};
use anyhow::{Context, Result};
use keyring::Entry;
use sha2::{Digest, Sha256};
use std::path::Path;

const SERVICE: &str = "modeld";

fn account_for(store_path: &Path) -> String {
    // 规范化为绝对路径字符串再哈希，避免相对路径/符号链接造成同一 store
    // 多个 keyring 账户。
    let canonical = std::fs::canonicalize(store_path).unwrap_or_else(|_| store_path.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    format!("store-{}", hex(&digest[..8]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// keyring 中是否已存在该 store 的 token。
pub fn has_token(store_path: &Path) -> bool {
    match Entry::new(SERVICE, &account_for(store_path)) {
        Ok(entry) => entry.get_password().is_ok(),
        Err(_) => false,
    }
}

/// 从 OS keyring 读取 token。keyring 不可用或未设置时返回 Err。
pub fn get_token(store_path: &Path) -> Result<String> {
    let entry = Entry::new(SERVICE, &account_for(store_path))
        .context("open OS keyring (no credential manager available)")?;
    entry.get_password().context("no token stored in OS keyring for this store")
}

/// 把 token 写入 OS keyring。成功不会写任何明文文件。
pub fn set_token(store_path: &Path, token: &str) -> Result<()> {
    if token.is_empty() {
        anyhow::bail!("refusing to store an empty token");
    }
    let entry = Entry::new(SERVICE, &account_for(store_path))
        .context("open OS keyring (no credential manager available)")?;
    entry.set_password(token).context("write token to OS keyring")?;
    Ok(())
}

/// 从 keyring 删除该 store 的 token。未找到时不报错（幂等）。
pub fn delete_token(store_path: &Path) -> Result<()> {
    let entry = Entry::new(SERVICE, &account_for(store_path))
        .context("open OS keyring (no credential manager available)")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("delete token from OS keyring"),
    }
}

/// 把 `modeld.toml` 里的明文 token 迁移到 keyring，并清空配置中的 token
/// 字段后写回。返回是否确实迁移了 token。
///
/// 若 keyring 写入失败，配置文件保持不变（不丢 token），错误上抛。
pub fn migrate_from_config(store_path: &Path) -> Result<bool> {
    let mut config: ModeldConfig = load_config(store_path).unwrap_or_default();
    let tokens = config.proxy_tokens();
    let token = match tokens.into_iter().next() {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(false),
    };
    set_token(store_path, &token)?;
    // 清空两套 schema 中的明文 token，但保留 require_token=true。
    config.auth.token.clear();
    config.proxy.auth.tokens.clear();
    save_config(store_path, &config)?;
    Ok(true)
}

/// 解析 token：keyring 优先，其次环境变量；两者都没有时返回明确错误。
/// 不再从 `modeld.toml` 读取明文 token 作为静默回退。
pub fn resolve_token(store_path: &Path) -> Result<String> {
    if let Ok(token) = get_token(store_path) {
        if !token.is_empty() {
            return Ok(token);
        }
    }
    if let Ok(token) = std::env::var("MODELD_AUTH_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }
    anyhow::bail!(
        "no bearer token available: set one with `modeld config token-set` \
         (OS keyring) or MODELD_AUTH_TOKEN"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn account_is_stable_and_store_scoped() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let a2 = a.clone();
        assert_eq!(account_for(&a), account_for(&a2));
        assert_ne!(account_for(&a), account_for(&b));
    }

    #[test]
    fn keyring_absence_is_explicit() {
        let tmp = TempDir::new().unwrap();
        // 在没有 keyring 的环境里必须返回 Err，而不是静默写配置文件。
        let before = load_config(tmp.path()).map(|c| c.proxy_tokens()).unwrap_or_default();
        let _ = resolve_token(tmp.path());
        let after = load_config(tmp.path()).map(|c| c.proxy_tokens()).unwrap_or_default();
        assert_eq!(before, after, "resolve_token must not modify on-disk config");
    }
}
