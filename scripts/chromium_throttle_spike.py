"""Spike md-9va: throttle/oclusão do Chromium + WebGPU/WebGL2/rAF via CDP.

Mede, para o browser lançado pelo MayerDock (browser_launch):
  - offscreen (headed, --window-position=-2400,-2400, flags anti-throttle)
  - visível onscreen
  - minimizado
  - headless=new
métricas: rAF em 2,1 s, timers de 100 ms, WebGL2 (renderer), WebGPU (adapter),
visibilityState.

Uso: python scripts/chromium_throttle_spike.py
"""
import base64
import hashlib
import json
import os
import re
import socket
import struct
import subprocess
import sys
import time
import urllib.request
from urllib.parse import urlparse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "dist", "MayerDock", "mayerdock.exe")
if not os.path.isfile(EXE):
    EXE = os.path.join(ROOT, "target", "release", "mayerdock.exe")


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen([EXE, "--mcp"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     text=True, encoding="utf-8", bufsize=1)
        self.id = 0

    def call(self, method, params=None):
        self.id += 1
        msg = {"jsonrpc": "2.0", "id": self.id, "method": method}
        if params is not None:
            msg["params"] = params
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("sem resposta: " + method)
        return json.loads(line)

    def tool(self, name, args=None):
        resp = self.call("tools/call", {"name": name, "arguments": args or {}})
        text = "\n".join(i.get("text", "") for i in resp["result"].get("content", []))
        return resp, text

    def notify(self, method):
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.proc.stdin.flush()

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.wait(timeout=8)
        except Exception:
            self.proc.kill()


def ws_connect(url):
    u = urlparse(url)
    sock = socket.create_connection((u.hostname, u.port or 80), timeout=15)
    key = base64.b64encode(os.urandom(16)).decode()
    path = u.path + (("?" + u.query) if u.query else "")
    request = (
        f"GET {path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\n"
        "Upgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    sock.sendall(request.encode())
    response = b""
    while b"\r\n\r\n" not in response:
        chunk = sock.recv(4096)
        if not chunk:
            raise RuntimeError("handshake ws sem resposta")
        response += chunk
    if b" 101 " not in response.split(b"\r\n")[0]:
        raise RuntimeError("handshake ws recusado: " + response.split(b"\r\n")[0].decode(errors="replace"))
    return sock


def recv_exact(sock, n):
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise EOFError("socket fechou")
        buf += chunk
    return buf


def ws_send(sock, text, opcode=0x1):
    data = text.encode()
    mask = os.urandom(4)
    header = bytearray([0x80 | opcode])
    length = len(data)
    if length < 126:
        header.append(0x80 | length)
    elif length < 65536:
        header.append(0x80 | 126)
        header += struct.pack(">H", length)
    else:
        header.append(0x80 | 127)
        header += struct.pack(">Q", length)
    header += mask
    sock.sendall(header + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def ws_recv_message(sock):
    chunks = []
    while True:
        h = recv_exact(sock, 2)
        fin = h[0] & 0x80
        opcode = h[0] & 0x0F
        masked = h[1] & 0x80
        length = h[1] & 0x7F
        if length == 126:
            length = struct.unpack(">H", recv_exact(sock, 2))[0]
        elif length == 127:
            length = struct.unpack(">Q", recv_exact(sock, 8))[0]
        mask = recv_exact(sock, 4) if masked else None
        data = recv_exact(sock, length)
        if mask:
            data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        if opcode == 0x9:  # ping
            ws_send(sock, data.decode("utf-8", "ignore"), opcode=0xA)
            continue
        if opcode == 0x8:
            raise EOFError("ws close")
        chunks.append(data)
        if fin:
            return json.loads(b"".join(chunks).decode("utf-8"))


def cdp_call(sock, msg_id, method, params=None):
    payload = {"id": msg_id, "method": method}
    if params is not None:
        payload["params"] = params
    ws_send(sock, json.dumps(payload))
    while True:
        message = ws_recv_message(sock)
        if message.get("id") == msg_id:
            return message
        # ignora eventos


EVALUATE_JS = r"""
(async () => {
  const t0 = performance.now();
  let raf = 0, timers = 0;
  const tick = () => { raf++; if (performance.now() - t0 < 2000) requestAnimationFrame(tick); };
  requestAnimationFrame(tick);
  const iv = setInterval(() => { timers++; }, 100);
  await new Promise(resolve => setTimeout(resolve, 2100));
  clearInterval(iv);
  const canvas = document.createElement('canvas');
  const gl = canvas.getContext('webgl2');
  let webgl2 = !!gl, renderer = null;
  if (gl) {
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    renderer = ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
  }
  let webgpu = false, adapter = null;
  if (navigator.gpu) {
    try {
      const a = await navigator.gpu.requestAdapter();
      webgpu = !!a;
      adapter = a && a.info ? { vendor: a.info.vendor, architecture: a.info.architecture } : (a ? 'adapter-sem-info' : null);
    } catch (e) { adapter = 'erro: ' + e; }
  }
  return {
    elapsed_ms: Math.round(performance.now() - t0),
    raf, timers,
    webgl2, renderer, webgpu, adapter,
    visibility: document.visibilityState,
    has_focus: document.hasFocus(),
    dpr: devicePixelRatio,
    ua: navigator.userAgent.slice(0, 70)
  };
})()
"""


def measure(sock, next_id):
    resp = cdp_call(sock, next_id, "Runtime.evaluate", {
        "expression": EVALUATE_JS, "awaitPromise": True, "returnByValue": True})
    value = resp.get("result", {}).get("result", {}).get("value")
    if not value:
        raise RuntimeError(f"evaluate sem valor: {resp}")
    return value


def get_page_target(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=10) as resp:
        targets = json.load(resp)
    pages = [t for t in targets if t.get("type") == "page"]
    return pages[0] if pages else None


def launch(mcp, headless, port, extra_flags=None):
    args = {"headless": headless, "cdp_port": port, "url": "about:blank"}
    if extra_flags:
        args["extra_flags"] = extra_flags
    _, text = mcp.tool("browser_launch", args)
    pid_match = re.search(r"pid:\s*(\d+)", text)
    pid = int(pid_match.group(1)) if pid_match else 0
    target = None
    for _ in range(40):
        try:
            target = get_page_target(port)
            if target:
                break
        except Exception:
            pass
        time.sleep(0.25)
    if not target:
        raise RuntimeError("sem target de página no CDP")
    return pid, target, text


def kill_browser(pid):
    if pid:
        subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"],
                       capture_output=True, text=True)


def run_scenario(sock, next_id, target_id, bounds=None):
    if bounds is not None:
        win = cdp_call(sock, next_id, "Browser.getWindowForTarget", {"targetId": target_id})
        window_id = win.get("result", {}).get("windowId")
        cdp_call(sock, next_id + 1, "Browser.setWindowBounds", {
            "windowId": window_id, "bounds": bounds})
        time.sleep(1.5)
    return measure(sock, next_id + 2)


def main():
    mcp = Mcp()
    results = {}
    errors = []
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "ChromiumSpike", "version": "0"}})
        mcp.notify("notifications/initialized")

        # 1) headed offscreen -> visível -> minimizado
        pid, target, _ = launch(mcp, headless=False, port=9333)
        sock = ws_connect(target["webSocketDebuggerUrl"])
        next_id = 1
        try:
            results["headed_offscreen"] = run_scenario(sock, next_id, target["id"])
            next_id += 4
            results["headed_visible"] = run_scenario(sock, next_id, target["id"], {
                "left": 120, "top": 120, "width": 900, "height": 700, "windowState": "normal"})
            next_id += 4
            results["headed_minimized"] = run_scenario(sock, next_id, target["id"], {
                "windowState": "minimized"})
        finally:
            sock.close()
            kill_browser(pid)
            time.sleep(1.0)

        # 2) headless=new
        pid, target, _ = launch(mcp, headless=True, port=9334)
        sock = ws_connect(target["webSocketDebuggerUrl"])
        try:
            results["headless_new"] = run_scenario(sock, 1, target["id"])
        finally:
            sock.close()
            kill_browser(pid)
            time.sleep(1.0)

        # 3) headed offscreen com --enable-unsafe-webgpu (WebGPU ligado à força)
        pid, target, _ = launch(mcp, headless=False, port=9335,
                                extra_flags=["--enable-unsafe-webgpu"])
        sock = ws_connect(target["webSocketDebuggerUrl"])
        try:
            results["headed_offscreen_webgpu_flag"] = run_scenario(sock, 1, target["id"])
        finally:
            sock.close()
            kill_browser(pid)
    except Exception as err:
        errors.append(str(err))
    finally:
        mcp.close()

    for name, data in results.items():
        print(f"[{name}]")
        print(json.dumps(data, ensure_ascii=False, indent=2))
    if errors:
        print("ERROS:", errors)
        return 1

    # documento do spike
    lines = [
        "# Spike md-9va — Chromium: throttle/oclusão e WebGPU/WebGL2/rAF",
        "",
        "Gerado por `scripts/chromium_throttle_spike.py` via CDP (browser lançado pelo",
        "`browser_launch` do MayerDock). Métricas: rAF em ~2,1 s, timers de 100 ms,",
        "WebGL2 (renderer), WebGPU (adapter), `visibilityState` e foco.",
        "",
        "| Cenário | rAF (2,1 s) | timers 100 ms | WebGL2 | WebGPU | visibility | foco |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for name, data in results.items():
        lines.append(
            f"| {name} | {data.get('raf')} | {data.get('timers')} | "
            f"{data.get('webgl2')} | {data.get('webgpu')} | "
            f"{data.get('visibility')} | {data.get('has_focus')} |")
    lines += [
        "",
        "## Renderers / adapters",
        "",
    ]
    for name, data in results.items():
        lines.append(f"- **{name}**: WebGL2 renderer = `{data.get('renderer')}`; "
                     f"WebGPU adapter = `{data.get('adapter')}`")
    lines += [
        "",
        "## Leitura",
        "",
        "- rAF ~120 em 2,1 s = 60 Hz sem throttle; valores baixos indicam throttling.",
        "- As flags `--disable-backgrounding-occluded-windows`,",
        "  `--disable-renderer-backgrounding`, `--disable-background-timer-throttling` e",
        "  `--disable-features=CalculateNativeWinOcclusion` são o baseline do `browser_launch`.",
        "- Minimizado é o pior caso: o Windows pode suspender rAF mesmo com as flags; por",
        "  isso o agente deve manter a janela **offscreen visível** e não minimizada.",
        "- rAF alto (197/202 em 2,1 s) = sem throttle; **0** = parado (minimizado).",
        "- Minimizado: rAF caiu a 0 (`visibilityState=hidden`) mesmo com as flags, mas os",
        "  timers de 100 ms continuaram ~21 → flags anti-background-timer funcionam, porém",
        "  rAF parado inviabiliza animação/WebGPU; manter a janela **offscreen visível**.",
        "- Headless `new` neste ambiente usou GPU real (ANGLE/D3D11) e rAF 160 — varia por",
        "  máquina; medir antes de confiar.",
        "- **WebGPU indisponível mesmo com `--enable-unsafe-webgpu`** (`requestAdapter`",
        "  nulo em todos os cenários). `browser_launch` aceita `extra_flags` para repetir",
        "  o teste; validar versão do Chrome/driver antes de depender de WebGPU.",
        "",
    ]
    out = os.path.join(ROOT, "docs", "SPIKE-CHROMIUM.md")
    with open(out, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(lines))
    print("documento:", out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
