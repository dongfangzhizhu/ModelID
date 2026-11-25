#!/usr/bin/env python3
"""生成 CycloneDX 1.5 风格 SBOM（C3）。

直接从 `cargo metadata --format-version 1 --locked` 读取真实依赖图，
不引入 Python 依赖，也不凭空编造组件。输出 `sbom/modeld-<version>.json`，
CI 与本地均可复现。
"""
from __future__ import annotations

import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(out.stdout)


def main() -> int:
    meta = metadata()
    version = next(
        (p["version"] for p in meta["packages"] if p["name"] == "modeld-core"), "0.0.0"
    )
    # workspace 成员标为 application，第三方依赖标为 library。
    members = {p["id"] for p in meta["packages"] if str(p["manifest_path"]).startswith(str(ROOT / "crates"))}
    components = []
    for pkg in meta["packages"]:
        lic = pkg.get("license") or ""
        components.append(
            {
                "type": "application" if pkg["id"] in members else "library",
                "bom-ref": f"pkg:cargo/{pkg['name']}@{pkg['version']}",
                "name": pkg["name"],
                "version": pkg["version"],
                "licenses": ([{"license": {"id": lic}}] if lic and lic != "NONE" else []),
                "purl": f"pkg:cargo/{pkg['name']}@{pkg['version']}",
            }
        )
    components.sort(key=lambda c: c["name"])

    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:modeld-{version}",
        "version": 1,
        "metadata": {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "component": {
                "type": "application",
                "name": "modeld",
                "version": version,
                "purl": f"pkg:cargo/modeld-cli@{version}",
            },
        },
        "components": components,
    }

    out_dir = ROOT / "sbom"
    out_dir.mkdir(exist_ok=True)
    out = out_dir / f"modeld-{version}.json"
    out.write_text(json.dumps(sbom, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {out.relative_to(ROOT)}: {len(components)} components (version {version})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
