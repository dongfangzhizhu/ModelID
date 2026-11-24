# ADR-0003：鉴权、网络暴露与安全默认

- **状态**：Accepted（as-built，2026-11-24；源码基线 `f6663ce`）
- **日期**：2026-11-24
- **相关任务**：A3、A4、A6、B9
- **相关文档**：[proxy HTTP API](../proxy-api.md)、[SECURITY.md](../../SECURITY.md)、[HF 兼容](0005-huggingface-compatibility.md)

## 背景

早期版本存在三类安全漂移：代理默认监听 `0.0.0.0` 且鉴权形同虚设；WebUI 默认无 token 且 CORS 全开；配置写入会清空未知段。当前默认必须「先关闭，再按需打开」，并且网络暴露要有显式开关。

## 决策

1. **代理默认关闭**：`127.0.0.1:8234`，`require_token = true`，`allow_anonymous = false`，`tokens = []`。CLI 首次启动会生成 UUIDv4 token 并保存到 `<store>/modeld.toml`；仅靠库默认配置且没有 token 时启动校验失败，不会启动一个「要求 token 但没有 token」的服务。
   - 源码：`crates/modeld-core/src/config.rs::{ProxyConfig::default,check_startup_invariants}`、`crates/modeld-cli/src/commands/proxy.rs`。

2. **token 解析顺序固定**：`--token` → `--config` 文件的 `[proxy.auth].tokens` → `<store>/modeld.toml` 的 `[proxy.auth].tokens`（为空回退 `[auth].token`）→ `MODELD_AUTH_TOKEN` → 自动生成并双写。轮换同时更新两个字段，但**需要重启进程**；`--config` 提供的 token 优先级高于 store 文件。
   - 源码：`crates/modeld-cli/src/commands/proxy.rs::{proxy_start_command,proxy_token_rotate_command}`、`crates/modeld-core/src/config.rs::{proxy_tokens,set_proxy_token}`。

3. **网络暴露必须显式**：非 loopback 绑定必须带 `--allow-lan`；`--allow-anonymous` 只允许 loopback。校验发生在绑定端口之前，因此错误配置不会短暂暴露服务。`--bind` 的 CLI 默认值会覆盖配置文件中的 `bind_address`。
   - 源码：`crates/modeld-cli/src/commands/proxy.rs`、`crates/modeld-core/src/config.rs::is_loopback_addr`、`crates/modeld-proxy/src/server.rs::start`。

4. **鉴权顺序**：先做 IP gate（deny 优先；空 allow 表示不限制），再检查是否禁用匿名（401 `anonymous access disabled`），最后校验 Bearer token（401 `invalid or missing token`）。token 比较对多个 token 不短路、等长逐字节 XOR 折叠；长度不等直接拒绝。
   - 源码：`crates/modeld-proxy/src/auth.rs::{check_ip,check_auth,tokens_equal}`、`src/server.rs::handle_request`。
   - 限制：IPv4 CIDR 是精确语义；IPv6 / hostname 退化为 `starts_with` 子串匹配，不应把它们当作安全边界。

5. **WebUI 的浏览器防护**：CORS 是 origin 白名单；对带 `Origin` 的 `POST/PUT/PATCH/DELETE` 要求白名单 origin + `X-Modeld-CSRF`；无 `Origin` 的 curl/CLI 不受 CSRF guard 限制（这是设计边界，不是漏洞修补）。Bearer token 通过 `Authorization`；仅 WebSocket 握手允许 `?token=`。
   - 源码：`crates/modeld-webui/src/{server.rs,auth.rs}`。

6. **可观测性的边界**：proxy 不提供 `/metrics`；WebUI 的 `GET /metrics` 故意免鉴权，任何能访问该端口的人都能读取计数器，需在网络层限制。`AppState.rate_limit_per_min` 字段存在但尚未挂中间件。
   - 源码：`crates/modeld-webui/src/{server.rs,state.rs}`、`crates/modeld-proxy/src/server.rs`。

## 后果

- 优点：默认安装不会对外暴露；多 token 不泄漏匹配位置；浏览器跨站写请求被挡；密钥双写避免旧格式回退后鉴权失效。
- 代价：HF 框架默认不会自动携带 modeld 的 Bearer token，代理模式需要显式配置 token 或在回环环境使用 `--allow-anonymous`；轮换后必须重启；IP 规则对 IPv6 不够严谨。
- 明确不做：HTTPS/TLS（只面向可信 LAN）；OS keyring；热重载；把 WebUI 的 `/metrics` 接入鉴权（留给 C4）。

## 验证

- `crates/modeld-proxy/tests/proxy_integration.rs`：token 缺失/无效 401、IP deny 403、慢下载期间 `/health` 响应。
- `crates/modeld-webui/tests/api_e2e.rs`：全路由 401 sweep、CSRF/CORS/metrics 行为。
- `crates/modeld-webui/src/server.rs` 内联测试：CSRF header、origin 白名单、token 不写日志。
- 手动：非 loopback 绑定不带 `--allow-lan` 退出码 1；`--allow-anonymous` 搭配非 loopback 被拒。
