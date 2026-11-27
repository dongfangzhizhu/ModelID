#!/usr/bin/env python3
"""Measure modeld proxy throughput against a raw loopback file sender (C6).

The baseline is a minimal Python socket server that streams the same bytes with
a 1 MiB read loop. It is not a network line-rate measurement; it is a stable
local reference for the same machine and same loopback interface. The modeld
proxy must reach at least 80% of that reference.
"""

from __future__ import annotations

import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "target" / "c6-proxy-bench"
SIZE = 64 * 1024 * 1024
CHUNK = 1024 * 1024
MODEL = Path(os.environ.get("MODELD_BENCH_BIN", str(ROOT / "target" / "release" / "modeld.exe")))
MIN_RATIO = 0.80
CLIENTS = 4


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def run(*args: str) -> None:
    subprocess.run(args, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)


def make_payload(path: Path) -> None:
    chunk = bytes((i % 251 for i in range(CHUNK)))
    with path.open("wb") as out:
        for _ in range(SIZE // CHUNK):
            out.write(chunk)


def fetch_bytes(port: int, path: str) -> int:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=120)
    try:
        conn.request("GET", path)
        response = conn.getresponse()
        if response.status != 200:
            raise RuntimeError(f"GET {path} returned {response.status}")
        total = 0
        while True:
            block = response.read(64 * 1024)
            if not block:
                break
            total += len(block)
        return total
    finally:
        conn.close()


def timed_download(port: int, path: str, clients: int) -> tuple[float, int]:
    results: list[int] = []

    def worker() -> None:
        results.append(fetch_bytes(port, path))

    threads = [threading.Thread(target=worker) for _ in range(clients)]
    start = time.perf_counter()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    elapsed = time.perf_counter() - start
    return elapsed, sum(results)


def serve_many(path: Path, port: int, clients: int) -> threading.Thread:
    def handle(conn: socket.socket) -> None:
        with conn:
            while b"\r\n\r\n" not in conn.recv(4096):
                pass
            header = (
                "HTTP/1.1 200 OK\r\n"
                f"Content-Length: {path.stat().st_size}\r\n"
                "Connection: close\r\n\r\n"
            ).encode()
            conn.sendall(header)
            with path.open("rb") as source:
                while block := source.read(CHUNK):
                    conn.sendall(block)

    def server() -> None:
        with socket.socket() as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(("127.0.0.1", port))
            listener.listen(clients)
            for _ in range(clients):
                conn, _ = listener.accept()
                threading.Thread(target=handle, args=(conn,), daemon=True).start()
            # Give the handler threads time to finish before the test timer ends.
            time.sleep(5)

    thread = threading.Thread(target=server, daemon=True)
    thread.start()
    return thread


def main() -> int:
    WORK.mkdir(parents=True, exist_ok=True)
    data = WORK / "data"
    store = WORK / "store"
    data.mkdir(exist_ok=True)
    payload = data / "bench.safetensors"
    if not payload.exists() or payload.stat().st_size != SIZE:
        make_payload(payload)

    if not (store / "modeld.db").exists():
        run(str(MODEL), "init", "--path", str(store))
    run(str(MODEL), "--store", str(store), "scan", str(data), "--copy")

    proxy_port = free_port()
    proxy = subprocess.Popen(
        [
            str(MODEL), "proxy", "start",
            "--bind", "127.0.0.1", "--port", str(proxy_port),
            "--store", str(store), "--allow-anonymous",
        ],
        cwd=ROOT,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        for _ in range(100):
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{proxy_port}/health", timeout=2
                ) as response:
                    if response.status == 200:
                        break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError("proxy did not become ready")

        with urllib.request.urlopen(
            f"http://127.0.0.1:{proxy_port}/v1/models", timeout=10
        ) as response:
            models = json.load(response)["models"]
        if not models:
            raise RuntimeError("no model was ingested")
        blob = f"/v1/blobs/{models[0]['hash']}"

        proxy_seconds, proxy_bytes = timed_download(proxy_port, blob, CLIENTS)
    finally:
        proxy.terminate()
        try:
            proxy.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proxy.kill()

    baseline_port = free_port()
    baseline_thread = serve_many(payload, baseline_port, CLIENTS)
    baseline_seconds, baseline_bytes = timed_download(baseline_port, "/bench", CLIENTS)
    baseline_thread.join(timeout=10)

    expected = SIZE * CLIENTS
    if proxy_bytes != expected or baseline_bytes != expected:
        raise RuntimeError(f"byte count mismatch: proxy={proxy_bytes}, baseline={baseline_bytes}")

    proxy_mbps = proxy_bytes / proxy_seconds / 1024 / 1024
    baseline_mbps = baseline_bytes / baseline_seconds / 1024 / 1024
    ratio = proxy_mbps / baseline_mbps
    print(
        f"{CLIENTS} clients — proxy: {proxy_mbps:.1f} MiB/s, "
        f"baseline: {baseline_mbps:.1f} MiB/s, "
        f"ratio: {ratio:.2f} (minimum {MIN_RATIO:.2f})"
    )
    return 0 if ratio >= MIN_RATIO else 1


if __name__ == "__main__":
    sys.exit(main())
