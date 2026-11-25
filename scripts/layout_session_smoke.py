"""Smoke do PreviewSession (md-d33.3.11) + divisórias (md-d33.3.2).

Valida: drag de divisória com commit atômico, undo (Ctrl+Alt+Z), cancel com
Esc (sem gravar), e resize de largura com commit/cancel/undo.
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
CONFIG = os.path.join(os.environ["APPDATA"], "MayerDock", "config.ini")
user32 = ctypes.windll.user32


class RECT(ctypes.Structure):
    _fields_ = [("left", wt.LONG), ("top", wt.LONG), ("right", wt.LONG), ("bottom", wt.LONG)]


class POINT(ctypes.Structure):
    _fields_ = [("x", wt.LONG), ("y", wt.LONG)]


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


def window_rect(hwnd):
    r = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


def lparam(x, y):
    return ((y & 0xFFFF) << 16) | (x & 0xFFFF)


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


def layout(mcp):
    _, text = mcp.tool("layout_info")
    return json.loads(text)


def config_text():
    try:
        return open(CONFIG, encoding="utf-8", errors="replace").read()
    except OSError:
        return ""


def main():
    dock = subprocess.Popen([EXE])
    time.sleep(2.5)
    hwnd = find_dock(dock.pid)
    if not hwnd:
        print("ERRO: dock não subiu")
        return 1
    wr = window_rect(hwnd)
    origin = POINT()
    user32.GetCursorPos(ctypes.byref(origin))
    errors = []
    mcp = Mcp()
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "LayoutTest", "version": "0"}})
        mcp.call("notifications/initialized")
        mcp.tool("open_app", {"path": "notepad.exe"})
        time.sleep(2.0)
        cli = subprocess.run([EXE, "--list-windows", "--json"], capture_output=True, text=True)
        np = next((w for w in json.loads(cli.stdout)["windows"]
                   if w.get("class", "").lower() == "notepad"), None)
        if np:
            resp, _ = mcp.tool("dock_pin", {"hwnd": np["hwnd"]})
            print("notepad encaixado:", "pinned" in _ or not resp["result"].get("isError"))
            time.sleep(0.8)

        base = layout(mcp)
        print("layout inicial:", {k: base[k] for k in ("split0", "split1", "width_logical")})
        if base["slots"] < 2:
            print("ERRO: precisa de 2 slots para testar divisórias")
            return 1
        content = base["content"]
        boundary = (base["slot_rects"][0]["bottom"] + base["slot_rects"][1]["top"]) // 2
        mid_x = (content["left"] + content["right"]) // 2
        original_split = base["split0"]
        original_width = base["width_logical"]
        config_before = config_text()

        def press_divider():
            user32.PostMessageW(hwnd, 0x0201, 1, lparam(mid_x, boundary))  # LBUTTONDOWN
            time.sleep(0.2)

        def move_to(client_y):
            user32.SetCursorPos(wr.left + mid_x, wr.top + client_y)
            user32.PostMessageW(hwnd, 0x0200, 0, lparam(mid_x, client_y))  # MOUSEMOVE
            time.sleep(0.25)

        # 1) commit do drag de divisória
        press_divider()
        move_to(boundary + 120)
        user32.PostMessageW(hwnd, 0x0202, 0, lparam(mid_x, boundary + 120))
        time.sleep(0.5)
        after_drag = layout(mcp)
        print("após drag:", after_drag["split0"], "| undo:", after_drag["undo"])
        if after_drag["split0"] <= original_split:
            errors.append("split0 deveria aumentar após arrastar para baixo")
        if not config_text().startswith(config_before.split("splits=")[0][:20]):
            pass
        if f"splits={after_drag['split0']}" not in config_text():
            errors.append("config.ini não gravou o split commitado")

        # 2) undo (Ctrl+Alt+Z)
        user32.PostMessageW(hwnd, 0x0312, 3, 0)  # WM_HOTKEY id 3
        time.sleep(0.5)
        after_undo = layout(mcp)
        print("após undo:", after_undo["split0"], "| redo:", after_undo.get("redo"))
        if after_undo["split0"] != original_split:
            errors.append("undo não restaurou o split original")

        # 3) cancel com Esc (arrasta e cancela)
        config_before_cancel = config_text()
        press_divider()
        move_to(boundary + 60)
        user32.PostMessageW(hwnd, 0x0100, 0x1B, 0)  # WM_KEYDOWN ESC
        user32.PostMessageW(hwnd, 0x0202, 0, lparam(10, 5))
        time.sleep(0.5)
        after_cancel = layout(mcp)
        print("após cancel:", after_cancel["split0"], "| editing:", after_cancel["editing"])
        if after_cancel["split0"] != original_split:
            errors.append("Esc não restaurou o split")
        if config_text() != config_before_cancel:
            errors.append("Esc cancelou mas gravou config")

        # 4) resize de largura: cancel com Esc
        user32.PostMessageW(hwnd, 0x0201, 1, lparam(2, 200))
        time.sleep(0.2)
        user32.SetCursorPos(wr.left + 60, wr.top + 200)
        user32.PostMessageW(hwnd, 0x0200, 0, lparam(60, 200))
        time.sleep(0.3)
        during_width = layout(mcp)
        user32.PostMessageW(hwnd, 0x0100, 0x1B, 0)
        user32.PostMessageW(hwnd, 0x0202, 0, lparam(10, 5))
        time.sleep(0.5)
        after_width_cancel = layout(mcp)
        print("width durante:", during_width["width_logical"], "| após Esc:", after_width_cancel["width_logical"])
        if after_width_cancel["width_logical"] != original_width:
            errors.append("Esc não restaurou a largura")

        # 5) resize commit + undo
        user32.PostMessageW(hwnd, 0x0201, 1, lparam(2, 200))
        time.sleep(0.2)
        user32.SetCursorPos(wr.left + 30, wr.top + 200)
        user32.PostMessageW(hwnd, 0x0200, 0, lparam(30, 200))
        time.sleep(0.3)
        user32.PostMessageW(hwnd, 0x0202, 0, lparam(10, 5))
        time.sleep(0.5)
        committed = layout(mcp)
        print("width commit:", committed["width_logical"], "| config contém:", f"width_logical={committed['width_logical']}" in config_text())
        if committed["width_logical"] == original_width:
            errors.append("resize comitado não mudou a largura")
        user32.PostMessageW(hwnd, 0x0312, 3, 0)
        time.sleep(0.5)
        undone = layout(mcp)
        print("width após undo:", undone["width_logical"])
        if undone["width_logical"] != original_width:
            errors.append("undo da largura não restaurou")

        if np:
            mcp.tool("dock_release_all")
            mcp.tool("window_state", {"hwnd": np["hwnd"], "action": "close"})
    finally:
        mcp.close()
        user32.SetCursorPos(origin.x, origin.y)
        user32.PostMessageW(hwnd, 0x0010, 0, 0)
        time.sleep(1.5)
        try:
            dock.wait(timeout=5)
        except Exception:
            dock.kill()

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: PreviewSession/divisórias passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
