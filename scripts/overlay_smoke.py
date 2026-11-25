"""Smoke do overlay 'screencast key': tooltips de ação que somem em ~0,5s.

Liga via MAYERDOCK_OVERLAY=1, dispara click/scroll/capture, verifica a janela
layered visível e, após 1,2 s, escondida. Também testa o toggle da bandeja.
"""
import ctypes
import ctypes.wintypes as wt
import json
import os
import subprocess
import sys
import time

from PIL import ImageGrab

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
if not os.path.isfile(EXE):
    EXE = os.path.join(ROOT, "target", "release", "mayerdock.exe")
LOGF = os.path.join(os.environ["APPDATA"], "MayerDock", "mayerdock.log")
user32 = ctypes.windll.user32


class RECT(ctypes.Structure):
    _fields_ = [("left", wt.LONG), ("top", wt.LONG), ("right", wt.LONG), ("bottom", wt.LONG)]


def find_dock(pid):
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _l):
        p = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
        if p.value == pid:
            buf = ctypes.create_unicode_buffer(256)
            user32.GetClassNameW(hwnd, buf, 256)
            if buf.value == "MayerDock.DockWindow.v1":
                found.append(hwnd)
                return False
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


def find_overlay():
    buf = ctypes.create_unicode_buffer(256)
    hwnd = user32.FindWindowW("MayerDock.Overlay.v1", None)
    return hwnd


def visible(hwnd):
    return bool(hwnd) and user32.IsWindowVisible(hwnd) != 0


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


def main():
    errors = []
    env = dict(os.environ, MAYERDOCK_OVERLAY="1")
    dock = subprocess.Popen([EXE], env=env)
    time.sleep(2.5)
    dock_hwnd = find_dock(dock.pid)
    if not dock_hwnd:
        print("ERRO: dock não subiu")
        return 1
    mcp = Mcp()
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "OverlayTest", "version": "0"}})
        mcp.tool("open_app", {"path": "notepad.exe"})
        time.sleep(2.0)
        cli = subprocess.run([EXE, "--list-windows", "--json"], capture_output=True, text=True)
        np = next((w for w in json.loads(cli.stdout)["windows"]
                   if w.get("class", "").lower() == "notepad"), None)
        if not np:
            print("ERRO: notepad não abriu")
            return 1
        hwnd = np["hwnd"]

        mcp.tool("scroll", {"hwnd": hwnd, "x": 50, "y": 80, "delta": -120})
        mcp.tool("click", {"hwnd": hwnd, "x": 60, "y": 90})
        mcp.tool("capture_window", {"hwnd": hwnd})
        time.sleep(0.1)
        overlay = find_overlay()
        print("overlay criado:", bool(overlay), "| visível:", visible(overlay))
        if not overlay or not visible(overlay):
            errors.append("overlay não ficou visível após ações")
        else:
            ImageGrab.grab().save(os.path.join(ROOT, "target", "overlay.png"))

        time.sleep(1.2)
        print("overlay após 1.2s visível:", visible(overlay))
        if visible(overlay):
            errors.append("overlay não sumiu após ~0,5s")

        # toggle pela bandeja (IDM_OVERLAY = 105)
        user32.PostMessageW(dock_hwnd, 0x8000 + 41, 105, 0)
        time.sleep(0.5)
        log = open(LOGF, encoding="utf-8", errors="replace").read()
        if "overlay: desligado" not in log:
            errors.append("toggle da bandeja não desligou o overlay")

        mcp.tool("window_state", {"hwnd": hwnd, "action": "close"})
    finally:
        try:
            mcp.close()
        except Exception:
            pass
        subprocess.run(["taskkill", "/IM", "mayerdock.exe", "/F"], capture_output=True)
        time.sleep(1.0)

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: overlay passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
