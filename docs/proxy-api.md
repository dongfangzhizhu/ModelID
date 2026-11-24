# modeld Proxy HTTP API

The modeld proxy (`modeld proxy start`) exposes a small HTTP API so multiple
machines on a LAN can share one CAS store. It also offers a HuggingFace-compatible
resolve route: point `HF_ENDPOINT` at `<base>/v1/hf-proxy` and HF-based frameworks
can download through modeld.

**Base URL**: `http://<host>:8234` by default (`127.0.0.1:8234`).

> modeld itself does not read `HF_ENDPOINT`; it is a client-side variable used by
> `huggingface_hub`. The proxy's upstream HuggingFace base URL is controlled by
> `MODELD_HF_BASE` (default `https://huggingface.co`).

## Authentication

**Secure by default.** The CLI binds `127.0.0.1` and requires a bearer token;
an unauthenticated request gets `401` out of the box.

- `/health` is always public. It is the **only** public proxy route.
- All other routes pass through IP and token middleware:
  - **IP gate first**: a denied IP gets `403`; an empty allow list means "allow
    every IP not explicitly denied". Only IPv4 CIDR matching is exact — IPv6
    and hostname entries fall back to prefix string matching and are not a
    security boundary.
  - **Anonymous check**: if anonymous access is disabled and no token was
    supplied, the response is `401 unauthorized: anonymous access disabled`.
  - **Bearer token**: `Authorization: Bearer <token>`. Comparison checks every
    configured token without short-circuiting and compares equal-length tokens
    byte-by-byte in constant time; a different length is rejected immediately.

### Where the token comes from

`modeld proxy start` resolves the token in this order:

1. `--token <token>` on the command line
2. `[proxy.auth] tokens` in the file passed with `--config`
3. `<store>/modeld.toml`: `[proxy.auth] tokens` (first entry), falling back to
   the legacy `[auth] token`
4. the `MODELD_AUTH_TOKEN` environment variable (non-empty)
5. otherwise a random UUIDv4 token is generated, saved to **both**
   `[auth].token` and `[proxy.auth].tokens` in `<store>/modeld.toml`, and
   printed once at startup

Use `--print-token` to print the token even when it was already configured.
`modeld proxy token-rotate` rewrites the same two fields but requires a restart;
hot reload is not implemented.

`ProxyConfig::default()` starts closed with an empty `tokens` list. The CLI
generates a token before starting; a library caller that supplies no token is
refused by `check_startup_invariants()` rather than starting an open server.

### Exposing the proxy on the network

| Flags | Bind | Auth |
|---|---|---|
| *(none)* | `127.0.0.1` | bearer token (auto-generated) |
| `--allow-anonymous` | `127.0.0.1` only | none — loopback only |
| `--bind <lan-ip> --allow-lan` | any interface | bearer token (mandatory) |

Binding to a non-loopback address without `--allow-lan` is refused with exit
code 1, as is `--allow-anonymous` on a non-loopback bind. The server validates
these invariants *before* binding the port. `--bind` has a CLI default of
`127.0.0.1` and overrides `bind_address` from `--config`, so a config-file bind
address requires an explicit matching `--bind` on the command line.

> Anonymous access on a loopback bind is a deliberate local-trust mode; it is
> not a substitute for a token on a shared machine.

## Error codes

| Status | When |
|---|---|
| `200` | `/health`, `/v1/models`, full blob/HF response |
| `206` | successful single-range response |
| `400` | invalid BLAKE3 hash in `/v1/blobs/{hash}` |
| `401` | token disabled for anonymous request, or invalid/missing token |
| `403` | IP denied by `[proxy.network]` |
| `404` | blob absent, or unknown route |
| `405` | method not allowed on a known route |
| `502` | upstream HuggingFace failure on the HF proxy route |

## Endpoints

### `GET /health`

Service health. Always public.

**Response** `200`:

```json
{
  "status": "ok",
  "version": "0.1.0",
  "uptime_seconds": 3600,
  "model_count": 42,
  "total_bytes": 5000000000000
}
```

`version` is the crate version at build time (`CARGO_PKG_VERSION`); the value
above is the current workspace version.

### `GET /v1/models`

List all models in the CAS store.

**Response** `200`:

```json
{
  "models": [
    {
      "hash": "abcd1234...64-hex-blake3",
      "size_bytes": 4470000000,
      "format": "safetensors",
      "category": "checkpoint"
    }
  ],
  "total": 1,
  "total_size_bytes": 4470000000
}
```

`format` and `category` are optional model fields and may be `null`.

