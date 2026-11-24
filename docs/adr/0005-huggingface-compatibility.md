# ADR-0005：HuggingFace 缓存兼容与拦截边界

- **状态**：Accepted（as-built，2026-11-24；源码基线 `f6663ce`）
- **日期**：2026-11-24
- **相关任务**：B4、B5、B9
- **相关文档**：[proxy HTTP API](../proxy-api.md)、[proxy setup](../proxy-setup.md)、[鉴权默认](0003-authentication-defaults.md)

## 背景

AI 框架通过 `HF_HOME` / `HF_ENDPOINT` 与 HuggingFace Hub 交互。要复用 CAS，modeld 必须生成一个 `huggingface_hub` 能原样识别的缓存目录，并在代理/下载两条路径上保持 SHA256（HF ETag）与 BLAKE3（CAS key）的映射一致。早期规划稿混淆了几件事：`downloads` 表不是 SHA256↔BLAKE3 的查询表；`import modeld_hook` 曾有副作用且与实际代码不符；`snapshot_download` 从未被真正拦截。

## 决策

1. **假缓存布局以库实现为准**：
   ```text
   <store>/hf_cache/hub/models--<org>--<repo>/
     blobs/<sha256>                 → ../../../../cas/blake3/<2-hex>/<64-hex>
     snapshots/<revision>/<file>    → ../../blobs/<sha256>
     refs/<branch>                  40-hex commit SHA（不是分支名）
   ```
   `models--` / `datasets--` / `spaces--` 由 `RepoType` 决定。符号链接失败是明确错误/降级状态，不静默复制成普通文件。
   - 相对目标按主机使用分隔符：Unix 用 `/`，Windows 用 `\`。Windows 上用 `/` 生成的链接虽然 `read_link()` 有值，但解引用时是悬空链接；B9 回归测试已覆盖。
   - 源码：`crates/modeld-core/src/hf_cache.rs::{relative_target_for_host,create_relative_symlink,create_cache_entry_with_type,check_cache,is_commit_sha}`。

2. **映射表是 `hf_mappings`，不是 `downloads`**：下载时从 HF HEAD 的 `x-linked-etag` / `etag` 提取 64-hex SHA256，写入 `hf_mappings`；后续 `hf-check` 通过 `get_blake3_by_sha256` 命中。无可信 SHA256 时结果标记为 `unverified`，不能假装已验证。
   - 源码：`crates/modeld-core/src/db.rs::{upsert_hf_mapping,get_blake3_by_sha256}`、`crates/modeld-core/src/downloader.rs::{fetch_hf_metadata,download_hf_file}`。

3. **下载可续传且流式校验**：`.part.meta` 记录 URL / ETag / size；续传带 `Range: bytes=N-` 与 `If-Range`；服务端返回 200 则从头开始。固定 64 KB 缓冲同时算 BLAKE3 与 SHA256，已知 size 必须精确匹配。DB 写入包在 `Database::in_transaction`。
   - 源码：`crates/modeld-core/src/downloader.rs::{Downloader,PartMeta}`。

4. **代理兼容路径是 `/v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}`**：客户端设置 `HF_ENDPOINT=<base>/v1/hf-proxy`（不是 `<base>`）；服务端回源地址由 `MODELD_HF_BASE` 控制，默认 `https://huggingface.co`。除 `/health` 外代理要求 Bearer token，所以“零客户端改动”只成立于 token 已正确配置/回环匿名模式。
   - 源码：`crates/modeld-proxy/src/server.rs::{handle_hf_proxy,match_hf_proxy_url}`、`crates/modeld-core/src/downloader.rs::DEFAULT_HF_BASE`。

5. **Python hook 的真实激活方式**：推荐 `HF_HOME="$(modeld hf-setup --print-path)"`；显式 `modeld_hook.activate()`；或 `MODELD_AUTO_HOOK=1`。**import 本身没有副作用**；`MODELD_NO_AUTO_HOOK` 和 `modeld hook install` 在代码中不存在。
   - 源码：`python/modeld_hook/__init__.py::{activate,deactivate,get_hf_home}`、`python/modeld_hook/intercept.py::make_hf_hub_download_wrapper`。

## 后果

- 优点：无需修改框架代码即可命中同一份 CAS；HF 的 SHA256 与 CAS 的 BLAKE3 各司其职；续传不会把变更后的远端对象拼到旧分片上。
- 限制与已知缺口：
  - `cache_dir` / `local_dir` / `force_download` 显式参数会绕过 modeld，交回原 HF 函数。
  - `snapshot_download` 的包装函数存在但未启用；`_original_snapshot_download` 未赋值。
  - Windows 无开发者模式时符号链接失败，HF 缓存命中降级，但 CAS 下载本身仍可成功。
  - `downloader.rs` 中 `snapshot_file_path_with_type` 的 filename / revision 实参顺序疑似传反（写入 WAL/alias 的路径与真实 cache 路径不一致），已记入 ROADMAP §12.3，未在本 ADR 中修复。
  - 20 仓库 `huggingface_hub>=0.24` 对拍未实测（沙箱无外网）。

## 验证

- `crates/modeld-proxy/tests/hf_proxy_test.rs`：miss → CAS → hit 全流程与 `X-Modeld-Cache` / `X-Modeld-Blake3` 头。
- `python/tests/test_modeld_hook.py`：import 无副作用、两处 monkeypatch 的还原、subfolder / repo_type、cache hit/miss、无 CLI 时回退。
- `crates/modeld-core/tests/integration_test.rs` 与下载器内联测试：`.part.meta` 不匹配时丢弃分片、BLAKE3/SHA256 流式校验与 size 强校验。
