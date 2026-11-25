#!/usr/bin/env python3
"""对发布产物做签名（C3）。

默认使用 Sigstore cosign keyless（CI 有 OIDC 身份）；若本机装了 cosign，
`python3 scripts/sign_release.py dist/` 会为 SHA256SUMS 生成 `.sig` 与 `.pem`。
本机无 cosign / 无 OIDC 时脚本明确报错退出，不伪造签名文件。
"""
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path


def main() -> int:
    dist = Path(sys.argv[1] if len(sys.argv) > 1 else "dist")
    if not dist.is_dir():
        print(f"error: {dist} is not a directory", file=sys.stderr)
        return 2
    sums = sorted(dist.glob("*SHA256SUMS.txt"))
    if not sums:
        print(f"error: no *SHA256SUMS.txt in {dist}", file=sys.stderr)
        return 2
    cosign = shutil.which("cosign")
    if not cosign:
        print("error: cosign not found; install Sigstore cosign to sign releases", file=sys.stderr)
        return 3
    for checksum_file in sums:
        print(f"signing {checksum_file.name} with cosign keyless")
        subprocess.run(
            [cosign, "sign-blob", "--yes", "--output-signature",
             str(checksum_file.with_suffix(checksum_file.suffix + ".sig")),
             "--output-certificate", str(checksum_file.with_suffix(".pem")),
             str(checksum_file)],
            check=True,
        )
    print("signed", len(sums), "checksum file(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
