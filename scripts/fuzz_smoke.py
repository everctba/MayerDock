"""Smoke de fuzz end-to-end do sidecar MCP (mayerdock.exe --mcp).

Envia corpus hostil + payloads grandes/profundos no stdin e depois uma
sequência válida (initialize + tools/list), verificando que o processo
sobrevive e responde. Uso:

    python scripts/fuzz_smoke.py [--exe caminho]
"""

import argparse
import json
import os
import subprocess
import sys
import threading
import time


def default_exe():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    for candidate in (
        os.path.join(root, "dist", "MayerDock", "mayerdock.exe"),
        os.path.join(root, "target", "release", "mayerdock.exe"),
    ):
        if os.path.isfile(candidate):
            return candidate
    raise SystemExit("mayerdock.exe não encontrado (rode .\\build.ps1 antes)")


def read_line_with_timeout(proc, timeout=10.0):
    box = {}

    def work():
        box["line"] = proc.stdout.readline()

    thread = threading.Thread(target=work, daemon=True)
    thread.start()
    thread.join(timeout)
    if thread.is_alive():
        return None
    return box.get("line")


def read_until_id(proc, wanted, timeout=10.0):
    """Lê respostas até encontrar o id pedido (ignora respostas do corpus)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        remaining = max(0.5, deadline - time.time())
        line = read_line_with_timeout(proc, remaining)
        if not line:
            return None
        try:
            response = json.loads(line)
        except Exception:
            continue
        if response.get("id") == wanted:
            return response
    return None


def send(proc, payload: bytes):
    proc.stdin.write(payload)
    if not payload.endswith(b"\n"):
        proc.stdin.write(b"\n")
    proc.stdin.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", default=default_exe())
    args = parser.parse_args()

    corpus_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fuzz_corpus", "mcp.jsonl")
    corpus = open(corpus_path, "rb").read().splitlines()

    proc = subprocess.Popen(
        [args.exe, "--mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    watchdog = threading.Timer(30.0, proc.kill)
    watchdog.start()
    errors = []
    try:
        # 1) corpus hostil (linhas individuais)
        for line in corpus:
            send(proc, line)

        # 2) JSON com aninhamento profundo (~200 KB)
        send(proc, b"[" * 100_000 + b"]" * 100_000)

        # 3) linha muito grande (> MAX_MCP_LINE de 4 MB) — deve ser descartada
        send(proc, b"A" * (5 * 1024 * 1024))

        # 4) bytes inválidos em UTF-8
        send(proc, b"\xff\xfe\x00\x01\x02")

        # 5) linha vazia e linha com só espaços
        send(proc, b"")
        send(proc, b"   ")

        # 6) sequência válida: o processo precisa continuar respondendo
        send(proc, json.dumps({
            "jsonrpc": "2.0", "id": 9001, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "FuzzSmoke", "version": "0"},
            },
        }).encode())
        line = read_until_id(proc, 9001)
        if not line:
            errors.append("sem resposta ao initialize depois do corpus hostil")
        else:
            response = line
            if "result" not in response:
                errors.append(f"resposta inesperada ao initialize: {response}")

        send(proc, json.dumps({"jsonrpc": "2.0", "id": 9002, "method": "tools/list"}).encode())
        response = read_until_id(proc, 9002)
        if not response:
            errors.append("sem resposta ao tools/list")
        else:
            tools = response.get("result", {}).get("tools", [])
            print(f"tools: {len(tools)}")
            if len(tools) < 10:
                errors.append(f"tools/list suspeito: {len(tools)} tools")

        if proc.poll() is not None:
            errors.append(f"sidecar morreu (exit {proc.returncode})")
        else:
            print("sidecar vivo após corpus hostil: True")
    finally:
        watchdog.cancel()
        try:
            proc.stdin.close()
        except Exception:
            pass
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()

    if errors:
        for error in errors:
            print("FALHA:", error)
        return 1
    print("OK: fuzz smoke passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
