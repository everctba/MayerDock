"""Smoke P3: badge na bandeja, tema automatico, confetti e launcher.

Valida via layout_info: tema (comparado ao registro), tray_tip com sessao,
favoritos carregados; confetti no fim de sessao com >=10 acoes (log + pixels
coloridos no cabecalho); launcher disparando "notepad" pelo menu.
"""
import ctypes
import ctypes.wintypes as wt
import json
import os
import subprocess
import sys
import time
import winreg

from PIL import ImageGrab

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
if not os.path.isfile(EXE):
    EXE = os.path.join(ROOT, "target", "release", "mayerdock.exe")
APPDATA = os.path.join(os.environ["APPDATA"], "MayerDock")
LOG = os.path.join(APPDATA, "mayerdock.log")
FAVORITES = os.path.join(APPDATA, "favorites.txt")
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


def notepad_pids():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq notepad.exe", "/FO", "CSV", "/NH"],
                         capture_output=True, text=True).stdout
    return {line.split(",")[1].strip('"') for line in out.splitlines() if "notepad" in line.lower()}


def colorful_pixels(image):
    count = 0
    for r, g, b in image.convert("RGB").getdata():
        if max(r, g, b) - min(r, g, b) > 70 and max(r, g, b) > 120:
            count += 1
    return count


def main():
    errors = []
    os.makedirs(APPDATA, exist_ok=True)
    had_favorites = os.path.exists(FAVORITES)
    with open(FAVORITES, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("# label|comando\nNotepad|notepad.exe\n")

    dock = subprocess.Popen([EXE])
    time.sleep(2.5)
    hwnd = find_dock(dock.pid)
    if not hwnd:
        print("ERRO: dock não subiu")
        return 1
    win = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(win))
    before_pids = notepad_pids()
    mcp = Mcp()
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "P3Test", "version": "0"}})
        mcp.call("notifications/initialized")
        base = layout(mcp)
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize") as key:
            light = winreg.QueryValueEx(key, "AppsUseLightTheme")[0]
        expected_theme = "light" if light else "dark"
        print("tema:", base["theme"], "| esperado:", expected_theme)
        if base["theme"] != expected_theme:
            errors.append("tema não acompanha o registro")
        if "Notepad" not in base["favorites"]:
            errors.append("favoritos não carregados")
        print("favoritos:", base["favorites"], "| tray:", base["tray_tip"].encode("ascii","backslashreplace").decode())
        if "●" not in base["tray_tip"]:
            errors.append("tray_tip sem badge de sessão")

        # 10+ ações para habilitar o confetti
        for _ in range(12):
            mcp.tool("layout_info")
        header_before = ImageGrab.grab(bbox=(win.left, win.top, win.right, win.top + 60))
        colorful_before = colorful_pixels(header_before)
        mcp.close()  # fecha stdin -> session_bye -> finish (confetti)
        time.sleep(0.15)
        header_after = ImageGrab.grab(bbox=(win.left, win.top, win.right, win.top + 60))
        colorful_after = colorful_pixels(header_after)
        print("confetti pixels:", colorful_before, "->", colorful_after)
        if colorful_after <= colorful_before:
            errors.append("confetti não apareceu no cabeçalho")
        header_after.save(os.path.join(ROOT, "target", "p3-confetti.png"))
        if "confetti:" not in open(LOG, encoding="utf-8", errors="replace").read():
            errors.append("log sem evento de confetti")

        # launcher via menu: WM_MENU_CHOICE (0x8000+41) cmd 200 (favorito 0)
        user32.PostMessageW(hwnd, 0x8000 + 41, 200, 0)
        time.sleep(2.0)
        after_pids = notepad_pids()
        launched = after_pids - before_pids
        print("launcher abriu notepad:", bool(launched), launched)
        if not launched:
            errors.append("launcher não abriu o favorito")

        # badge/tray: o fim da sessao foi registrado no log
        if "agent: sessão encerrada (session_bye)" not in open(LOG, encoding="utf-8", errors="replace").read():
            errors.append("log sem sessao encerrada (session_bye)")
    finally:
        try:
            mcp.close()
        except Exception:
            pass
        user32.PostMessageW(hwnd, 0x0010, 0, 0)
        time.sleep(1.5)
        try:
            dock.wait(timeout=5)
        except Exception:
            dock.kill()
        for pid in notepad_pids() - before_pids:
            subprocess.run(["taskkill", "/PID", pid, "/F"], capture_output=True)
        if not had_favorites:
            try:
                os.remove(FAVORITES)
            except OSError:
                pass

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: P3 passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
