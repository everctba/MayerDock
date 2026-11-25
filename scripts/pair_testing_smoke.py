"""Smoke do pair testing (md-d33.3.7): HUD "quem está no controle" + Assumir.

Fluxo: dock + sidecar -> agente captura OK -> clica em ✋ (Assumir) -> tools
pausadas -> clica em ▶ (Devolver) -> captura volta a funcionar.
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


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen([EXE, "--mcp"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     text=True, encoding="utf-8", bufsize=1)
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
        line = self.proc.stdout.readline() if not notify else ""
        if not line and not notify:
            raise RuntimeError("sem resposta: " + method)
        return json.loads(line) if line else None

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


def main():
    dock = subprocess.Popen([EXE])
    time.sleep(2.5)
    hwnd = find_dock(dock.pid)
    if not hwnd:
        print("ERRO: dock não subiu")
        return 1
    client = RECT()
    user32.GetClientRect(hwnd, ctypes.byref(client))
    width = client.right
    control_x = width - 158
    errors = []

    mcp = Mcp()
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "PairTest", "version": "0"}})
        mcp.call("notifications/initialized", notify=True)
        mcp.tool("open_app", {"path": "notepad.exe"})
        time.sleep(2.0)
        cli = subprocess.run([EXE, "--list-windows", "--json"], capture_output=True, text=True)
        notepad = next((w for w in json.loads(cli.stdout)["windows"]
                        if w.get("class", "").lower() == "notepad"), None)
        if not notepad:
            print("ERRO: notepad não achado")
            return 1

        first = mcp.tool("capture_window", {"hwnd": notepad["hwnd"]})
        print("antes: captura ok =", not first["result"].get("isError"))
        if first["result"].get("isError"):
            errors.append("captura deveria funcionar antes do Assumir")

        def click_control():
            lparam = (20 << 16) | (control_x & 0xFFFF)
            user32.PostMessageW(hwnd, 0x0202, 0, lparam)  # WM_LBUTTONUP
            time.sleep(1.2)

        click_control()  # ✋ assumir
        status = mcp.tool("control_status")
        print("controle:", text_of(status).strip())
        paused_try = mcp.tool("capture_window", {"hwnd": notepad["hwnd"]})
        paused_text = text_of(paused_try)
        print("captura pausada =", paused_try["result"].get("isError"), "|", paused_text.strip()[:60])
        if not paused_try["result"].get("isError") or "pausado" not in paused_text:
            errors.append("captura deveria ser negada com o humano no controle")
        if "humano" not in text_of(status):
            errors.append("control_status deveria dizer humano")

        # screenshot do HUD com "você no controle" (evidência/demo)
        win = RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(win))
        shot = ImageGrab.grab(bbox=(win.left, win.top, win.right, win.top + 60))
        shot.save(os.path.join(ROOT, "target", "pair-hud.png"))

        click_control()  # ▶ devolver
        status2 = mcp.tool("control_status")
        resumed = mcp.tool("capture_window", {"hwnd": notepad["hwnd"]})
        print("após devolver:", text_of(status2).strip(), "| captura ok =", not resumed["result"].get("isError") is False)
        if "agente" not in text_of(status2):
            errors.append("control_status deveria voltar a agente")
        if resumed["result"].get("isError"):
            errors.append("captura deveria voltar a funcionar")

        mcp.tool("window_state", {"hwnd": notepad["hwnd"], "action": "close"})
    finally:
        mcp.close()
        user32.PostMessageW(hwnd, 0x0010, 0, 0)  # WM_CLOSE
        time.sleep(1.5)
        try:
            dock.wait(timeout=5)
        except Exception:
            dock.kill()

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: pair testing passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
