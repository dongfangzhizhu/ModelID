# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| `0.1.x` (main / unreleased) | Yes — security fixes land on `main` and are backported only if a release branch exists |
| older planning snapshots under `docs/history/` | No |

The workspace is currently marked `publish = false`. Treat the repository as a
pre-1.0 project: update before exposing it to untrusted networks.

## Reporting a vulnerability

Please **do not** open a public issue for a security report.

- Preferred: use GitHub's private vulnerability report for this repository:
  <https://github.com/dongfangzhizhu/ModelID/security/advisories/new>
- Include: affected version/commit, component (`core`, `cli`, `proxy`, `webui`,
  `client`, Python hook), reproduction steps, impact, and any suggested fix.
- You should receive an acknowledgement within 7 days. We will coordinate a fix
  and disclosure timeline before publishing details.

## Secure defaults (as built)

These are current code behaviours, not aspirations:

- `modeld proxy start` binds `127.0.0.1:8234` and requires a Bearer token. On
  first start the CLI generates a UUIDv4 token, persists it to
  `<store>/modeld.toml`, and prints it once. Starting via the library with
  default config and no token is refused.
- A non-loopback bind requires explicit `--allow-lan`; `--allow-anonymous` is
  accepted only on loopback. Both checks run before the socket is bound.
- Proxy routes are gated by IP rules first (deny wins → `403`), then the
  anonymous check (`401`), then constant-time token comparison (`401`). Only
  `GET /health` is public.
- `modeld-webui` uses an origin allowlist for CORS and requires
  `X-Modeld-CSRF` on browser-originated `POST/PUT/PATCH/DELETE`. Requests with
  no `Origin` (curl, CLI) are intentionally outside the CSRF guard.
- Tokens are never logged. The CLI prints the generated/rotated token to stdout
  so an operator can capture it; treat that terminal output as a secret.
- Config writes use `toml_edit` and rewrite only the section that owns a key,
  preserving comments and unknown tables.

## Hardening checklist

1. Keep the proxy/WebUI on loopback unless LAN access is required. If it is,
   put TLS in front and restrict access at the network layer.
2. Rotate with `modeld proxy token-rotate` after exposing a token, and restart
   the process — hot reload is not implemented.
3. Set `[proxy.network] denied_ips` / `allowed_ips` as a second layer. Only IPv4
   CIDR matching is exact; do not rely on IPv6 or hostname rules as a boundary.
4. Do not pass HF tokens on the command line on shared machines; prefer
   `HF_TOKEN` or the store token file.
5. Restrict access to `GET /metrics` on the WebUI port — it is unauthenticated
   by design. The proxy service does not expose `/metrics`.
6. Back up `<store>/modeld.db` before schema migrations; migrations create a
   `VACUUM INTO` backup under `<store>/backups/` automatically.
7. Treat `<store>/modeld.toml`, `hf_token`, and shell history as sensitive.

## Known limitations

- **Plain HTTP only**: no TLS, HTTP/2, or WebSocket support on the proxy.
- **Client integrity**: `modeld-client` verifies transfer length but does not
  recompute BLAKE3. Callers that need end-to-end integrity should hash the
  downloaded file and compare with `/v1/models`.
- **Token in URL**: proxy accepts Bearer headers only; `?token=` is a WebUI
  WebSocket handshake exception and must not be used for APIs.
- **IP matching**: IPv6/hostname entries fall back to prefix string matching.
- **Store lock**: OS locks are per store; NFS/SMB semantics are untested.
- **Anonymous HF mirror**: default proxy auth means HF clients must send the
  modeld Bearer token; otherwise they get `401`. Loopback `--allow-anonymous`
  is a local-trust exception, not a shared-machine policy.
- **Audit log**: `<store>/audit.log` grows without rotation; planned for C2.

## Security-relevant design records

- [ADR-0001 Storage layout](docs/adr/0001-storage-layout.md)
- [ADR-0002 Transaction and recovery](docs/adr/0002-transaction-and-recovery.md)
- [ADR-0003 Authentication defaults](docs/adr/0003-authentication-defaults.md)
- [ADR-0005 HuggingFace compatibility](docs/adr/0005-huggingface-compatibility.md)
