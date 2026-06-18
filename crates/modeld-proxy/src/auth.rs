//! Authentication & IP access control middleware for the proxy.
//!
//! - Bearer token check via the `Authorization: Bearer <token>` header
//! - IP allow/deny lists (prefix match; CIDR support is best-effort)
//!
//! Step 3 fills in the real logic; these are the public signatures used by
//! `server.rs`.

use crate::config::{AuthConfig, NetworkConfig};

/// Result of an access decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    Allow,
    DenyMissingToken,
    DenyInvalidToken,
    DenyIp,
}

/// Check a Bearer token against the configured token set.
///
/// Returns `true` when access is allowed:
/// - `require_token == false` → always allowed
/// - otherwise the provided token must match one of the configured tokens
pub fn check_auth(provided_token: Option<&str>, cfg: &AuthConfig) -> bool {
    if !cfg.require_token {
        return true;
    }
    match provided_token {
        Some(t) => cfg.tokens.iter().any(|valid| valid == t),
        None => false,
    }
}

/// Decide whether a peer IP is permitted by the network rules.
///
/// Order: deny-list wins over allow-list. If an allow-list is configured, only
/// listed IPs (by prefix match) are permitted. An empty allow-list means
/// "allow all (that aren't denied)".
pub fn check_ip(peer_ip: &str, cfg: &NetworkConfig) -> bool {
    if cfg.denied_ips.iter().any(|d| ip_matches(peer_ip, d)) {
        return false;
    }
    if cfg.allowed_ips.is_empty() {
        return true;
    }
    cfg.allowed_ips.iter().any(|a| ip_matches(peer_ip, a))
}

/// Best-effort IP/CIDR-prefix match.
///
/// Matches exact IPs (`192.168.1.5`) and CIDR prefixes (`192.168.1.0/24`) by
/// comparing the dotted-decimal prefix before the `/` with the peer's address
/// up to the same number of dotted components. Does not handle IPv6 subnets
/// precisely (prefix string match only).
fn ip_matches(ip: &str, rule: &str) -> bool {
    if ip == rule {
        return true;
    }
    // CIDR: split rule into base + mask length
    let (base, mask) = match rule.split_once('/') {
        Some((b, m)) => (b, m.parse::<u32>().ok()),
        None => (rule, None),
    };
    let peer_parts: Vec<&str> = ip.split('.').collect();
    let base_parts: Vec<&str> = base.split('.').collect();

    // IPv4 dotted prefix comparison
    if peer_parts.len() == 4 && base_parts.len() == 4 {
        if let Some(bits) = mask {
            // number of full octets covered by the mask
            let full_octets = (bits / 8) as usize;
            if full_octets > 4 {
                return false;
            }
            // compare full octets exactly
            for i in 0..full_octets {
                if peer_parts[i] != base_parts[i] {
                    return false;
                }
            }
            // partial octet
            let rem = bits % 8;
            if rem > 0 && full_octets < 4 {
                let p = peer_parts[full_octets].parse::<u32>().unwrap_or(0);
                let b = base_parts[full_octets].parse::<u32>().unwrap_or(0);
                let mask_byte = 0xFFu32 << (8 - rem) & 0xFF;
                return (p & mask_byte) == (b & mask_byte);
            }
            return true;
        }
        // no mask: compare all four octets (already handled by == above)
        return ip == base;
    }
    // Fallback: substring match (IPv6 / hostname-ish)
    ip.starts_with(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(require: bool, tokens: &[&str]) -> AuthConfig {
        AuthConfig {
            require_token: require,
            tokens: tokens.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn net(allow: &[&str], deny: &[&str]) -> NetworkConfig {
        NetworkConfig {
            allow_anonymous: allow.is_empty(),
            allowed_ips: allow.iter().map(|s| s.to_string()).collect(),
            denied_ips: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_auth_not_required_allows_all() {
        let cfg = auth(false, &[]);
        assert!(check_auth(None, &cfg));
        assert!(check_auth(Some("anything"), &cfg));
    }

    #[test]
    fn test_auth_required_missing_denied() {
        let cfg = auth(true, &["secret"]);
        assert!(!check_auth(None, &cfg));
    }

    #[test]
    fn test_auth_required_valid_token() {
        let cfg = auth(true, &["secret", "other"]);
        assert!(check_auth(Some("secret"), &cfg));
        assert!(check_auth(Some("other"), &cfg));
    }

    #[test]
    fn test_auth_required_invalid_token() {
        let cfg = auth(true, &["secret"]);
        assert!(!check_auth(Some("wrong"), &cfg));
    }

    #[test]
    fn test_ip_empty_lists_allow_all() {
        let cfg = net(&[], &[]);
        assert!(check_ip("192.168.1.5", &cfg));
        assert!(check_ip("10.0.0.1", &cfg));
    }

    #[test]
    fn test_ip_deny_overrides_allow() {
        let cfg = net(&["192.168.1.0/24"], &["192.168.1.99"]);
        assert!(!check_ip("192.168.1.99", &cfg));
        assert!(check_ip("192.168.1.5", &cfg));
    }

    #[test]
    fn test_ip_allow_list_cidr() {
        let cfg = net(&["192.168.1.0/24"], &[]);
        assert!(check_ip("192.168.1.5", &cfg));
        assert!(check_ip("192.168.1.254", &cfg));
        assert!(!check_ip("192.168.2.5", &cfg));
        assert!(!check_ip("10.0.0.1", &cfg));
    }

    #[test]
    fn test_ip_cidr_partial_octet() {
        // /20 covers 255.255.240.0 → 192.168.16.x..31.x
        let cfg = net(&["192.168.16.0/20"], &[]);
        assert!(check_ip("192.168.31.5", &cfg));
        assert!(!check_ip("192.168.32.5", &cfg));
    }

    #[test]
    fn test_ip_exact_match() {
        let cfg = net(&["10.0.0.1"], &[]);
        assert!(check_ip("10.0.0.1", &cfg));
        assert!(!check_ip("10.0.0.2", &cfg));
    }
}