### `GET /v1/blobs/{blake3_hash}`

Download a CAS object by its 64-char BLAKE3 hash. Supports one HTTP Range.

**Request headers**:
- `Range: bytes=0-1023` (optional) — request a byte range
- `Authorization: Bearer <token>` (if required)

**Response** (full): `200`, `Content-Type: application/octet-stream`,
`Content-Length: <size>`, `Accept-Ranges: bytes`.

**Response** (range): `206 Partial Content`,
`Content-Range: bytes 0-1023/<size>`, `Content-Length: 1024`.

A single range response is capped at **64 MiB** (`[proxy] range_chunk_bytes`).
A client that asks for `bytes=0-` on a 20 GB object receives
`Content-Range: bytes 0-67108863/21474836480` and must re-request the next
window. The caller is responsible for looping; the built-in Rust SDK's
`download_blob()` performs one request (plus resume) per call and does not
fetch subsequent windows automatically.

Multiple ranges in one header are not supported — only the first is used. An
unsatisfiable range currently returns `200` with the full object rather than
`416`. Errors: `400` (invalid hash), `404` (blob absent).

## Concurrency and resource use

- **Streaming**: responses are read straight from the CAS file into the socket.
  Server memory does not depend on object size (a 512 MiB download grows RSS by
  less than 1 MB). See `scripts/bench-proxy-streaming.ps1`.
- **Worker pool**: requests are served by a fixed pool of one thread per core,
  clamped to `[2, 16]`. A slow or stalled client occupies one worker, not the
  whole server. A handler panic is caught and the worker keeps serving.
- **Panic isolation**: a handler panic is caught, logged, and the worker keeps
  serving. A poisoned DB mutex is recovered rather than aborting every later
  request.
- **HF cache miss**: the upstream download runs on its own SQLite connection, so
  `/health` and `/v1/models` stay responsive while a multi-GB model is being
  fetched.
- **Metrics**: the proxy does not expose `/metrics`. The separate WebUI service
  has an unauthenticated `GET /metrics`; restrict it at the network layer.

### `GET /v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}`

HuggingFace-compatible proxy. The path mirrors the HF Hub resolve URL, so
setting `HF_ENDPOINT=http://<host>:8234/v1/hf-proxy` makes this work with no
client code changes — but the client must also send the modeld Bearer token on
each request (or use loopback `--allow-anonymous`). The `/v1/hf-proxy/` prefix
is required; pointing `HF_ENDPOINT` at the bare base URL does not match any
route. See `docs/proxy-setup.md`.

**Flow**:
1. Check the fake HF cache for `{org}/{repo}@{revision}/{file}`.
2. **Cache hit** → stream the cached file with `X-Modeld-Cache: hit`.
3. **Cache miss** → download from `MODELD_HF_BASE` (default
   `https://huggingface.co`), store in CAS, build the fake HF cache entry, then
   stream with `X-Modeld-Cache: miss`.

**Response headers**:
- `X-Modeld-Cache: hit | miss`
- `X-Modeld-Blake3: <blake3>` (miss only; the freshly computed hash)
- `Content-Type: application/octet-stream`

**Errors**: `502` (upstream HuggingFace failure). Path traversal segments are
rejected before the cache is consulted.

## Configuration (`modeld.toml [proxy]`)

```toml
[proxy]
port = 8234
bind_address = "127.0.0.1"
store_path = ".modeld"
range_chunk_bytes = 67108864

[proxy.auth]
require_token = true
tokens = ["your-secret-token"]

[proxy.network]
allow_anonymous = false
allowed_ips = ["192.168.1.0/24"]
denied_ips = []
```

A missing file or `[proxy]` table gives the safe defaults: loopback bind,
token required, anonymous off, empty allow/deny lists, 64 MiB range windows,
port 8234. The CLI's `--bind` default overrides a configured `bind_address`;
pass `--bind` explicitly when using a non-loopback config value. An IP in
`denied_ips` wins over `allowed_ips`. `require_token = true` with an empty
`tokens` list is a configuration error for library callers — the CLI generates
a token first.

## Not supported (documented limits)

- **HTTPS/TLS**: plain HTTP only (designed for trusted LANs).
- **Multi-range responses**: a `Range` header with multiple ranges serves only
  the first range (no `multipart/byteranges`).
- **HTTP/2, WebSocket**: not implemented on the proxy. The WebUI has its own
  `/ws` endpoint.
- **Client-side BLAKE3 verification**: `modeld-client` checks content length but
  does not rehash downloads; callers needing end-to-end integrity should hash
  locally and compare with `/v1/models`.
