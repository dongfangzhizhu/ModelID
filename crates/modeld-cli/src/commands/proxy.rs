//! `modeld proxy …` — start, discover, inspect and re-key the local proxy.

use crate::{format_bytes, load_config, resolve_store, save_config};
use anyhow::Result;
use clap::Subcommand;
use colored::*;
use modeld_core::{t, tf, Database};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Subcommand)]
pub enum ProxyAction {
    /// Start the proxy server (foreground; Ctrl+C to stop)
    Start {
        /// Bind address (default: 127.0.0.1, loopback only)
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Port (default: 8234)
        #[arg(long)]
        port: Option<u16>,
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
        /// Bearer token (a random one is generated and saved when omitted)
        #[arg(long)]
        token: Option<String>,
        /// Allow anonymous (unauthenticated) access — loopback binds only
        #[arg(long, default_value = "false")]
        allow_anonymous: bool,
        /// Path to a modeld.toml config file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Allow binding to a non-loopback address (bearer token is enforced)
        #[arg(long, default_value = "false")]
        allow_lan: bool,
        /// Print the bearer token at startup
        #[arg(long, default_value = "false")]
        print_token: bool,
    },
    /// Discover modeld proxy servers on the local network (mDNS)
    Discover {
        /// How long to scan, in seconds (default: 5)
        #[arg(short = 't', long, default_value = "5")]
        timeout: u64,
    },
    /// Query a running proxy server's health and model stats
    Status {
        /// Proxy base URL (e.g. http://192.168.1.5:8234)
        #[arg(short = 'u', long, default_value = "http://localhost:8234")]
        url: String,
        /// Bearer token (if the server requires one)
        #[arg(long)]
        token: Option<String>,
    },
    /// Rotate (regenerate) the bearer token stored in modeld.toml
    TokenRotate {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

pub fn proxy_command(action: ProxyAction) -> Result<()> {
    match action {
        ProxyAction::Start {
            bind,
            port,
            store,
            token,
            allow_anonymous,
            config,
            allow_lan,
            print_token,
        } => {
            // `modeld proxy start` is a compatibility alias for the legacy proxy server.
            proxy_start_command(ProxyStartOptions {
                bind,
                port,
                store,
                token,
                allow_anonymous,
                config,
                allow_lan,
                print_token,
            })
        }
        ProxyAction::Discover { timeout } => proxy_discover_command(timeout),
        ProxyAction::Status { url, token } => proxy_status_command(&url, token),
        ProxyAction::TokenRotate { store } => proxy_token_rotate_command(resolve_store(store)),
    }
}

/// Options for `modeld proxy start`, grouped to keep the signature readable.
struct ProxyStartOptions {
    bind: String,
    port: Option<u16>,
    store: PathBuf,
    token: Option<String>,
    allow_anonymous: bool,
    config: Option<PathBuf>,
    allow_lan: bool,
    print_token: bool,
}

fn proxy_start_command(opts: ProxyStartOptions) -> Result<()> {
    use colored::Colorize;
    let ProxyStartOptions {
        bind,
        port,
        store,
        token,
        allow_anonymous,
        config,
        allow_lan,
        print_token,
    } = opts;

    // Load config from file if provided; otherwise build from CLI flags.
    let mut cfg = if let Some(ref cfg_path) = config {
        modeld_proxy::ProxyConfig::load(cfg_path)?
    } else {
        modeld_proxy::ProxyConfig::from_cli(
            port,
            Some(bind.clone()),
            store.clone(),
            token.clone(),
            allow_anonymous,
        )
    };
    // CLI bind override always wins (most explicit).
    cfg.bind_address = bind;
    if let Some(p) = port {
        cfg.port = p;
    }

    let loopback = cfg.is_loopback_bind();

    // Exposing the proxy on the network must be an explicit decision.
    if !loopback && !allow_lan {
        anyhow::bail!(
            "refusing to bind the proxy to '{}': pass --allow-lan to expose it on the \
             network (bearer-token authentication will be enforced)",
            cfg.bind_address
        );
    }
    let wants_anonymous = allow_anonymous || cfg.network.allow_anonymous;
    if !loopback && wants_anonymous {
        anyhow::bail!(
            "anonymous access is only allowed on a loopback bind; '{}' requires a bearer token",
            cfg.bind_address
        );
    }

    if loopback && wants_anonymous {
        cfg.network.allow_anonymous = true;
        cfg.auth.require_token = false;
        cfg.auth.tokens.clear();
        println!(
            "{}",
            "⚠  anonymous access enabled — reachable from this machine only".yellow().bold()
        );
    } else {
        // Resolution order: --token, --config file, <store>/modeld.toml (either
        // schema), MODELD_AUTH_TOKEN, generated.
        let mut store_config = load_config(&store).unwrap_or_default();
        let configured = token
            .or_else(|| cfg.auth.tokens.first().cloned())
            .or_else(|| store_config.proxy_tokens().into_iter().next())
            .or_else(|| std::env::var("MODELD_AUTH_TOKEN").ok().filter(|t| !t.is_empty()));

        let (token_value, generated) = match configured {
            Some(t) => (t, false),
            None => {
                let t = Uuid::new_v4().to_string();
                store_config.set_proxy_token(&t);
                save_config(&store, &store_config)?;
                (t, true)
            }
        };

        cfg.auth.require_token = true;
        cfg.auth.tokens = vec![token_value.clone()];
        cfg.network.allow_anonymous = false;

        if generated || print_token {
            println!("{}", "🔑  Bearer token for this proxy:".cyan().bold());
            println!("    {}", token_value.yellow().bold());
            println!("    {}", format!("Authorization: Bearer {}", token_value).dimmed());
            if generated {
                println!(
                    "{}",
                    "   Generated once and saved to <store>/modeld.toml [auth] token.".dimmed()
                );
            }
            println!();
        }
    }

    // Refuse to start on an unsafe combination rather than leaking the store.
    cfg.check_startup_invariants()?;

    // Initialize the store components if missing so a fresh store works.
    let cas = modeld_core::CasStore::new(&store);
    cas.init()?;
    let db_path = store.join("modeld.db");
    Database::open(&db_path)?;

    let server = modeld_proxy::ProxyServer::new(cfg, store);
    // mDNS announcement is best-effort; it logs the intended registration.
    // (Publication needs the host's mDNS daemon — see discovery.rs docs.)
    server.start()
}

fn proxy_discover_command(timeout: u64) -> Result<()> {
    println!("{}", t("proxy.discover.scanning").cyan().bold());
    println!("  {}", t("proxy.discover.service"));
    println!();

    let servers = modeld_proxy::discover(timeout);

    if servers.is_empty() {
        println!("{}", t("proxy.discover.none").yellow());
        println!();
        println!("{}", t("proxy.discover.make_sure").dimmed());
        println!("  {}", t("proxy.discover.tip_server"));
        println!("  {}", t("proxy.discover.tip_mdns"));
        println!("  {}", t("proxy.discover.tip_subnet"));
        println!("  {}", t("proxy.discover.tip_firewall"));
        return Ok(());
    }

    println!("{} {}", "✓".green().bold(), tf("proxy.discover.found", &[("count", &servers.len())]));
    println!();
    for s in &servers {
        let url = format!("http://{}:{}", s.address, s.port);
        println!("  {} {}", "•".cyan(), tf("proxy.discover.line", &[("url", &url)]).bold());
        if !s.txt.is_empty() {
            let parts: Vec<String> = s.txt.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            println!("      {}", parts.join(", ").dimmed());
        }
    }

    Ok(())
}

fn proxy_status_command(url: &str, token: Option<String>) -> Result<()> {
    let mut builder = modeld_client::ModeldClient::new(url);
    if let Some(t) = token {
        builder = builder.with_token(t);
    }

    let health = builder.health().map_err(|e| {
        let es = format!("{:#}", e);
        anyhow::anyhow!("{}", tf("proxy.status.reach_failed", &[("url", &url), ("error", &es)]))
    })?;

    println!("{}", t("proxy.status.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  {}", tf("proxy.status.url", &[("url", &url)]));
    println!("  {}", tf("proxy.status.status", &[("value", &health.status)]).green());
    println!("  {}", tf("proxy.status.version", &[("value", &health.version)]));
    println!("  {}", tf("proxy.status.uptime", &[("value", &health.uptime_seconds)]));
    println!("  {}", tf("proxy.status.models", &[("count", &health.model_count)]).bold());
    let size = format_bytes(health.total_bytes as u64);
    println!("  {}", tf("proxy.status.total_size", &[("value", &size)]));
    println!("{}", "─".repeat(40).cyan());

    match builder.list_models() {
        Ok(models) => {
            if models.is_empty() {
                println!("{}", t("proxy.status.no_models").dimmed());
            } else {
                println!();
                println!("{}", t("proxy.status.recent_header").cyan());
                for m in models.iter().take(5) {
                    let hash_short = &m.hash[..16.min(m.hash.len())];
                    let size = format_bytes(m.size_bytes as u64);
                    let fmt = m.format.as_deref().unwrap_or("unknown");
                    println!(
                        "  {}",
                        tf(
                            "proxy.status.recent_line",
                            &[("hash", &hash_short), ("size", &size), ("format", &fmt)]
                        )
                        .dimmed()
                    );
                }
                if models.len() > 5 {
                    let more = models.len() - 5;
                    println!("  {}", tf("proxy.status.more", &[("count", &more)]));
                }
            }
        }
        Err(e) => {
            let es = format!("{:#}", e);
            println!("  {} {}", "⚠".yellow(), tf("proxy.status.list_failed", &[("error", &es)]));
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// proxy token-rotate command
// ─────────────────────────────────────────────────────────────────────────────

fn proxy_token_rotate_command(store_path: PathBuf) -> Result<()> {
    let mut config = load_config(&store_path).unwrap_or_default();

    let new_token = Uuid::new_v4().to_string();
    // Write `[auth] token` *and* `[proxy.auth] tokens`: the proxy reads the
    // latter, older versions read the former. Rotating only one left the server
    // accepting the old token after a restart (A6).
    config.set_proxy_token(&new_token);
    save_config(&store_path, &config)?;

    println!("{}", "✓ Bearer token rotated successfully.".green().bold());
    println!("  {}", format!("New token: {}", new_token).bold());
    println!(
        "  {}",
        format!(
            "Written to {}/modeld.toml as [auth] token and [proxy.auth] tokens.",
            store_path.display()
        )
        .dimmed()
    );
    println!("  {}", "Restart the proxy server for the change to take effect.".dimmed());
    println!(
        "  {}",
        "(--reload / SIGHUP hot-reload is not implemented yet; see ROADMAP §12.3)".dimmed()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// doctor command
// ─────────────────────────────────────────────────────────────────────────────
