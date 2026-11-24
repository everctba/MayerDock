"""Matriz de compatibilidade de captura/UIA (spike md-tkr).

Abre alguns apps representativos, mede para cada janela:
  - captura via PrintWindow (método padrão do capture_window)
  - captura via BitBlt (forçada)
  - nº de controles filhos (list_controls)
  - nº de elementos UIA (uia_query)
e gera docs/COMPAT.md.

Uso: python scripts/compat_matrix.py [--exe caminho]
"""

import argparse
import base64
import io
import json
import os
import subprocess
import sys
import time

from PIL import Image


def repo_root():
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def default_exe():
    root = repo_root()
    for candidate in (
        os.path.join(root, "dist", "MayerDock", "mayerdock.exe"),
        os.path.join(root, "target", "release", "mayerdock.exe"),
    ):
        if os.path.isfile(candidate):
            return candidate
    raise SystemExit("mayerdock.exe não encontrado (rode .\\build.ps1 antes)")


class Mcp:
    def __init__(self, exe):
        self.proc = subprocess.Popen(
            [exe, "--mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, encoding="utf-8", bufsize=1)
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


def text_of(resp):
    return "\n".join(i.get("text", "") for i in resp.get("result", {}).get("content", [])
                     if i.get("type") == "text")


def list_windows(mcp):
    rows = []
    for line in text_of(mcp.call("list_windows")).split("\r\n"):
        parts = line.split("\t")
        if len(parts) >= 3:
            try:
                hwnd = int(parts[0], 16)
            except ValueError:
                continue
            rows.append({"hwnd": hwnd, "class": parts[1], "title": parts[2]})
    return rows


def find_window(mcp, predicate):
    for row in list_windows(mcp):
        if predicate(row):
            return row
    return None


def capture_metrics(mcp, hwnd, method):
    resp = mcp.call("capture_window", {"hwnd": hwnd, "method": method})
    if resp.get("result", {}).get("isError"):
        return {"error": text_of(resp).strip()[:120]}
    content = resp["result"]["content"]
    text = "\n".join(i.get("text", "") for i in content if i.get("type") == "text")
    image = [i for i in content if i.get("type") == "image"]
    if not image:
        return {"error": "sem imagem"}
    png = base64.b64decode(image[0]["data"])
    img = Image.open(io.BytesIO(png)).convert("RGB")
    gray = img.convert("L")
    pixels = gray.tobytes()
    total = max(1, len(pixels))
    nonblack = sum(1 for value in pixels if value > 8) / total
    mean = sum(pixels) / total
    small = img.resize((64, 64))
    unique = len(set(small.tobytes()))
    used = "?"
    for line in text.split("\r\n"):
        if line.startswith("método:"):
            used = line.split(":", 1)[1].strip()
    return {
        "size": img.size,
        "nonblack": nonblack,
        "mean": mean,
        "unique": unique,
        "used": used,
        "bytes": len(png),
    }


def count_controls(mcp, hwnd):
    text = text_of(mcp.call("list_controls", {"hwnd": hwnd}))
    if "(sem controles" in text:
        return 0
    return sum(1 for line in text.split("\r\n") if line.startswith("0x"))


def count_uia(mcp, hwnd):
    try:
        resp = mcp.call("uia_query", {"hwnd": hwnd, "max": 120})
        text = text_of(resp)
        if resp.get("result", {}).get("isError") or not text:
            return -1
        return len(json.loads(text))
    except Exception:
        return -1


def fmt_capture(metrics):
    if "error" in metrics:
        return f"erro: {metrics['error']}"
    size = f"{metrics['size'][0]}x{metrics['size'][1]}"
    return (f"{size}, não-preto {metrics['nonblack'] * 100:.0f}%, "
            f"brilho {metrics['mean']:.0f}, cores {metrics['unique']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", default=default_exe())
    parser.add_argument("--out", default=os.path.join(repo_root(), "docs", "COMPAT.md"))
    args = parser.parse_args()

    mcp = Mcp(args.exe)
    rows = []
    try:
        mcp.send("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "CompatMatrix", "version": "0"}})
        mcp.send("notifications/initialized", notify=True)

        targets = [
            ("Notepad (Win32 clássico)", ["notepad.exe"],
             lambda w: w["class"].lower() == "notepad"),
            ("Calculadora (UWP/XAML)", ["calc.exe"],
             lambda w: w["class"] == "ApplicationFrameWindow" and "alcul" in w["title"]),
            ("Explorer (Win32 shell)", ["explorer.exe"],
             lambda w: w["class"] == "CabinetWClass"
             or "Explorer" in w["title"] or "Arquivos" in w["title"]),
            ("WordPad (Win32 + rich edit)", ["wordpad.exe"],
             lambda w: w["class"] == "WordPadClass"),
            ("Chrome (Chromium, já aberto)", [],
             lambda w: "Chrome_WidgetWin_1" in w["class"] and w["title"].strip() != ""),
        ]

        for name, cmd, predicate in targets:
            if cmd:
                mcp.call("open_app", {"path": cmd[0]})
                time.sleep(3.0)
            row = find_window(mcp, predicate)
            if not row:
                rows.append((name, None, "janela não encontrada", "-", "-", "-", "não abriu"))
                print(f"{name}: não encontrada")
                continue
            hwnd = row["hwnd"]
            pw = capture_metrics(mcp, hwnd, "printwindow")
            bb = capture_metrics(mcp, hwnd, "bitblt")
            controls = count_controls(mcp, hwnd)
            uia = count_uia(mcp, hwnd)
            note = []
            if "error" in pw:
                note.append("PrintWindow falhou")
            if "error" in bb:
                note.append("BitBlt falhou")
            if "error" not in pw and pw["nonblack"] < 0.02:
                note.append("PrintWindow preto")
            if "error" not in bb and bb["nonblack"] < 0.02:
                note.append("BitBlt preto")
            if uia == 0:
                note.append("sem UIA")
            if uia < 0:
                note.append("UIA falhou")
            rows.append((name, hwnd, fmt_capture(pw), fmt_capture(bb), controls, uia,
                         "; ".join(note) if note else "ok"))
            print(f"{name}: {rows[-1]}")
            if cmd:
                mcp.call("window_state", {"hwnd": hwnd, "action": "close"})
                time.sleep(0.8)
    finally:
        mcp.close()

    lines = [
        "# Matriz de compatibilidade — captura e UIA (spike md-tkr)",
        "",
        f"Gerado por `scripts/compat_matrix.py` em {time.strftime('%Y-%m-%d %H:%M')}.",
        "Ambiente: Windows 10 19041 (máquina de desenvolvimento); janelas abertas pelo próprio teste.",
        "",
        "Metodologia: para cada janela, `capture_window` em duas variantes (PrintWindow com",
        "`PW_RENDERFULLCONTENT` e BitBlt forçado), `list_controls` (EnumChildWindows) e",
        "`uia_query` (UI Automation, máximo 120 elementos). “Não-preto” = % de pixels com",
        "luminância > 8 (detecta frame preto). “Cores” = cores únicas numa miniatura 64×64.",
        "",
        "| App | HWND | PrintWindow | BitBlt | Controles filhos | Elementos UIA | Observações |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for name, hwnd, pw, bb, controls, uia, note in rows:
        hwnd_text = f"0x{hwnd:X}" if hwnd else "-"
        lines.append(f"| {name} | {hwnd_text} | {pw} | {bb} | {controls} | {uia} | {note} |")
    lines += [
        "",
        "## Notas",
        "",
        "- Chromium/Electron renderizam por GPU: PrintWindow pode devolver frame preto",
        "  (WGC é o caminho recomendado); BitBlt só pega o que está visível na tela.",
        "- UWP/XAML expõe árvore UIA rica (bom para `uia_invoke`); apps Win32 clássicos",
        "  variam (Notepad não expõe a menu bar como elemento UIA).",
        "- Calculadora: BitBlt devolve frame preto (superfície composta), PrintWindow ok;",
        "  é o caso que justifica WGC para apps GPU.",
        "- Para browser, a estratégia do MayerDock é `browser_launch` + **CDP**, não captura",
        "  de janela.",
        "- Este documento é um retrato do ambiente; medir de novo após o UIA nativo (COM).",
        "",
    ]
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(lines))
    print(f"\nmatriz escrita em {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
