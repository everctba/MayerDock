"""Smoke de encaixe (dock_pin/unpin) — valida geometria real, sem screenshot.

Fluxo: abre Notepad -> dock_pin -> slot ocupado + janela exatamente no retângulo
do slot -> dock_unpin -> slot livre + janela restaurada na posição original.
"""
import ctypes
import ctypes.wintypes as wt
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
if not os.path.isfile(EXE):
    EXE = os.path.join(ROOT, "target", "release", "mayerdock.exe")
user32 = ctypes.windll.user32


class RECT(ctypes.Structure):
    _fields_ = [("left", wt.LONG), ("top", wt.LONG), ("right", wt.LONG), ("bottom", wt.LONG)]


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen([EXE, "--mcp"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     text=True, encoding="utf-8", bufsize=1)
        self.id = 0

    def call(self, method, params=None):
        self.id += 1
        msg = {"jsonrpc": "2.0", "id": self.id, "method": method, "params": params or {}}
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("sem resposta: " + method)
        return json.loads(line)

    def tool(self, name, args=None):
        self.id += 1
        msg = {"jsonrpc": "2.0", "id": self.id, "method": "tools/call",
               "params": {"name": name, "arguments": args or {}}}
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("sem resposta: " + name)
        resp = json.loads(line)
        text = "\n".join(i.get("text", "") for i in resp["result"].get("content", []))
        return resp, text

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=8)
        except Exception:
            self.proc.kill()


def rect_of(hwnd):
    r = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return (r.left, r.top, r.right, r.bottom)


def close_enough(a, b, tolerance=4):
    return all(abs(x - y) <= tolerance for x, y in zip(a, b))


def slots(mcp):
    _, text = mcp.tool("list_slots")
    return json.loads(text)


def main():
    errors = []
    notepad = subprocess.Popen(["notepad.exe"])
    time.sleep(2.0)
    dock = subprocess.Popen([EXE])
    time.sleep(2.5)
    mcp = Mcp()
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "PinTest", "version": "0"}})
        mcp.call("notifications/initialized")

        cli = subprocess.run([EXE, "--list-windows", "--json"], capture_output=True, text=True)
        window = next((w for w in json.loads(cli.stdout)["windows"]
                       if w.get("class", "").lower() == "notepad"), None)
        if not window:
            print("ERRO: notepad não achado")
            return 1
        hwnd = window["hwnd"]
        original = rect_of(hwnd)
        print("janela original:", original)

        resp, text = mcp.tool("dock_pin", {"hwnd": hwnd})
        time.sleep(0.8)
        pinned = slots(mcp)
        slot = next((s for s in pinned if s.get("occupied") and s.get("hwnd") == hwnd), None)
        if not slot:
            errors.append(f"dock_pin não ocupou slot: {text}")
        else:
            actual = rect_of(hwnd)
            expected = slot["rect"]
            expected_rect = (expected["left"], expected["top"], expected["right"], expected["bottom"])
            print("slot:", slot["index"], "| retângulo esperado:", expected_rect, "| janela:", actual)
            if not close_enough(actual, expected_rect):
                errors.append(f"janela fora do slot: {actual} != {expected_rect}")

        resp, text = mcp.tool("dock_unpin", {"hwnd": hwnd})
        time.sleep(0.8)
        freed = slots(mcp)
        if any(s.get("occupied") and s.get("hwnd") == hwnd for s in freed):
            errors.append("dock_unpin não liberou o slot")
        restored = rect_of(hwnd)
        print("janela após unpin:", restored)
        if not close_enough(restored, original):
            errors.append(f"unpin não restaurou posição: {restored} != {original}")
    finally:
        try:
            mcp.close()
        except Exception:
            pass
        try:
            notepad.terminate()
        except Exception:
            pass
        try:
            pass
        finally:
            subprocess.run(["taskkill", "/IM", "mayerdock.exe", "/F"], capture_output=True)
            time.sleep(1.0)

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: dock_pin/unpin passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
