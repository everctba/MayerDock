"""Smoke de workspaces (md-d33.3.1) + draft/approval (md-jcf.4).

Salva workspace com Notepad encaixado, libera tudo, restaura (layout + app
reaberto/encaixado), cria rascunho e aplica via workspace_apply_draft.
Aprovação automática no dock via MAYERDOCK_APPROVE_AUTO=1 (QA).
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
APPDATA = os.path.join(os.environ["APPDATA"], "MayerDock")
WORKSPACES = os.path.join(APPDATA, "workspaces.json")
DRAFTS = os.path.join(APPDATA, "drafts")


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


def slots(mcp):
    _, text = mcp.tool("list_slots")
    return json.loads(text)


def notepad_pids():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq notepad.exe", "/FO", "CSV", "/NH"],
                         capture_output=True, text=True).stdout
    return {line.split(",")[1].strip('"') for line in out.splitlines() if "notepad" in line.lower()}


def main():
    errors = []
    backup = None
    if os.path.exists(WORKSPACES):
        backup = open(WORKSPACES, encoding="utf-8").read()
    before_pids = notepad_pids()
    env = dict(os.environ, MAYERDOCK_APPROVE_AUTO="1")
    dock = subprocess.Popen([EXE], env=env)
    time.sleep(2.5)
    mcp = Mcp()
    draft_path = None
    try:
        mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "WorkspaceTest", "version": "0"}})
        mcp.call("notifications/initialized")
        mcp.tool("open_app", {"path": "notepad.exe"})
        time.sleep(2.0)
        cli = subprocess.run([EXE, "--list-windows", "--json"], capture_output=True, text=True)
        np = next((w for w in json.loads(cli.stdout)["windows"]
                   if w.get("class", "").lower() == "notepad"), None)
        if not np:
            print("ERRO: notepad não abriu")
            return 1
        mcp.tool("dock_pin", {"hwnd": np["hwnd"]})
        time.sleep(0.8)

        resp, text = mcp.tool("workspace_save", {"name": "smoke"})
        saved = json.loads(open(WORKSPACES, encoding="utf-8").read())
        ws = saved["workspaces"]["smoke"]
        print("workspace salvo:", {k: ws[k] for k in ("width", "split0", "split1")},
              "| slots:", ws["slots"])
        if not any(slot and "notepad" in slot.get("exe", "").lower() for slot in ws["slots"]):
            errors.append("workspace não salvou o exe do Notepad")
        _, listed = mcp.tool("workspace_list")
        if "smoke" not in listed:
            errors.append("workspace_list não mostrou 'smoke'")

        # libera tudo e fecha o notepad; restaura
        mcp.tool("dock_release_all")
        mcp.tool("window_state", {"hwnd": np["hwnd"], "action": "close"})
        time.sleep(1.0)
        _, restored = mcp.tool("workspace_restore", {"name": "smoke"})
        time.sleep(2.0)
        print("restore:", restored.strip()[:100])
        after = slots(mcp)
        occupied = [s for s in after if s.get("occupied")]
        if not occupied:
            errors.append("restore não reencaixou nenhum app")
        else:
            print("slot restaurado:", occupied[0].get("title"), "| exe:", occupied[0].get("exe"))
            if "notepad" not in occupied[0].get("exe", "").lower():
                errors.append("slot restaurado não é o Notepad")

        # draft + apply
        _, draft_text = mcp.tool("workspace_draft", {"name": "smoke-draft"})
        for line in draft_text.splitlines():
            if line.startswith("rascunho criado:"):
                draft_path = line.split(":", 1)[1].strip()
                break
        if not draft_path or not os.path.exists(draft_path):
            errors.append("workspace_draft não criou arquivo")
        else:
            print("draft:", draft_path)
            _, applied = mcp.tool("workspace_apply_draft", {"path": draft_path})
            print("apply:", applied.strip()[:100])
            time.sleep(2.0)
            if not any(s.get("occupied") for s in slots(mcp)):
                errors.append("apply_draft não reencaixou app")
    finally:
        try:
            mcp.close()
        except Exception:
            pass
        for pid in notepad_pids() - before_pids:
            subprocess.run(["taskkill", "/PID", pid, "/F"], capture_output=True)
        if draft_path and os.path.exists(draft_path):
            os.remove(draft_path)
        if backup is not None:
            open(WORKSPACES, "w", encoding="utf-8").write(backup)
        else:
            try:
                os.remove(WORKSPACES)
            except OSError:
                pass
        subprocess.run(["taskkill", "/IM", "mayerdock.exe", "/F"], capture_output=True)
        time.sleep(1.0)

    if errors:
        print("FALHAS:", errors)
        return 1
    print("OK: workspaces/draft passou")
    return 0


if __name__ == "__main__":
    sys.exit(main())
