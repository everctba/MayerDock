"""Smoke de guardrails (md-d33.3.8): blocklist, redaction e auditoria.

Uso: python scripts/guardrails_smoke.py
"""
import base64
import io
import json
import os
import subprocess
import sys
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
if not os.path.isfile(EXE):
    EXE = os.path.join(ROOT, "target", "release", "mayerdock.exe")
GUARDRAILS = os.path.join(os.environ["APPDATA"], "MayerDock", "guardrails.json")
AUDIT = os.path.join(os.environ["APPDATA"], "MayerDock", "audit.jsonl")


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen(
            [EXE, "--mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, encoding="utf-8", bufsize=1)
        self.id = 0

    def call(self, method, params=None, notify=False):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.id += 1
            msg["id"] = self.id
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        if notify:
            return None
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("sem resposta: " + method)
        return json.loads(line)

    def tool(self, name, args=None):
        return self.call("tools/call", {"name": name, "arguments": args or {}})

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=8)
        except Exception:
            self.proc.kill()


def text_of(resp):
    return "\n".join(i.get("text", "") for i in resp.get("result", {}).get("content", [])
                     if i.get("type") == "text")


def cli_windows():
    out = subprocess.run([EXE, "--list-windows", "--json"], capture_output=True, text=True)
    return json.loads(out.stdout)


def write_guardrails(data):
    os.makedirs(os.path.dirname(GUARDRAILS), exist_ok=True)
    with open(GUARDRAILS, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(data, handle, ensure_ascii=False)


def main():
    for path in (GUARDRAILS, AUDIT):
        if os.path.exists(path):
            os.remove(path)

    mcp = Mcp()
    errors = []
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "Guardrails", "version": "0"}})
        mcp.call("notifications/initialized", notify=True)
        mcp.tool("open_app", {"path": "notepad.exe"})
        import time
        time.sleep(2.0)

        window = None
        for item in cli_windows().get("windows", []):
            if item.get("class", "").lower() == "notepad":
                window = item
                break
        if not window:
            return 1
        hwnd = window["hwnd"]
        rect = window["rect"]
        print("notepad:", hex(hwnd), window["title"])

        status = text_of(mcp.tool("guardrails_status"))
        print("guardrails_status ok:", "blocked_processes" in status)

        # 1) blocklist por tÃ­tulo
        write_guardrails({
            "disable_defaults": True,
            "blocked_title_patterns": [window["title"][:9]],
        })
        listed = cli_windows().get("windows", [])
        hidden = all(item["hwnd"] != hwnd for item in listed)
        print("blocklist: janela oculta do inventÃ¡rio =", hidden)
        denied = mcp.tool("capture_window", {"hwnd": hwnd})
        denied_text = text_of(denied)
        print("blocklist: captura bloqueada =", denied["result"].get("isError"),
              "|", denied_text.strip()[:80])
        audit_ok = os.path.exists(AUDIT) and '"allowed":false' in open(AUDIT, encoding="utf-8").read()
        print("auditoria registrou bloqueio =", audit_ok)
        errors += [] if hidden and denied["result"].get("isError") and audit_ok else ["blocklist/audit"]

        # 2) redaction de regiÃ£o (canto superior esquerdo da janela)
        write_guardrails({
            "disable_defaults": True,
            "redact_regions": [{
                "left": rect["left"],
                "top": rect["top"],
                "right": rect["left"] + 120,
                "bottom": rect["top"] + 60,
            }],
        })
        captured = mcp.tool("capture_window", {"hwnd": hwnd})
        image = [i for i in captured["result"]["content"] if i["type"] == "image"][0]
        png = base64.b64decode(image["data"])
        img = Image.open(io.BytesIO(png)).convert("RGB")
        corner = img.crop((0, 0, 120, 60))
        corner_max = max(corner.getextrema()[0][1], corner.getextrema()[1][1], corner.getextrema()[2][1])
        outside = img.crop((200, 200, 400, 300))
        outside_max = max(outside.getextrema()[0][1], outside.getextrema()[1][1], outside.getextrema()[2][1])
        print("redaction: canto preto =", corner_max == 0, "| fora preservado =", outside_max > 0)
        errors += [] if corner_max == 0 and outside_max > 0 else ["redaction"]

        mcp.tool("window_state", {"hwnd": hwnd, "action": "close"})
    finally:
        mcp.close()
        try:
            os.remove(GUARDRAILS)
        except OSError:
            pass

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: guardrails passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())

