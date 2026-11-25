"""Smoke da borda de foco por sessão (md-d33.3.5).

Verifica a cor da borda esquerda do dock:
  sem sessão = azul (74,112,240)
  agente     = cor estável pelo hash do nome
  lease      = âmbar (245,158,11)
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
DOCK = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
MCP = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
if not os.path.isfile(DOCK):
    DOCK = os.path.join(ROOT, "target", "release", "mayerdock.exe")
    MCP = DOCK

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


def rect_of(hwnd):
    r = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


def edge_pixel(r):
    shot = ImageGrab.grab(bbox=(r.left + 1, r.top + 200, r.left + 2, r.top + 201))
    return shot.getpixel((0, 0))


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen([MCP, "--mcp"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     text=True, encoding="utf-8", bufsize=1)
        self.id = 0

    def send(self, method, params=None, notify=False):
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

    def call(self, name, args=None):
        return self.send("tools/call", {"name": name, "arguments": args or {}})

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=8)
        except Exception:
            self.proc.kill()


def fnv_color(name):
    palette = [(74, 112, 240), (139, 92, 246), (20, 184, 166),
               (236, 72, 153), (34, 197, 94), (234, 179, 8)]
    h = 14695981039346656037
    for b in name.encode():
        h ^= b
        h = (h * 1099511628211) & 0xFFFFFFFFFFFFFFFF
    return palette[h % len(palette)]


def main():
    env = dict(os.environ, MAYERDOCK_LEASE_AUTO="1")
    dock = subprocess.Popen([DOCK], env=env)
    time.sleep(2.5)
    hwnd = find_dock(dock.pid)
    if not hwnd:
        print("ERRO: dock não subiu")
        return 1
    rect = rect_of(hwnd)
    default = edge_pixel(rect)
    print("sem sessão:", default)
    errors = []
    if default != (74, 112, 240):
        errors.append(f"cor padrão inesperada: {default}")

    # nome cujo hash cai numa cor != azul, para o teste ser determinístico
    name = next(n for n in ("Cursor", "Claude", "DSH", "Agent", "Test") if fnv_color(n) != (74, 112, 240))
    expected_agent = fnv_color(name)
    mcp = Mcp()
    try:
        mcp.send("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": name, "version": "0"}})
        mcp.send("notifications/initialized", notify=True)
        time.sleep(0.6)
        agent_color = edge_pixel(rect)
        print(f"agente {name}: {agent_color} (esperado {expected_agent})")
        if agent_color != expected_agent:
            errors.append(f"cor do agente: {agent_color} != {expected_agent}")

        mcp.call("open_app", {"path": "notepad.exe"})
        time.sleep(2.0)
        windows = []
        # lista via CLI para descobrir hwnd do notepad
        cli = subprocess.run([MCP, "--list-windows", "--json"], capture_output=True, text=True)
        windows = json.loads(cli.stdout).get("windows", [])
        notepad = next((w for w in windows if w.get("class", "").lower() == "notepad"), None)
        if not notepad:
            errors.append("notepad não achado para o lease")
        else:
            mcp.call("focus_lease", {"hwnd": notepad["hwnd"], "seconds": 4, "reason": "smoke borda"})
            time.sleep(0.6)
            lease_color = edge_pixel(rect)
            print("lease:", lease_color)
            if lease_color != (245, 158, 11):
                errors.append(f"cor de lease: {lease_color}")
            time.sleep(4.5)
            after = edge_pixel(rect)
            print("após lease:", after)
            if after != expected_agent:
                errors.append(f"cor pós-lease: {after} != {expected_agent}")
            mcp.call("window_state", {"hwnd": notepad["hwnd"], "action": "close"})
    finally:
        mcp.close()
        user32.PostMessageW(hwnd, 0x0010, 0, 0)
        time.sleep(1.5)
        try:
            dock.wait(timeout=5)
        except Exception:
            dock.kill()

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: borda de foco passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
