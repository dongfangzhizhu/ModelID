#!/usr/bin/env python3
"""阶段 C 出口 Gate 的可复现静态自检（C 收口）。

阶段 C 的正式出口标准是「三平台可安装；发布 0.2.0；端到端生态接入验收」。
其中真实安装、真实 publish、真实前端出图依赖外部平台/凭据，本机无法验证，
由路线图的「待办/边界」如实登记；本脚本只检查能从仓库确定性判定的 as-built
不变量，防止后续改动把它们悄悄移除：

1. Rust workspace 与 Python 包版本统一为 0.2.0；
2. 所有 crate 继承 workspace 的 publish = true；
3. release workflow 含 build / sbom / sign / publish-crates / publish-pypi jobs；
4. SBOM 存在且含非零组件；
5. 三平台安装清单（Homebrew / winget / scoop）齐备；
6. 守护单元、Grafana 面板、生态接入文档齐备；
7. C1–C7 在状态板全部为 done；
8. C5/C6 与阶段 B 的静态门全部通过：
   scripts/{ui_static_check,c6_gate,stage_b_gate}.py。

退出码：0 全部通过，1 至少一项失败，2 输入缺失。
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION = "0.2.0"
REQUIRED_RELEASE_JOBS = {
    "build",
    "sbom",
    "sign",
    "publish-crates",
    "publish-pypi",
}
REQUIRED_ARTIFACTS = [
    "deploy/systemd/modeld-daemon.service",
    "deploy/grafana/modeld-dashboard.json",
    "deploy/homebrew/modeld.rb",
    "deploy/winget/modeld.version.yaml",
    "deploy/scoop/modeld.json",
    "docs/frontend-integration.md",
    "docs/release-checklist.md",
    "sbom/modeld-0.2.0.json",
]
REQUIRED_STATIC_GATES = ["ui_static_check.py", "c6_gate.py", "stage_b_gate.py"]


def check(name: str, ok: bool, detail: str) -> tuple[str, bool, str]:
    return name, ok, detail


def workspace_version() -> str | None:
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r"^\[workspace\.package\].*?^version\s*=\s*\"([^\"]+)\"", cargo, re.S | re.M)
    return match.group(1) if match else None


def python_version() -> str | None:
    pyproject = (ROOT / "python/pyproject.toml").read_text(encoding="utf-8")
    match = re.search(r"^version\s*=\s*\"([^\"]+)\"", pyproject, re.M)
    return match.group(1) if match else None


def crates_publish_inherited() -> tuple[bool, str]:
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    workspace_publish = re.search(r"^\[workspace\.package\].*?^publish\s*=\s*true", cargo, re.S | re.M)
    if not workspace_publish:
        return False, "workspace 未声明 publish = true"
    manifests = sorted((ROOT / "crates").glob("*/Cargo.toml"))
    bad = []
    for manifest in manifests:
        if "publish.workspace = true" not in manifest.read_text(encoding="utf-8"):
            bad.append(manifest.parent.name)
    return not bad, ("全部继承 workspace publish" if not bad else "未继承：" + ", ".join(bad))


def release_jobs() -> set[str]:
    workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    return set(re.findall(r"^  ([a-z][a-z0-9_-]*):\s*$", workflow, re.M))


def sbom_components() -> int:
    path = ROOT / "sbom/modeld-0.2.0.json"
    if not path.exists():
        return 0
    data = json.loads(path.read_text(encoding="utf-8"))
    return len(data.get("components", []))


def stage_c_tasks_done() -> tuple[bool, str]:
    roadmap = (ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
    rows = re.findall(r"^\| (C[1-7]) \|[^|]*\| ([^|]+) \|", roadmap, re.M)
    by_id = dict(rows)
    missing = [f"C{i}" for i in range(1, 8) if not by_id.get(f"C{i}", "").strip().startswith("done")]
    return not missing, ("C1–C7 全部 done" if not missing else "未完成：" + ", ".join(missing))


def run_static_gate(script: str) -> tuple[bool, str]:
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / script)],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    detail = result.stdout.strip().splitlines()
    return result.returncode == 0, detail[-1] if detail else result.stderr.strip()


def run() -> list[tuple[str, bool, str]]:
    checks: list[tuple[str, bool, str]] = []

    rs_ver, py_ver = workspace_version(), python_version()
    checks.append(
        check("版本统一 0.2.0", rs_ver == VERSION and py_ver == VERSION,
              f"Cargo={rs_ver or '缺失'}, Python={py_ver or '缺失'}")
    )

    ok, detail = crates_publish_inherited()
    checks.append(check("crate publish 配置", ok, detail))

    jobs = release_jobs()
    missing_jobs = REQUIRED_RELEASE_JOBS - jobs
    checks.append(
        check("release jobs 齐备", not missing_jobs,
              "缺少：" + ", ".join(sorted(missing_jobs)) if missing_jobs else "五项发布 job 全部存在")
    )

    components = sbom_components()
    checks.append(check("SBOM 含组件", components > 0, f"components = {components}"))

    missing_artifacts = [a for a in REQUIRED_ARTIFACTS if not (ROOT / a).exists()]
    checks.append(
        check("部署与生态产物齐备", not missing_artifacts,
              "缺少：" + ", ".join(missing_artifacts) if missing_artifacts else "8 项产物全部存在")
    )

    ok, detail = stage_c_tasks_done()
    checks.append(check("C1–C7 状态板 done", ok, detail))

    for script in REQUIRED_STATIC_GATES:
        ok, detail = run_static_gate(script)
        checks.append(check(f"静态门 {script}", ok, detail))

    return checks


def main() -> int:
    checks = run()
    print(f"{'检查项':<32} {'结果':<6} 说明")
    print("-" * 78)
    for name, ok, detail in checks:
        print(f"{name:<32} {'PASS' if ok else 'FAIL':<6} {detail}")
    failed = [name for name, ok, _ in checks if not ok]
    if failed:
        print(f"\n未通过 {len(failed)} 项：{', '.join(failed)}", file=sys.stderr)
        return 1
    print("\n阶段 C 出口 Gate 静态检查全部通过")
    print("注意：三平台安装、真实 publish、真实前端出图需外部平台，未在本脚本验证范围内。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
