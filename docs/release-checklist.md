# 发布检查清单（0.2.0 / C3）

本清单区分「本地实测」与「需要外部凭据/平台，未在本机验证」，避免把准备
工作误说成已发布。

## 已本地实测（有命令与输出）

- **版本号统一**：根 `Cargo.toml`、5 个 crate（workspace 继承）、
  `python/pyproject.toml`、`python/modeld_hook/__init__.py` 均为 `0.2.0`。
  - `./target/debug/modeld.exe --version` → `modeld 0.2.0`。
- **`cargo install` 可用**：
  `cargo install --path crates/modeld-cli --root <tmp> --locked` 成功，
  安装出的 `<tmp>/bin/modeld.exe --version` → `modeld 0.2.0`。
  发布到 crates.io 后 `cargo install modeld-cli` 走同一份清单。
- **crate 可打包**：`cargo package -p modeld-core --locked --allow-dirty --no-verify`
  成功（36 文件）。依赖 crate 需先发布到 crates.io 才能打包，发布工作流
  `publish-crates` 按 core → client → proxy → webui → cli 拓扑顺序执行。
- **SBOM 可复现**：`python3 scripts/sbom.py` 从真实 `cargo metadata` 生成
  `sbom/modeld-0.2.0.json`（CycloneDX 1.5，372 组件，含 modeld-cli/rusqlite）。
- **签名脚本诚实失败**：`python3 scripts/sign_release.py <dir>` 在没有 cosign
  或没有 SHA256SUMS 时明确报错退出（2/3），不伪造 `.sig`。
- **工作流结构**：`.github/workflows/release.yml` 新增 `sbom`/`sign`/
  `publish-crates`/`publish-pypi` 四个 job，任务名无重复。

## 需要外部条件，本机未验证（如实登记）

- **真实发布到 crates.io**：需 `CRATESIO_TOKEN` 与仓库变量
  `PUBLISH_CRATES=true`；本机无凭据，未执行 `cargo publish`。
- **真实发布到 PyPI**：需 `PYPI_TOKEN` 与 `PUBLISH_PYPI=true`；本机未执行。
- **三平台安装包可安装并 `--version` 正常**：release.yml 的五平台矩阵
  （Linux x86_64/aarch64、macOS x86_64/aarch64、Windows x86_64）产物已在
  CI 定义，但本机只在 Windows 验证过 `modeld.exe --version`；Linux/macOS
  的安装包未在对应平台实机安装验证。
- **签名与校验**：CI 用 sigstore cosign keyless（OIDC `id-token: write`）；
  本机无 cosign 与 CI OIDC 身份，未产出真实 `.sig`/`.pem`。
- **Homebrew / winget / scoop 实际安装**：`deploy/` 下的清单含
  `REPLACE_WITH_SHA256_*` 占位符，需由 CI 用 `SHA256SUMS` 回填后提交到
  各包管理器的仓库；本机未做 `brew install` / `winget install` / `scoop install`。
- **SHA256SUMS 由 CI `checksums` job 生成**：本机未跑完整 5 平台构建矩阵。

## 发布顺序（人工触发 tag 后由 CI 完成）

1. 打 tag `v0.2.0` 触发 `release.yml`。
2. `build` / `macos-universal` → 上传 5 平台归档。
3. `checksums` → `modeld-v0.2.0-SHA256SUMS.txt`。
4. `sbom` → `sbom/modeld-0.2.0.json` 上传 Release。
5. `sign` → 对 SHA256SUMS 做 keyless 签名并上传 `.sig`/`.pem`。
6. `publish-crates`（需 `PUBLISH_CRATES=true`）按拓扑顺序发布 5 个 crate。
7. `publish-pypi`（需 `PUBLISH_PYPI=true`）发布 `modeld-hook` 0.2.0。
8. 人工把 `deploy/` 清单的 SHA256 占位符替换为第 3 步的真实值，提交到
   Homebrew/winget/scoop 对应仓库。
