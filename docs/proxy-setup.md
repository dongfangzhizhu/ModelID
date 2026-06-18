# modeld Proxy Setup Guide

The proxy lets one machine download models once and serve them to every other
machine on the LAN at full LAN bandwidth, with transparent HuggingFace
deduplication.

## 1. Start the proxy (on the machine holding the CAS store)

```bash
# Default: port 8234, open access on the LAN
modeld proxy start

# Bind a specific port / store
modeld proxy start --port 8234 --bind 0.0.0.0 --store /data/modeld

# Require a bearer token
modeld proxy start --token your-secret-token

# From a config file
modeld proxy start --config modeld.toml
```

The server runs in the foreground; press `Ctrl+C` to stop.

### Configuration file

Place a `modeld.toml` next to the store (or pass `--config`):

```toml
[proxy]
port = 8234
bind_address = "0.0.0.0"
store_path = ".modeld"

[proxy.auth]
require_token = false
tokens = []

[proxy.network]
allow_anonymous = true
allowed_ips = ["192.168.1.0/24"]
denied_ips = []
```

See [proxy-api.md](./proxy-api.md) for the full HTTP API.

## 2. Use it as a HuggingFace mirror (zero client changes)

On each client machine, point `HF_ENDPOINT` at the proxy so that
diffusers / transformers / ComfyUI / Forge / A1111 all download through it:

```bash
# Linux/macOS
export HF_ENDPOINT="http://192.168.1.5:8234/v1/hf-proxy"

# Windows PowerShell
$env:HF_ENDPOINT = "http://192.168.1.5:8234/v1/hf-proxy"
```

Now a normal HF download is transparently routed through modeld:
- first download → pulled from HuggingFace, stored in CAS, returned
- every subsequent request (from any LAN machine) → served from the local CAS

## 3. Use the Rust client SDK

For programmatic access from Rust:

```rust
use modeld_client::ModeldClient;

let client = ModeldClient::new("http://192.168.1.5:8234")
    .with_token("your-secret-token"); // only if the server requires one

let health = client.health()?;
println!("{} models, {} bytes", health.model_count, health.total_bytes);

// List models
for m in client.list_models()? {
    println!("{}  {} bytes", &m.hash[..16], m.size_bytes);
}

// Download a blob by BLAKE3 hash (resumes partial files)
client.download_blob("abcd1234...64-hex", "model.safetensors", None)?;
```

## 4. Discover servers on the LAN (mDNS)

If servers publish themselves via mDNS (Avahi/Bonjour):

```bash
modeld proxy discover --timeout 5
```

To publish a server, register it with the host mDNS daemon. Example on
Avahi-based Linux:

```bash
avahi-publish -s modeld _modeld._tcp 8234 version=0.1.0 models=42
```

> **Note**: the `mdns` crate modeld uses is *discovery-only*. Publishing must
> be done by the host's mDNS daemon (Avahi on Linux, Bonjour/dns-sd on macOS,
> the mDNS Responder service on Windows). `modeld proxy discover` will find
> any service registered as `_modeld._tcp.local.`.

## 5. Check a running server's status

```bash
modeld proxy status --url http://localhost:8234
```

## Security notes

- The proxy is **plain HTTP** — intended for trusted LANs, not the public
  internet. Do not expose port 8234 to untrusted networks without a reverse
  proxy providing TLS.
- For sensitive environments, enable `require_token` and restrict
  `allowed_ips` to your subnet.
