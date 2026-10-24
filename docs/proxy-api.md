# modeld Proxy HTTP API

The modeld proxy (`modeld proxy start`) exposes a small HTTP API so multiple
machines on a LAN can share one CAS store. It is also HuggingFace-compatible:
point `HF_ENDPOINT` at the proxy and every HF-based framework downloads
transparently through modeld.

**Base URL**: `http://<host>:8234` (default port).

## Authentication

**Secure by default.** The proxy binds `127.0.0.1` and requires a bearer token;
an unauthenticated request gets `401` out of the box.

- `/health` is always public.
- All other routes pass through auth + IP middleware:
  - **Bearer token**: send `Authorization: Bearer <token>`.
  - **IP allow/deny lists**: configured in `modeld.toml [proxy.network]`.

When a token is required and absent/invalid, routes return `401`.
When an IP is denied, routes return `403`.

### Where the token comes from

`modeld proxy start` resolves the token in this order:

1. `--token <token>` on the command line
2. `[proxy.auth] tokens` in the file passed with `--config`
3. `[auth] token` in `<store>/modeld.toml` (written by `modeld proxy token-rotate`)
4. the `MODELD_AUTH_TOKEN` environment variable
5. otherwise a random UUIDv4 token is generated, saved to
   `<store>/modeld.toml [auth] token`, and printed once at startup

Use `--print-token` to print the token even when it was already configured.
Token comparison is constant-time.

### Exposing the proxy on the network

| Flags | Bind | Auth |
|---|---|---|
| *(none)* | `127.0.0.1` | bearer token (auto-generated) |
| `--allow-anonymous` | `127.0.0.1` only | none — loopback only |
| `--bind <lan-ip> --allow-lan` | any interface | bearer token (mandatory) |

Binding to a non-loopback address without `--allow-lan` is refused with exit
code 1, as is `--allow-anonymous` on a non-loopback bind. The server validates
these invariants *before* binding the port, so a misconfiguration can never
expose the store even briefly.

> Anonymous access on a loopback bind is a deliberate local-trust mode; it is
> not a substitute for a token on a shared machine.

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

### `GET /v1/blobs/{blake3_hash}`

Download a CAS object by its 64-char BLAKE3 hash. Supports HTTP Range.

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
window. This keeps memory flat and makes downloads resumable; it is the same
contract any chunked range server offers.

**Errors**: `400` (invalid hash), `404` (blob absent).

## Concurrency and resource use

- **Streaming**: responses are read straight from the CAS file into the socket.
  Server memory does not depend on object size (a 512 MiB download grows RSS by
  less than 1 MB). See `scripts/bench-proxy-streaming.ps1`.
- **Worker pool**: requests are served by a fixed pool (one thread per core, at
  most 16). A slow or stalled client occupies one worker, not the whole server.
- **Panic isolation**: a handler panic is caught, logged, and the worker keeps
  serving. A poisoned DB mutex is recovered rather than aborting every later
  request.
- **HF cache miss**: the upstream download runs on its own SQLite connection, so
  `/health` and `/v1/models` stay responsive while a multi-GB model is
  being fetched.

### `GET /v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}`

HuggingFace-compatible proxy. The path mirrors the HF Hub resolve URL, so
setting `HF_ENDPOINT=http://host:8234` makes this work with no client changes.
The `/v1/hf-proxy/` prefix is the canonical route; see `docs/proxy-setup.md`
for the `HF_ENDPOINT` configuration that maps HF requests onto it.

**Flow**:
1. Check the fake HF cache for `{org}/{repo}@{revision}/{file}`.
2. **Cache hit** → stream the cached file with `X-Modeld-Cache: hit`.
3. **Cache miss** → download from HuggingFace, store in CAS, build the fake HF
   cache entry, then stream with `X-Modeld-Cache: miss`.

**Response headers**:
- `X-Modeld-Cache: hit | miss`
- `X-Modeld-Blake3: <blake3>` (on miss; the freshly computed hash)
- `Content-Type: application/octet-stream`

**Errors**: `502` (upstream HuggingFace failure).

## Configuration (`modeld.toml [proxy]`)

```toml
[proxy]
port = 8234
bind_address = "127.0.0.1"
store_path = ".modeld"

[proxy.auth]
require_token = true
tokens = ["your-secret-token"]

[proxy.network]
allow_anonymous = false
allowed_ips = ["192.168.1.0/24"]
denied_ips = []
```

All fields have safe defaults: a missing file or `[proxy]` table starts a
loopback-only server that requires a token. `require_token = true` with an
empty `tokens` list is a configuration error — the server refuses to start
rather than accepting every request.

## Not supported (documented limits)

- **HTTPS/TLS**: plain HTTP only (designed for trusted LANs).
- **Multi-range responses**: a `Range` header with multiple ranges serves only
  the first range (no `multipart/byteranges`).
- **HTTP/2, WebSocket**: not implemented.
