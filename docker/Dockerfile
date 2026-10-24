# syntax=docker/dockerfile:1
#
# Multi-stage build: the toolchain image is ~1.4 GB, the runtime image only
# carries the stripped binary and ca-certificates (~80 MB).
#
# The Rust version must satisfy `rust-version` in Cargo.toml (1.88, verified with
# `cargo +1.88 build --workspace --locked`).

FROM rust:1.88-slim AS builder
WORKDIR /app

# Native build dependencies (rusqlite bundles SQLite itself; TLS is via rustls
# or vendored openssl-free crates, so no libssl-dev is required).
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config \
 && rm -rf /var/lib/apt/lists/*

# Layer the manifests first so a source-only change does not re-download the
# whole dependency graph.
COPY Cargo.toml Cargo.lock ./
COPY crates/modeld-core/Cargo.toml crates/modeld-core/
COPY crates/modeld-cli/Cargo.toml crates/modeld-cli/
COPY crates/modeld-proxy/Cargo.toml crates/modeld-proxy/
COPY crates/modeld-client/Cargo.toml crates/modeld-client/
COPY crates/modeld-webui/Cargo.toml crates/modeld-webui/

COPY crates ./crates
RUN cargo build --release --locked -p modeld-cli \
 && strip target/release/modeld

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home --home-dir /home/modeld modeld \
 && mkdir -p /data \
 && chown modeld:modeld /data

COPY --from=builder /app/target/release/modeld /usr/local/bin/modeld

USER modeld
WORKDIR /data
ENV MODELD_STORE=/data
VOLUME ["/data"]
EXPOSE 8234

# `--allow-lan` is required for a non-loopback bind; the token comes from
# `MODELD_AUTH_TOKEN` (never baked into the image or written to the store).
ENTRYPOINT ["modeld"]
CMD ["serve", "--store", "/data", "--host", "0.0.0.0", "--port", "8234", "--allow-lan"]
