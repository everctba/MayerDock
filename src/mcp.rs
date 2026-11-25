//! Sidecar **MCP** (Model Context Protocol) do MayerDock — v1.5.
//!
//! Modo de uso: `mayerdock.exe --mcp` (servidor MCP sobre stdio, JSON-RPC 2.0,
//! uma mensagem JSON por linha).
//!
//! Duas camadas:
//! - **standalone**: inventário de janelas, captura de janela, abertura de app e
//!   estado de janela (funciona sem o dock);
//! - **dock** (named pipe `\\.\pipe\MayerDock.Agent` + token): `list_slots`,
//!   `dock_pin`, `dock_unpin`, `dock_release_all`, sessão/HUD no dock.
//!
//! Regras do guia embutido (entregue ao agente em `initialize`):
//! - preferir dados estruturados (árvore/lista) a pixels;
//! - capturar só a janela alvo, nunca a tela inteira;
//! - sem input global nesta fatia (UIA/CDP entram na próxima).

#![allow(static_mut_refs)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
    GetWindowDC, ReleaseDC, ScreenToClient, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, mouse_event, IsWindowEnabled, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetDlgCtrlID, GetWindowLongW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindow,
    IsWindowVisible, PostMessageW, SendMessageW, SetCursorPos, ShowWindow, BM_CLICK, GWL_EXSTYLE,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNOACTIVATE, WM_CHAR, WM_GETTEXT, WM_GETTEXTLENGTH,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SETTEXT, WS_EX_TOOLWINDOW,
};

/// `MK_LBUTTON` (windows-sys não expõe direto no módulo de mensagens).
const MK_LBUTTON: usize = 1;

/// Contadores exibidos no HUD (cliques e capturas desta sessão).
static mut CLICKS: u32 = 0;
static mut CAPTURES: u32 = 0;

/// `PW_RENDERFULLCONTENT` (windows-sys não expõe a constante).
const PW_RENDERFULLCONTENT: u32 = 2;

/// Limite de linha do MCP stdio (proteção contra payloads gigantes).
const MAX_MCP_LINE: usize = 4 * 1024 * 1024;

/// Lê uma linha com limite de tamanho.
///
/// `Ok(None)` = EOF; `Ok(Some(vazio))` = linha oversized descartada (o resto
/// foi consumido até o `\n`, sem crescer memória).
fn read_line_capped<R: BufRead>(reader: &mut R, cap: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        let (data, consumed, newline) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(if buffer.is_empty() {
                    None
                } else {
                    Some(buffer)
                });
            }
            match chunk.iter().position(|&b| b == b'\n') {
                Some(pos) => (chunk[..pos].to_vec(), pos + 1, true),
                None => (chunk.to_vec(), chunk.len(), false),
            }
        };
        buffer.extend_from_slice(&data);
        reader.consume(consumed);
        if newline {
            if buffer.len() > cap {
                return Ok(Some(Vec::new()));
            }
            return Ok(Some(buffer));
        }
        if buffer.len() > cap {
            // Consome o resto da linha oversized sem guardar o conteúdo.
            loop {
                let (consumed, newline) = {
                    let chunk = reader.fill_buf()?;
                    if chunk.is_empty() {
                        return Ok(None);
                    }
                    match chunk.iter().position(|&b| b == b'\n') {
                        Some(pos) => (pos + 1, true),
                        None => (chunk.len(), false),
                    }
                };
                reader.consume(consumed);
                if newline {
                    return Ok(Some(Vec::new()));
                }
            }
        }
    }
}

fn parse_message_line(line: &str) -> Result<Value, String> {
    serde_json::from_str(line).map_err(|err| err.to_string())
}

pub fn serve(version: &str) -> i32 {
    crate::win::log("mcp: sidecar iniciado (stdio)");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut initialized = false;
    let mut reader = stdin.lock();

    loop {
        let raw = match read_line_capped(&mut reader, MAX_MCP_LINE) {
            Ok(Some(raw)) => raw,
            Ok(None) => break,
            Err(_) => break,
        };
        if raw.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&raw);
        let line = text.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match parse_message_line(line) {
            Ok(value) => value,
            Err(err) => {
                crate::win::log(&format!("mcp: JSON inválido: {err}"));
                continue;
            }
        };
        if message.get("id").is_none() {
            // Notificação (ex.: notifications/initialized): não responde.
            if message.get("method").and_then(Value::as_str) == Some("notifications/initialized") {
                initialized = true;
            }
            continue;
        }
        let response = handle(&message, version, &mut initialized);
        if let Ok(text) = serde_json::to_string(&response) {
            let _ = writeln!(stdout, "{text}");
            let _ = stdout.flush();
        }
    }
    dock_bye();
    crate::win::log("mcp: sidecar encerrado");
    0
}

// ---------------------------------------------------------------------------
// Cliente do dock (named pipe + token + sessão)
// ---------------------------------------------------------------------------

struct Dock {
    writer: File,
    reader: BufReader<File>,
    next_id: u64,
}

static mut DOCK: Option<Dock> = None;

impl Dock {
    fn connect() -> Result<Dock, String> {
        let token_path = crate::config::data_dir().join("agent-token");
        let token = std::fs::read_to_string(&token_path).map_err(|_| {
            "dock não está rodando (agent-token ausente); abra o MayerDock".to_string()
        })?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\pipe\MayerDock.Agent")
            .map_err(|err| format!("não conectei no pipe do dock: {err}"))?;
        let reader = BufReader::new(
            file.try_clone()
                .map_err(|err| format!("pipe clone: {err}"))?,
        );
        let mut dock = Dock {
            writer: file,
            reader,
            next_id: 0,
        };
        let auth = dock.call("auth", json!({ "token": token.trim() }))?;
        if auth.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err("token do dock inválido".to_string());
        }
        crate::win::log("mcp: conectado ao dock (pipe autenticado)");
        Ok(dock)
    }

    fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut line = serde_json::to_string(&json!({ "id": id, "tool": tool, "args": args }))
            .map_err(|err| err.to_string())?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|err| format!("pipe write: {err}"))?;
        self.writer
            .flush()
            .map_err(|err| format!("pipe flush: {err}"))?;

        let mut buffer = String::new();
        let read = match self.reader.read_line(&mut buffer) {
            Ok(read) => read,
            // Pipe quebrado = dock fechou/Parar (kill switch): encerra o sidecar,
            // o harness vê o servidor MCP cair e o agente para.
            Err(err) => {
                crate::win::log(&format!(
                    "mcp: pipe do dock fechou ({err}) — encerrando sidecar (kill switch?)"
                ));
                std::process::exit(0);
            }
        };
        if read == 0 {
            crate::win::log("mcp: dock fechou a conexão (kill switch?) — encerrando sidecar");
            std::process::exit(0);
        }
        let response: Value = serde_json::from_str(buffer.trim())
            .map_err(|err| format!("pipe JSON inválido: {err}"))?;
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(response.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("erro do dock")
                .to_string())
        }
    }
}

fn dock() -> Result<&'static mut Dock, String> {
    unsafe {
        if DOCK.is_none() {
            DOCK = Some(Dock::connect()?);
        }
        Ok(DOCK.as_mut().expect("dock conectado"))
    }
}

fn dock_session(name: &str) {
    if let Ok(dock) = dock() {
        let _ = dock.call(
            "session_hello",
            json!({ "name": name, "pid": std::process::id() }),
        );
    }
}

fn dock_bye() {
    unsafe {
        if let Some(dock) = DOCK.as_mut() {
            let _ = dock.call("session_bye", json!({}));
        }
    }
}

/// Executa uma tool no dock (erro legível quando o dock não está aberto).
fn dock_tool(tool: &str, args: Value) -> Result<Value, String> {
    dock()?.call(tool, args)
}

/// Atualiza a atividade no HUD do dock, se houver conexão (best-effort).
fn dock_activity_if_connected(text: &str) {
    unsafe {
        if let Some(dock) = DOCK.as_mut() {
            let _ = dock.call(
                "activity",
                json!({ "text": text, "clicks": CLICKS, "captures": CAPTURES }),
            );
        }
    }
}

fn handle(message: &Value, version: &str, _initialized: &mut bool) -> Value {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2024-11-05");
            let protocol = match requested {
                "2024-11-05" | "2025-03-26" | "2025-06-18" => requested,
                _ => "2024-11-05",
            };
            // Sessão para o HUD do dock (silencioso se o dock não estiver aberto).
            let client_name = params
                .pointer("/clientInfo/name")
                .and_then(Value::as_str)
                .unwrap_or("agente")
                .to_string();
            dock_session(&client_name);
            result(
                id,
                json!({
                    "protocolVersion": protocol,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mayerdock-mcp", "version": version },
                    "instructions": "MayerDock agent workspace. Prefira dados estruturados \
                        (list_windows/list_slots) a screenshots. Capture somente a janela \
                        alvo. Use list_slots/dock_pin para encaixar janelas no dock sem \
                        roubar foco. Não há input global nesta versão (UIA/CDP na próxima); \
                        se o dock estiver aberto, a sessão aparece no HUD e o botão Parar \
                        encerra este sidecar."
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(name, &args) {
                Ok(content) => result(id, json!({ "content": content, "isError": false })),
                Err(err) => result(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": err }],
                        "isError": true
                    }),
                ),
            }
        }
        _ => error(id, -32601, &format!("método desconhecido: {method}")),
    }
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Value {
    json!([
        {
            "name": "status",
            "description": "Estado do sidecar e resumo do ambiente (versão, PID, janelas).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "list_windows",
            "description": "Lista janelas visíveis com título, classe, PID, retângulo e DPI \
                (equivalente a list_windows do MayerDock; nesta fatia sem slots).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "capture_window",
            "description": "Captura UMA janela (por handle ou trecho do título) em PNG, sem \
                mover o cursor e sem capturar a tela inteira. Retorna caminho do arquivo, \
                dimensões, DPI e a imagem.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer", "description": "Handle da janela" },
                    "title": { "type": "string", "description": "Trecho do título (case-insensitive)" },
                    "method": {
                        "type": "string",
                        "enum": ["auto", "printwindow", "bitblt"],
                        "description": "Força o método de captura (spike de compatibilidade)"
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "open_app",
            "description": "Abre um executável/arquivo (ShellExecute). Nesta fatia abre \
                destacado; encaixar em slot do dock entra na próxima etapa.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "args": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        },
        {
            "name": "window_state",
            "description": "Maximiza, minimiza, restaura ou fecha uma janela (sem ativá-la).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "action": { "type": "string", "enum": ["maximize", "minimize", "restore", "close"] }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_slots",
            "description": "Lista os slots do dock (índice, ocupado, hwnd, título, retângulo). \
                Requer o MayerDock aberto.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "dock_pin",
            "description": "Encaixa uma janela existente no primeiro slot livre do dock, sem \
                ativá-la. Requer hwnd de list_windows.",
            "inputSchema": {
                "type": "object",
                "properties": { "hwnd": { "type": "integer" } },
                "required": ["hwnd"],
                "additionalProperties": false
            }
        },
        {
            "name": "dock_unpin",
            "description": "Libera a janela de um slot (volta ao lugar/tamanho original).",
            "inputSchema": {
                "type": "object",
                "properties": { "hwnd": { "type": "integer" } },
                "required": ["hwnd"],
                "additionalProperties": false
            }
        },
        {
            "name": "dock_release_all",
            "description": "Libera todas as janelas encaixadas no dock.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "list_controls",
            "description": "Lista os controles (janelas filhas) de uma janela: hwnd, classe, \
                texto, id, enabled e retângulo. Base estruturada para clicar/digitar sem pixels.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "click",
            "description": "Clique direcionado (PostMessage, sem mover o cursor real) em um \
                hwnd. Coordenadas de cliente por padrão; use coords=screen para tela.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "coords": { "type": "string", "enum": ["client", "screen"] }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "type_text",
            "description": "Digita texto em um controle/janela via WM_CHAR direcionado (sem \
                usar o teclado global).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "send_key",
            "description": "Envia uma tecla (WM_KEYDOWN/WM_KEYUP) para a janela, por nome \
                (enter, tab, esc, space, backspace, delete, up/down/left/right) ou código VK.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "key": { "type": ["string", "integer"] }
                },
                "required": ["key"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_text",
            "description": "Lê o texto de uma janela/controle (WM_GETTEXT, fallback título).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "wait_for",
            "description": "Espera uma condição na janela (título contém, texto contém, \
                visível) com timeout; evita polling cego do agente.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "title_contains": { "type": "string" },
                    "text_contains": { "type": "string" },
                    "visible": { "type": "boolean" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "focus_lease",
            "description": "Pede ao usuário (modal no dock) permissão para controlar mouse e \
                teclado por N segundos. Sem lease aprovado, as tools global_* falham.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "seconds": { "type": "integer" },
                    "reason": { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "global_click",
            "description": "Clique global (SendInput/mouse real) — EXIGE focus lease aprovado.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "type": "integer" }
                },
                "required": ["x", "y"],
                "additionalProperties": false
            }
        },
        {
            "name": "global_type",
            "description": "Digitação global (SendInput/unicode, teclado real) — EXIGE focus lease.",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "global_key",
            "description": "Tecla global (SendInput, teclado real) — EXIGE focus lease.",
            "inputSchema": {
                "type": "object",
                "properties": { "key": { "type": ["string", "integer"] } },
                "required": ["key"],
                "additionalProperties": false
            }
        },
        {
            "name": "set_text",
            "description": "Define o texto de um controle/janela via WM_SETTEXT (sem teclado \
                global). Ideal para Edit/RichEdit/combos.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "control_click",
            "description": "Aciona um controle por mensagem: BM_CLICK para botões e fallback \
                de clique no centro do controle (PostMessage). Sem cursor global.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_launch",
            "description": "Sobe Chrome/Edge com perfil dedicado, flags anti-throttle \
                (CalculateNativeWinOcclusion etc.) e --remote-debugging-port; devolve o \
                endpoint CDP para o agente conectar (Playwright/CDP). headless=false mantém \
                GPU real (WebGPU/WebGL2).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "headless": { "type": "boolean" },
                    "cdp_port": { "type": "integer" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "uia_query",
            "description": "Árvore de UI Automation de uma janela (nome, tipo, AutomationId, \
                posição, enabled) — ponte PowerShell (protótipo; COM nativo depois).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "max": { "type": "integer" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "uia_invoke",
            "description": "Invoca (InvokePattern) ou define valor (ValuePattern) de um \
                elemento UIA pelo nome ou AutomationId — ponte PowerShell (protótipo).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hwnd": { "type": "integer" },
                    "title": { "type": "string" },
                    "name": { "type": "string" },
                    "action": { "type": "string", "enum": ["invoke", "set"] },
                    "value": { "type": "string" }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        },
        {
            "name": "guardrails_status",
            "description": "Mostra as regras de guardrails em vigor (blocklist de apps/títulos, \
                redaction de regiões/títulos) e o caminho do log de auditoria.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }
    ])
}

fn call_tool(name: &str, args: &Value) -> Result<Vec<Value>, String> {
    match name {
        "status" => {
            let windows = list_windows();
            let dock_info = match dock() {
                Ok(dock) => dock
                    .call("status", json!({}))
                    .ok()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "erro ao consultar".to_string()),
                Err(_) => "offline (abra o MayerDock para slots/HUD)".to_string(),
            };
            let text = format!(
                "mayerdock-mcp {}\r\npid: {}\r\njanelas visíveis: {}\r\ndock: {dock_info}",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                windows.len()
            );
            Ok(vec![text_content(&text)])
        }
        "list_windows" => {
            let windows = list_windows();
            let text = windows
                .iter()
                .map(|w| {
                    format!(
                        "0x{:X}\t{}\t{}\tpid={}\t{}x{}\tdpi={:.0}%",
                        w.hwnd as usize,
                        w.class,
                        w.title,
                        w.pid,
                        w.rect.right - w.rect.left,
                        w.rect.bottom - w.rect.top,
                        w.dpi * 100.0
                    )
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            Ok(vec![text_content(if text.is_empty() {
                "(nenhuma janela visível com título)"
            } else {
                &text
            })])
        }
        "capture_window" => capture_window(args),
        "open_app" => open_app(args),
        "window_state" => window_state(args),
        "list_slots" => {
            let slots = dock_tool("list_slots", json!({}))?;
            Ok(vec![text_content(&pretty(&slots))])
        }
        "dock_pin" => {
            let hwnd = args
                .get("hwnd")
                .and_then(Value::as_i64)
                .ok_or("informe hwnd")?;
            let result = dock_tool("dock_pin", json!({ "hwnd": hwnd }))?;
            Ok(vec![text_content(&pretty(&result))])
        }
        "dock_unpin" => {
            let hwnd = args
                .get("hwnd")
                .and_then(Value::as_i64)
                .ok_or("informe hwnd")?;
            let result = dock_tool("dock_unpin", json!({ "hwnd": hwnd }))?;
            Ok(vec![text_content(&pretty(&result))])
        }
        "dock_release_all" => {
            let result = dock_tool("dock_release_all", json!({}))?;
            Ok(vec![text_content(&pretty(&result))])
        }
        "list_controls" => list_controls(args),
        "click" => click(args),
        "type_text" => type_text(args),
        "send_key" => send_key(args),
        "get_text" => get_text(args),
        "wait_for" => wait_for(args),
        "focus_lease" => focus_lease(args),
        "global_click" => global_click(args),
        "global_type" => global_type(args),
        "global_key" => global_key(args),
        "set_text" => set_text(args),
        "control_click" => control_click(args),
        "browser_launch" => browser_launch(args),
        "uia_query" => uia_query(args),
        "uia_invoke" => uia_invoke(args),
        "guardrails_status" => {
            let guardrails = load_guardrails();
            let regions: Vec<Value> = guardrails
                .redact_regions
                .iter()
                .map(|rect| {
                    json!({
                        "left": rect.left,
                        "top": rect.top,
                        "right": rect.right,
                        "bottom": rect.bottom
                    })
                })
                .collect();
            Ok(vec![text_content(&pretty(&json!({
                "config": guardrails_config_path().display().to_string(),
                "audit": audit_log_path().display().to_string(),
                "blocked_processes": guardrails.blocked_processes,
                "blocked_title_patterns": guardrails.blocked_title_patterns,
                "redact_titles": guardrails.redact_titles,
                "redact_regions": regions
            })))])
        }
        _ => Err(format!("tool desconhecida: {name}")),
    }
}

fn text_content(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

// ---------------------------------------------------------------------------
// Janelas
// ---------------------------------------------------------------------------

struct WinInfo {
    hwnd: HWND,
    title: String,
    class: String,
    pid: u32,
    rect: RECT,
    dpi: f64,
}

fn list_windows() -> Vec<WinInfo> {
    let mut list: Vec<WinInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_window), &mut list as *mut Vec<WinInfo> as LPARAM);
    }
    let guardrails = load_guardrails();
    list.retain(|window| guardrails_block_reason(window.hwnd, &guardrails).is_none());
    for window in list.iter_mut() {
        if is_redacted_title(window.hwnd, &guardrails) {
            window.title = "«redigido»".to_string();
        }
    }
    list
}

// ---------------------------------------------------------------------------
// Guardrails (md-d33.3.8): blocklist, redaction de titulos/regioes e auditoria
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Guardrails {
    blocked_processes: Vec<String>,
    blocked_title_patterns: Vec<String>,
    redact_titles: Vec<String>,
    redact_regions: Vec<RECT>,
}

impl Default for Guardrails {
    fn default() -> Self {
        Guardrails {
            blocked_processes: [
                "keepass",
                "keepassxc",
                "1password",
                "bitwarden",
                "lastpass",
                "dashlane",
                "nordpass",
                "enpass",
                "roboform",
                "protonpass",
                "keeper",
            ]
            .iter()
            .map(|value| value.to_string())
            .collect(),
            blocked_title_patterns: [
                "senha",
                "password",
                "1password",
                "keepass",
                "bitwarden",
                "lastpass",
                "dashlane",
                "nordpass",
                "enpass",
                "roboform",
                "nubank",
                "itau",
                "itaú",
                "bradesco",
                "santander",
                "banco do brasil",
                "caixa econ",
                "paypal",
                "picpay",
                "mercadopago",
            ]
            .iter()
            .map(|value| value.to_string())
            .collect(),
            redact_titles: ["senha", "password", "token", "segredo"]
                .iter()
                .map(|value| value.to_string())
                .collect(),
            redact_regions: Vec::new(),
        }
    }
}

fn guardrails_config_path() -> PathBuf {
    crate::config::data_dir().join("guardrails.json")
}

fn audit_log_path() -> PathBuf {
    crate::config::data_dir().join("audit.jsonl")
}

fn normalize_guardrails(mut guardrails: Guardrails) -> Guardrails {
    for list in [
        &mut guardrails.blocked_processes,
        &mut guardrails.blocked_title_patterns,
        &mut guardrails.redact_titles,
    ] {
        for value in list.iter_mut() {
            *value = value.trim().to_lowercase();
        }
    }
    guardrails
}

fn load_guardrails() -> Guardrails {
    let mut guardrails = Guardrails::default();
    let Ok(text) = std::fs::read_to_string(guardrails_config_path()) else {
        return normalize_guardrails(guardrails);
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return normalize_guardrails(guardrails);
    };
    if value
        .get("disable_defaults")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        guardrails = Guardrails {
            blocked_processes: Vec::new(),
            blocked_title_patterns: Vec::new(),
            redact_titles: Vec::new(),
            redact_regions: Vec::new(),
        };
    }
    for (key, target) in [
        ("blocked_processes", &mut guardrails.blocked_processes),
        (
            "blocked_title_patterns",
            &mut guardrails.blocked_title_patterns,
        ),
        ("redact_titles", &mut guardrails.redact_titles),
    ] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(text) = item.as_str() {
                    target.push(text.to_string());
                }
            }
        }
    }
    if let Some(regions) = value.get("redact_regions").and_then(Value::as_array) {
        for region in regions {
            let get = |name: &str| region.get(name).and_then(Value::as_i64).unwrap_or(0) as i32;
            let rect = RECT {
                left: get("left"),
                top: get("top"),
                right: get("right"),
                bottom: get("bottom"),
            };
            if rect.right > rect.left && rect.bottom > rect.top {
                guardrails.redact_regions.push(rect);
            }
        }
    }
    normalize_guardrails(guardrails)
}

fn window_process_name(hwnd: HWND) -> String {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return String::new();
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return String::new();
        }
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size);
        CloseHandle(process);
        if ok == 0 {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        std::path::Path::new(&path)
            .file_stem()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }
}

fn guardrails_block_reason(hwnd: HWND, guardrails: &Guardrails) -> Option<String> {
    let process = window_process_name(hwnd);
    if let Some(pattern) = guardrails
        .blocked_processes
        .iter()
        .find(|pattern| !pattern.is_empty() && process.contains(pattern.as_str()))
    {
        return Some(format!("processo bloqueado ({pattern})"));
    }
    let title = window_title(hwnd).to_lowercase();
    if let Some(pattern) = guardrails
        .blocked_title_patterns
        .iter()
        .find(|pattern| !pattern.is_empty() && title.contains(pattern.as_str()))
    {
        return Some(format!("título bloqueado ({pattern})"));
    }
    None
}

fn is_redacted_title(hwnd: HWND, guardrails: &Guardrails) -> bool {
    let title = window_title(hwnd).to_lowercase();
    guardrails
        .redact_titles
        .iter()
        .any(|pattern| !pattern.is_empty() && title.contains(pattern.as_str()))
}

fn audit_guardrail(tool: &str, hwnd: HWND, allowed: bool, detail: &str) {
    use std::io::Write;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let line = json!({
        "ts": timestamp,
        "tool": tool,
        "hwnd": hwnd as usize,
        "allowed": allowed,
        "detail": detail,
    });
    let path = audit_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        if let Ok(text) = serde_json::to_string(&line) {
            let _ = writeln!(file, "{text}");
        }
    }
}

/// Resolve o alvo e aplica os guardrails (bloqueia app/título sensível).
fn ensure_target(args: &Value, tool: &str) -> Result<HWND, String> {
    let hwnd = find_window(args)?;
    let guardrails = load_guardrails();
    if let Some(reason) = guardrails_block_reason(hwnd, &guardrails) {
        audit_guardrail(tool, hwnd, false, &reason);
        return Err(format!("guardrails: {reason}"));
    }
    Ok(hwnd)
}

fn redact_capture(
    pixels: &mut [u8],
    width: i32,
    height: i32,
    origin_x: i32,
    origin_y: i32,
    regions: &[RECT],
) {
    for region in regions {
        let x0 = (region.left - origin_x).max(0);
        let y0 = (region.top - origin_y).max(0);
        let x1 = (region.right - origin_x).min(width).max(0);
        let y1 = (region.bottom - origin_y).min(height).max(0);
        for y in y0..y1 {
            let row = (y as usize) * (width as usize) * 4;
            for x in x0..x1 {
                let offset = row + (x as usize) * 4;
                pixels[offset] = 0;
                pixels[offset + 1] = 0;
                pixels[offset + 2] = 0;
                pixels[offset + 3] = 255;
            }
        }
    }
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    let list = &mut *(lparam as *mut Vec<WinInfo>);
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    let title = window_title(hwnd);
    if title.is_empty() {
        return 1;
    }
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == std::process::id() {
        return 1;
    }
    let mut class = [0u16; 256];
    let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
    let mut rect: RECT = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut rect) == 0 || rect.right <= rect.left || rect.bottom <= rect.top {
        return 1;
    }
    let dpi = GetDpiForWindow(hwnd) as f64 / 96.0;
    list.push(WinInfo {
        hwnd,
        title,
        class,
        pid,
        rect,
        dpi: if dpi > 0.0 { dpi } else { 1.0 },
    });
    1
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..copied as usize])
    }
}

fn find_window(args: &Value) -> Result<HWND, String> {
    if let Some(raw) = args.get("hwnd").and_then(Value::as_i64) {
        let hwnd = raw as usize as HWND;
        if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
            return Err(format!("hwnd inválido: 0x{raw:X}"));
        }
        return Ok(hwnd);
    }
    let needle = args
        .get("title")
        .and_then(Value::as_str)
        .ok_or("informe hwnd ou title")?
        .to_lowercase();
    let windows = list_windows();
    windows
        .iter()
        .find(|w| w.title.to_lowercase().contains(&needle))
        .map(|w| w.hwnd)
        .ok_or_else(|| format!("nenhuma janela com título contendo {needle:?}"))
}

// ---------------------------------------------------------------------------
// Captura de janela (PrintWindow + fallback BitBlt) → PNG
// ---------------------------------------------------------------------------

struct SavedCapture {
    file: PathBuf,
    png: Vec<u8>,
    width: u32,
    height: u32,
    dpi: f64,
    method: &'static str,
    hwnd: HWND,
}

/// Captura uma janela e grava o PNG (respeita `method` e `out` do args).
fn save_capture(args: &Value) -> Result<SavedCapture, String> {
    let hwnd = ensure_target(args, "save_capture")?;
    let method = args.get("method").and_then(Value::as_str).unwrap_or("auto");
    let (png, width, height, dpi, used) = capture(hwnd, method)?;
    let file = match args.get("out").and_then(Value::as_str) {
        Some(path) => PathBuf::from(path),
        None => capture_dir().join(format!(
            "cap-{}.png",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0)
        )),
    };
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&file, &png).map_err(|err| format!("não escrevi {:?}: {err}", file))?;
    unsafe {
        CAPTURES = CAPTURES.saturating_add(1);
    }
    dock_activity_if_connected("capturando");
    crate::win::log(&format!(
        "mcp: captura hwnd=0x{:X} {}x{} {} bytes ({used}) -> {}",
        hwnd as usize,
        width,
        height,
        png.len(),
        file.display()
    ));
    Ok(SavedCapture {
        file,
        png,
        width,
        height,
        dpi,
        method: used,
        hwnd,
    })
}

fn capture_window(args: &Value) -> Result<Vec<Value>, String> {
    let saved = save_capture(args)?;
    let title = window_title(saved.hwnd);
    let text = format!(
        "capturada: {title:?}\r\narquivo: {}\r\n{}x{} px dpi={:.0}%\r\nmétodo: {}\r\nbytes: {}",
        saved.file.display(),
        saved.width,
        saved.height,
        saved.dpi * 100.0,
        saved.method,
        saved.png.len()
    );
    let data = base64::engine::general_purpose::STANDARD.encode(&saved.png);
    Ok(vec![
        text_content(&text),
        json!({ "type": "image", "mimeType": "image/png", "data": data }),
    ])
}

fn capture_dir() -> PathBuf {
    // Pasta estável por usuário (o TEMP desta máquina é redirecionado).
    crate::config::data_dir()
        .join("agent")
        .join(std::process::id().to_string())
}

// ---------------------------------------------------------------------------
// Modo console / CI (one-shot, sem abrir o dock)
// ---------------------------------------------------------------------------

fn print_json(value: &Value) {
    crate::win::console_print(&format!(
        "{}\r\n",
        serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
    ));
}

fn cli_error(message: &str, json_output: bool) -> i32 {
    if json_output {
        print_json(&json!({ "ok": false, "error": message }));
    } else {
        crate::win::console_print(&format!("erro: {message}\r\n"));
    }
    1
}

/// Erro de uso/script inválido → exit code 2 (como argumentos inválidos).
fn cli_usage_error(message: &str, json_output: bool) -> i32 {
    if json_output {
        print_json(&json!({ "ok": false, "error": message }));
    } else {
        crate::win::console_print(&format!("erro: {message}\r\n"));
    }
    2
}

/// `--list-windows [--json]`: inventário de janelas para scripts/CI.
pub fn cli_list_windows(json_output: bool) -> i32 {
    let windows = list_windows();
    if json_output {
        let items: Vec<Value> = windows
            .iter()
            .map(|window| {
                json!({
                    "hwnd": window.hwnd as usize,
                    "title": window.title,
                    "class": window.class,
                    "pid": window.pid,
                    "rect": {
                        "left": window.rect.left,
                        "top": window.rect.top,
                        "right": window.rect.right,
                        "bottom": window.rect.bottom
                    },
                    "width": window.rect.right - window.rect.left,
                    "height": window.rect.bottom - window.rect.top,
                    "dpi_scale": window.dpi
                })
            })
            .collect();
        print_json(&json!({ "ok": true, "windows": items, "count": windows.len() }));
        return 0;
    }
    let mut out = String::new();
    for window in &windows {
        out.push_str(&format!(
            "0x{:X}\t{}\t{}\tpid={}\t{}x{}\tdpi={:.0}%\r\n",
            window.hwnd as usize,
            window.class,
            window.title,
            window.pid,
            window.rect.right - window.rect.left,
            window.rect.bottom - window.rect.top,
            window.dpi * 100.0
        ));
    }
    if out.is_empty() {
        out.push_str("(nenhuma janela visível com título)\r\n");
    }
    crate::win::console_print(&out);
    0
}

/// `--capture (--title T | --hwnd N) [--out ARQ] [--json]`.
pub fn cli_capture(
    hwnd: Option<i64>,
    title: Option<String>,
    out: Option<String>,
    json_output: bool,
) -> i32 {
    let mut args = match (hwnd, title.as_ref()) {
        (Some(hwnd), _) => json!({ "hwnd": hwnd }),
        (None, Some(title)) => json!({ "title": title }),
        (None, None) => return cli_error("informe --title ou --hwnd", json_output),
    };
    if let Some(out) = &out {
        if let Some(object) = args.as_object_mut() {
            object.insert("out".to_string(), json!(out));
        }
    }
    let saved = match save_capture(&args) {
        Ok(saved) => saved,
        Err(err) => return cli_error(&err, json_output),
    };
    if json_output {
        print_json(&json!({
            "ok": true,
            "file": saved.file.display().to_string(),
            "hwnd": saved.hwnd as usize,
            "width": saved.width,
            "height": saved.height,
            "dpi_scale": saved.dpi,
            "method": saved.method,
            "bytes": saved.png.len()
        }));
    } else {
        crate::win::console_print(&format!(
            "capturada: 0x{:X} {}x{} dpi={:.0}% método={}\r\narquivo: {}\r\nbytes: {}\r\n",
            saved.hwnd as usize,
            saved.width,
            saved.height,
            saved.dpi * 100.0,
            saved.method,
            saved.file.display(),
            saved.png.len()
        ));
    }
    0
}

// ---------------------------------------------------------------------------
// Script runner (Fase 2 do modo console): --script arquivo.json
// ---------------------------------------------------------------------------

fn step_output_text(values: &[Value]) -> Option<String> {
    values.iter().find_map(|value| {
        value
            .get("text")
            .and_then(Value::as_str)
            .map(|text| text.to_string())
    })
}

/// Substitui strings `"$var"` pelos valores capturados nos passos anteriores.
fn vars_resolve(args: &Value, vars: &HashMap<String, Value>) -> Value {
    match args {
        Value::String(text) => match text.strip_prefix('$') {
            Some(name) => vars.get(name).cloned().unwrap_or_else(|| args.clone()),
            None => args.clone(),
        },
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| vars_resolve(item, vars)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), vars_resolve(value, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn script_sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms.min(60_000)));
}

fn script_wait_window(args: &Value) -> Result<(Value, Option<Value>), String> {
    let timeout = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5000)
        .clamp(100, 60_000);
    let started = Instant::now();
    loop {
        if let Ok(hwnd) = find_window(args) {
            let hwnd_number = hwnd as usize;
            return Ok((json!({ "hwnd": hwnd_number }), Some(json!(hwnd_number))));
        }
        if started.elapsed().as_millis() as u64 >= timeout {
            return Err(format!("timeout de {timeout} ms esperando janela"));
        }
        script_sleep(100);
    }
}

/// Executa uma ação do script; devolve (saída para o relatório, valor para `as`).
fn run_script_action(
    action: &str,
    args: &Value,
    _vars: &mut HashMap<String, Value>,
) -> Result<(Value, Option<Value>), String> {
    match action {
        "open_app" => {
            let content = open_app(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "wait_window" => script_wait_window(args),
        "assert_window" => {
            let result = script_wait_window(args)?;
            Ok((json!({ "ok": true, "hwnd": result.0["hwnd"] }), None))
        }
        "capture" => {
            let saved = save_capture(args)?;
            Ok((
                json!({
                    "file": saved.file.display().to_string(),
                    "width": saved.width,
                    "height": saved.height,
                    "method": saved.method
                }),
                Some(json!(saved.file.display().to_string())),
            ))
        }
        "click" => {
            let content = click(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "type_text" => {
            let content = type_text(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "send_key" => {
            let content = send_key(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "set_text" => {
            let content = set_text(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "control_click" => {
            let content = control_click(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "get_text" => {
            let hwnd = ensure_target(args, "run_script_action")?;
            let text = window_message_text(hwnd).unwrap_or_else(|| window_title(hwnd));
            Ok((json!({ "text": text.clone() }), Some(json!(text))))
        }
        "find_control" => {
            let parent = find_window(args)?;
            let class_filter = args
                .get("class")
                .and_then(Value::as_str)
                .map(|value| value.to_lowercase());
            let text_filter = args
                .get("text_contains")
                .and_then(Value::as_str)
                .map(|value| value.to_lowercase());
            for control in child_controls(parent) {
                if let Some(filter) = &class_filter {
                    if !control.class.to_lowercase().contains(filter) {
                        continue;
                    }
                }
                if let Some(filter) = &text_filter {
                    if !control.text.to_lowercase().contains(filter) {
                        continue;
                    }
                }
                let hwnd = control.hwnd as usize;
                return Ok((
                    json!({ "hwnd": hwnd, "class": control.class, "text": control.text }),
                    Some(json!(hwnd)),
                ));
            }
            Err("controle não encontrado (class/text_contains)".to_string())
        }
        "wait_for" => {
            let content = wait_for(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "uia_query" => {
            let content = uia_query(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "uia_invoke" => {
            let content = uia_invoke(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "window_state" => {
            let content = window_state(args)?;
            Ok((json!({ "text": step_output_text(&content) }), None))
        }
        "list_slots" | "dock_pin" | "dock_unpin" | "dock_release_all" => {
            let result = dock_tool(action, args.clone())?;
            Ok((result.clone(), None))
        }
        "sleep" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(500);
            script_sleep(ms);
            Ok((json!({ "slept_ms": ms }), None))
        }
        "assert_text" => {
            let hwnd = ensure_target(args, "run_script_action")?;
            let text = window_message_text(hwnd).unwrap_or_else(|| window_title(hwnd));
            let contains = args.get("contains").and_then(Value::as_str);
            let equals = args.get("equals").and_then(Value::as_str);
            let ok = if let Some(needle) = contains {
                text.contains(needle)
            } else if let Some(expected) = equals {
                text == expected
            } else {
                return Err("assert_text exige contains ou equals".to_string());
            };
            if !ok {
                return Err(format!(
                    "assert_text falhou: esperado {:?}/{:?}, obtido {text:?}",
                    contains, equals
                ));
            }
            Ok((json!({ "text": text }), None))
        }
        other => Err(format!("ação desconhecida: {other}")),
    }
}

/// `--script arquivo.json [--json] [--timeout ms]`: roda passos declarativos.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_text_artifact(path: &str, content: &str) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, content).map_err(|err| format!("não escrevi {path:?}: {err}"))
}

/// Gera JUnit XML a partir do relatório do script (para a esteira de CI).
fn junit_report(name: &str, results: &[Value], duration_ms: u128) -> String {
    let tests = results.len();
    let failures = results
        .iter()
        .filter(|step| step.get("ok").and_then(Value::as_bool) != Some(true))
        .count();
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<testsuite name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" errors=\"0\" time=\"{:.3}\">\n",
        xml_escape(name),
        duration_ms as f64 / 1000.0
    ));
    for step in results {
        let index = step.get("index").and_then(Value::as_u64).unwrap_or(0);
        let action = step.get("action").and_then(Value::as_str).unwrap_or("-");
        let ms = step.get("ms").and_then(Value::as_u64).unwrap_or(0);
        let case = format!("[{index}] {action}");
        let step_ok = step.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if step_ok {
            xml.push_str(&format!(
                "  <testcase classname=\"mayerdock.script\" name=\"{}\" time=\"{:.3}\"/>\n",
                xml_escape(&case),
                ms as f64 / 1000.0
            ));
        } else {
            let error = step.get("error").and_then(Value::as_str).unwrap_or("");
            xml.push_str(&format!(
                "  <testcase classname=\"mayerdock.script\" name=\"{}\" time=\"{:.3}\"><failure message=\"{}\">{}</failure></testcase>\n",
                xml_escape(&case),
                ms as f64 / 1000.0,
                xml_escape(error),
                xml_escape(error)
            ));
        }
    }
    xml.push_str("</testsuite>\n");
    xml
}

/// `--script arquivo.json [--json] [--timeout ms] [--ci] [--report-junit arq] [--report-json arq]`.
pub fn cli_script(
    path: &str,
    json_output: bool,
    timeout_ms: Option<u64>,
    ci: bool,
    junit_path: Option<&str>,
    report_json_path: Option<&str>,
) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => return cli_usage_error(&format!("não li {path:?}: {err}"), json_output),
    };
    // Aceita arquivos gerados por editores Windows que gravam BOM UTF-8.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let script: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return cli_usage_error(&format!("JSON inválido em {path:?}: {err}"), json_output)
        }
    };
    let Some(steps) = script.get("steps").and_then(Value::as_array) else {
        return cli_usage_error("script sem campo \"steps\" (array)", json_output);
    };
    let name = script
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("script");
    let global_timeout = timeout_ms
        .or(script.get("timeout_ms").and_then(Value::as_u64))
        .or(if ci { Some(120_000) } else { None });
    let started = Instant::now();
    let mut vars: HashMap<String, Value> = HashMap::new();
    let mut results: Vec<Value> = Vec::new();
    let mut ok = true;
    let mut aborted = false;

    for (index, step) in steps.iter().enumerate() {
        if let Some(limit) = global_timeout {
            if started.elapsed().as_millis() as u64 > limit {
                results.push(json!({
                    "index": index,
                    "ok": false,
                    "error": format!("timeout global de {limit} ms")
                }));
                ok = false;
                aborted = true;
                break;
            }
        }
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let raw_args = step.get("args").cloned().unwrap_or_else(|| json!({}));
        let step_args = vars_resolve(&raw_args, &vars);
        let step_started = Instant::now();
        let result = if ci
            && matches!(
                action.as_str(),
                "focus_lease" | "global_click" | "global_type" | "global_key"
            ) {
            Err(format!(
                "ação {action:?} bloqueada em --ci (exige interação/lease; use input direcionado)"
            ))
        } else {
            run_script_action(&action, &step_args, &mut vars)
        };
        let elapsed = step_started.elapsed().as_millis();
        match result {
            Ok((output, variable)) => {
                if let (Some(value), Some(variable_name)) =
                    (variable, step.get("as").and_then(Value::as_str))
                {
                    vars.insert(variable_name.to_string(), value);
                }
                results.push(json!({
                    "index": index,
                    "action": action,
                    "ok": true,
                    "ms": elapsed,
                    "output": output
                }));
            }
            Err(err) => {
                results.push(json!({
                    "index": index,
                    "action": action,
                    "ok": false,
                    "ms": elapsed,
                    "error": err
                }));
                let continue_on_error = step
                    .get("continue_on_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !continue_on_error {
                    ok = false;
                    aborted = true;
                    break;
                }
            }
        }
    }

    let report = json!({
        "ok": ok,
        "name": name,
        "steps": results,
        "duration_ms": started.elapsed().as_millis(),
        "aborted": aborted,
        "ci": ci
    });
    // Artefatos de CI: JUnit (para o runner) e relatório JSON (para dashboards).
    if let Some(path) = report_json_path {
        let pretty = serde_json::to_string_pretty(&report).unwrap_or_default();
        if let Err(err) = write_text_artifact(path, &pretty) {
            crate::win::console_print(&format!("erro: {err}\r\n"));
            return 2;
        }
    }
    if let Some(path) = junit_path {
        let xml = junit_report(
            name,
            report["steps"]
                .as_array()
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
            started.elapsed().as_millis(),
        );
        if let Err(err) = write_text_artifact(path, &xml) {
            crate::win::console_print(&format!("erro: {err}\r\n"));
            return 2;
        }
    }
    if json_output {
        print_json(&report);
    } else {
        let mut out = String::new();
        out.push_str(&format!("script: {name}\r\n"));
        for step in report["steps"].as_array().into_iter().flatten() {
            let index = step.get("index").and_then(Value::as_u64).unwrap_or(0);
            let action = step.get("action").and_then(Value::as_str).unwrap_or("-");
            let step_ok = step.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let ms = step.get("ms").and_then(Value::as_u64).unwrap_or(0);
            if step_ok {
                out.push_str(&format!("  [{index}] {action}: ok ({ms} ms)\r\n"));
            } else {
                let error = step.get("error").and_then(Value::as_str).unwrap_or("");
                out.push_str(&format!(
                    "  [{index}] {action}: FALHOU ({ms} ms): {error}\r\n"
                ));
            }
        }
        out.push_str(&format!(
            "resultado: {} em {} ms\r\n",
            if ok { "PASSOU" } else { "FALHOU" },
            started.elapsed().as_millis()
        ));
        crate::win::console_print(&out);
    }
    if ok {
        0
    } else {
        1
    }
}

fn capture(hwnd: HWND, method: &str) -> Result<(Vec<u8>, u32, u32, f64, &'static str), String> {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return Err("GetWindowRect falhou".into());
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return Err(format!("janela sem tamanho: {width}x{height}"));
        }
        let dpi = {
            let d = GetDpiForWindow(hwnd);
            if d > 0 {
                d as f64 / 96.0
            } else {
                1.0
            }
        };

        let window_dc = GetWindowDC(hwnd);
        if window_dc.is_null() {
            return Err("GetWindowDC falhou".into());
        }
        let mem_dc = CreateCompatibleDC(window_dc);
        let bitmap = CreateCompatibleBitmap(window_dc, width, height);
        if mem_dc.is_null() || bitmap.is_null() {
            ReleaseDC(hwnd, window_dc);
            return Err("não criei DC/bitmap compatível".into());
        }
        let old = SelectObject(mem_dc, bitmap as HGDIOBJ);

        // PrintWindow captura até janelas ocluídas; BitBlt é o fallback.
        // `method` permite comparar os dois no spike de compatibilidade.
        let want_bitblt = method.eq_ignore_ascii_case("bitblt");
        let printed = if want_bitblt {
            0
        } else {
            PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT)
        };
        let used = if printed != 0 {
            "PrintWindow"
        } else {
            BitBlt(mem_dc, 0, 0, width, height, window_dc, 0, 0, SRCCOPY);
            "BitBlt"
        };

        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = 0; // BI_RGB
        let lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr() as *mut c_void,
            &mut info,
            DIB_RGB_COLORS,
        );

        SelectObject(mem_dc, old);
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(mem_dc);
        ReleaseDC(hwnd, window_dc);

        if lines == 0 {
            return Err("GetDIBits falhou".into());
        }

        // BGRA -> RGBA
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }

        // Guardrails: pinta de preto as regiões configuradas (coordenadas de tela)
        // antes de o PNG sair do processo.
        let guardrails = load_guardrails();
        if !guardrails.redact_regions.is_empty() {
            redact_capture(
                &mut pixels,
                width,
                height,
                rect.left,
                rect.top,
                &guardrails.redact_regions,
            );
        }

        let mut png_bytes: Vec<u8> = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, width as u32, height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|e| format!("png header: {e}"))?;
            writer
                .write_image_data(&pixels)
                .map_err(|e| format!("png data: {e}"))?;
        }
        Ok((png_bytes, width as u32, height as u32, dpi, used))
    }
}

// ---------------------------------------------------------------------------
// open_app / window_state
// ---------------------------------------------------------------------------

fn open_app(args: &Value) -> Result<Vec<Value>, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("informe path")?;
    let arguments = args.get("args").and_then(Value::as_str).unwrap_or("");
    let file = crate::win::wide(path);
    let params = crate::win::wide(arguments);
    let op = crate::win::wide("open");
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            if arguments.is_empty() {
                std::ptr::null()
            } else {
                params.as_ptr()
            },
            std::ptr::null(),
            SW_SHOWNOACTIVATE as i32,
        )
    };
    let code = result as isize;
    if code <= 32 {
        return Err(format!("ShellExecute falhou (código {code}) em {path:?}"));
    }
    crate::win::log(&format!("mcp: open_app {path} {arguments}"));
    Ok(vec![text_content(&format!(
        "aberto sem ativar: {path} {arguments}. Use list_windows + dock_pin(hwnd) para encaixar no dock."
    ))])
}

fn window_state(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "window_state")?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or("informe action")?;
    unsafe {
        match action {
            "maximize" => ShowWindow(hwnd, SW_MAXIMIZE),
            "minimize" => ShowWindow(hwnd, SW_MINIMIZE),
            "restore" => ShowWindow(hwnd, SW_RESTORE),
            "close" => {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
                1
            }
            other => return Err(format!("action inválida: {other}")),
        };
    }
    crate::win::log(&format!(
        "mcp: window_state {action} hwnd=0x{:X}",
        hwnd as usize
    ));
    Ok(vec![text_content(&format!("{action}: ok"))])
}

// ---------------------------------------------------------------------------
// Controles e input direcionado (PostMessage — sem cursor/teclado global)
// ---------------------------------------------------------------------------

struct ControlInfo {
    hwnd: HWND,
    class: String,
    text: String,
    id: i32,
    enabled: bool,
    rect: RECT,
}

fn list_controls(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "list_controls")?;
    dock_activity_if_connected("lendo controles");
    let controls = child_controls(hwnd);
    if controls.is_empty() {
        return Ok(vec![text_content("(sem controles filhos)")]);
    }
    let lines = controls
        .iter()
        .map(|control| {
            format!(
                "0x{:X}\t{}\t{}\tid={}\tenabled={}\t{}x{} @ ({},{})",
                control.hwnd as usize,
                control.class,
                control.text,
                control.id,
                control.enabled,
                control.rect.right - control.rect.left,
                control.rect.bottom - control.rect.top,
                control.rect.left,
                control.rect.top
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    Ok(vec![text_content(&lines)])
}

fn child_controls(hwnd: HWND) -> Vec<ControlInfo> {
    let mut out: Vec<ControlInfo> = Vec::new();
    unsafe {
        EnumChildWindows(
            hwnd,
            Some(enum_child),
            &mut out as *mut Vec<ControlInfo> as LPARAM,
        );
    }
    out
}

unsafe extern "system" fn enum_child(hwnd: HWND, lparam: LPARAM) -> i32 {
    let out = &mut *(lparam as *mut Vec<ControlInfo>);
    let mut class = [0u16; 256];
    let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
    let mut rect: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut rect);
    out.push(ControlInfo {
        hwnd,
        class,
        text: window_title(hwnd),
        id: GetDlgCtrlID(hwnd),
        enabled: IsWindowEnabled(hwnd) != 0,
        rect,
    });
    1
}

fn click(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "click")?;
    let x = args.get("x").and_then(Value::as_i64).unwrap_or(0) as i32;
    let y = args.get("y").and_then(Value::as_i64).unwrap_or(0) as i32;
    let (cx, cy) = match args.get("coords").and_then(Value::as_str) {
        Some("screen") => {
            let mut point = POINT { x, y };
            unsafe {
                ScreenToClient(hwnd, &mut point);
            }
            (point.x, point.y)
        }
        _ => (x, y),
    };
    let lparam = (((cy as u16 as u32) << 16) | (cx as u16 as u32)) as isize;
    dock_activity_if_connected("clicando");
    unsafe {
        PostMessageW(hwnd, WM_MOUSEMOVE, 0, lparam);
        PostMessageW(hwnd, WM_LBUTTONDOWN, MK_LBUTTON, lparam);
        PostMessageW(hwnd, WM_LBUTTONUP, 0, lparam);
    }
    unsafe {
        CLICKS = CLICKS.saturating_add(1);
    }
    crate::win::log(&format!(
        "mcp: click hwnd=0x{:X} ({cx},{cy})",
        hwnd as usize
    ));
    Ok(vec![text_content(&format!(
        "click em 0x{:X} ({cx},{cy})",
        hwnd as usize
    ))])
}

fn type_text(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "type_text")?;
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or("informe text")?;
    dock_activity_if_connected("digitando");
    for unit in text.encode_utf16() {
        unsafe {
            PostMessageW(hwnd, WM_CHAR, unit as usize, 1);
        }
    }
    crate::win::log(&format!(
        "mcp: type_text hwnd=0x{:X} {} chars",
        hwnd as usize,
        text.chars().count()
    ));
    Ok(vec![text_content(&format!(
        "digitado em 0x{:X}: {text:?}",
        hwnd as usize
    ))])
}

fn send_key(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "send_key")?;
    let key = args.get("key").ok_or("informe key")?;
    let vk: u32 = if let Some(code) = key.as_u64() {
        code as u32
    } else {
        let name = key.as_str().unwrap_or("").to_ascii_lowercase();
        match name.as_str() {
            "enter" | "return" => 0x0D,
            "tab" => 0x09,
            "esc" | "escape" => 0x1B,
            "space" => 0x20,
            "backspace" => 0x08,
            "delete" | "del" => 0x2E,
            "up" => 0x26,
            "down" => 0x28,
            "left" => 0x25,
            "right" => 0x27,
            other => return Err(format!("tecla desconhecida: {other}")),
        }
    };
    dock_activity_if_connected("enviando tecla");
    unsafe {
        PostMessageW(hwnd, WM_KEYDOWN, vk as usize, 1);
        PostMessageW(hwnd, WM_KEYUP, vk as usize, 1);
    }
    crate::win::log(&format!(
        "mcp: send_key hwnd=0x{:X} vk=0x{vk:02X}",
        hwnd as usize
    ));
    Ok(vec![text_content(&format!(
        "tecla 0x{vk:02X} em 0x{:X}",
        hwnd as usize
    ))])
}

fn get_text(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "get_text")?;
    let text = window_message_text(hwnd).unwrap_or_else(|| window_title(hwnd));
    Ok(vec![text_content(&format!(
        "0x{:X}: {text:?}",
        hwnd as usize
    ))])
}

fn window_message_text(hwnd: HWND) -> Option<String> {
    unsafe {
        let len = SendMessageW(hwnd, WM_GETTEXTLENGTH, 0, 0) as usize;
        if len == 0 {
            return None;
        }
        let len = len.min(1 << 20);
        let mut buffer = vec![0u16; len + 1];
        let copied =
            SendMessageW(hwnd, WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize) as usize;
        if copied == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..copied]))
    }
}

fn wait_for(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "wait_for")?;
    let timeout = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5000)
        .clamp(100, 30000);
    let title_needle = args
        .get("title_contains")
        .and_then(Value::as_str)
        .map(|value| value.to_lowercase());
    let text_needle = args
        .get("text_contains")
        .and_then(Value::as_str)
        .map(|value| value.to_lowercase());
    let visible = args.get("visible").and_then(Value::as_bool);
    if title_needle.is_none() && text_needle.is_none() && visible.is_none() {
        return Err("informe title_contains, text_contains ou visible".to_string());
    }
    dock_activity_if_connected("aguardando condição");
    let started = std::time::Instant::now();
    loop {
        let mut matched = true;
        if let Some(needle) = &title_needle {
            matched &= window_title(hwnd).to_lowercase().contains(needle);
        }
        if let Some(needle) = &text_needle {
            let text = window_message_text(hwnd)
                .unwrap_or_else(|| window_title(hwnd))
                .to_lowercase();
            matched &= text.contains(needle);
        }
        if let Some(expected) = visible {
            matched &= (unsafe { IsWindowVisible(hwnd) } != 0) == expected;
        }
        if matched {
            return Ok(vec![text_content(&format!(
                "condição atingida em {} ms",
                started.elapsed().as_millis()
            ))]);
        }
        if started.elapsed().as_millis() as u64 >= timeout {
            return Err(format!("timeout de {timeout} ms"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// Focus lease + input global (SendInput) — liberado só durante o lease
// ---------------------------------------------------------------------------

static mut LEASE_UNTIL: Option<std::time::Instant> = None;

fn lease_active() -> bool {
    unsafe {
        LEASE_UNTIL
            .map(|until| until > std::time::Instant::now())
            .unwrap_or(false)
    }
}

fn require_lease() -> Result<(), String> {
    if lease_active() {
        Ok(())
    } else {
        Err(
            "sem focus lease aprovado — chame focus_lease (modal no dock) antes do input global"
                .to_string(),
        )
    }
}

fn focus_lease(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "focus_lease")?;
    let seconds = args
        .get("seconds")
        .and_then(Value::as_u64)
        .unwrap_or(15)
        .clamp(3, 120);
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("testes do agente");
    let result = dock_tool(
        "focus_lease",
        json!({ "hwnd": hwnd as usize, "seconds": seconds, "reason": reason }),
    )?;
    let granted = result
        .get("granted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if granted {
        unsafe {
            LEASE_UNTIL = Some(std::time::Instant::now() + std::time::Duration::from_secs(seconds));
        }
        Ok(vec![text_content(&format!(
            "lease concedido por {seconds} s (janela 0x{:X}); input global liberado até expirar",
            hwnd as usize
        ))])
    } else {
        Ok(vec![text_content("lease negado pelo usuário")])
    }
}

fn global_click(args: &Value) -> Result<Vec<Value>, String> {
    require_lease()?;
    let x = args.get("x").and_then(Value::as_i64).ok_or("informe x")? as i32;
    let y = args.get("y").and_then(Value::as_i64).ok_or("informe y")? as i32;
    dock_activity_if_connected("input global (mouse)");
    unsafe {
        CLICKS = CLICKS.saturating_add(1);
        SetCursorPos(x, y);
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
    crate::win::log(&format!("mcp: global_click ({x},{y})"));
    Ok(vec![text_content(&format!("clique global em ({x},{y})"))])
}

fn global_type(args: &Value) -> Result<Vec<Value>, String> {
    require_lease()?;
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or("informe text")?;
    dock_activity_if_connected("input global (teclado)");
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => press_vk(0x0D),
            '\t' => press_vk(0x09),
            other => send_unicode(other as u16),
        }
    }
    crate::win::log(&format!("mcp: global_type {} chars", text.chars().count()));
    Ok(vec![text_content(&format!(
        "digitado globalmente: {} chars",
        text.chars().count()
    ))])
}

fn global_key(args: &Value) -> Result<Vec<Value>, String> {
    require_lease()?;
    let key = args.get("key").ok_or("informe key")?;
    let vk: u16 = if let Some(code) = key.as_u64() {
        code as u16
    } else {
        match key.as_str().unwrap_or("").to_ascii_lowercase().as_str() {
            "enter" | "return" => 0x0D,
            "tab" => 0x09,
            "esc" | "escape" => 0x1B,
            "space" => 0x20,
            "backspace" => 0x08,
            "delete" | "del" => 0x2E,
            "up" => 0x26,
            "down" => 0x28,
            "left" => 0x25,
            "right" => 0x27,
            other => return Err(format!("tecla desconhecida: {other}")),
        }
    };
    press_vk(vk);
    crate::win::log(&format!("mcp: global_key 0x{vk:02X}"));
    Ok(vec![text_content(&format!("tecla global 0x{vk:02X}"))])
}

fn press_vk(vk: u16) {
    unsafe {
        keybd_event(vk as u8, 0, 0, 0);
        keybd_event(vk as u8, 0, 2, 0); // KEYEVENTF_KEYUP
    }
}

fn send_unicode(code: u16) {
    unsafe {
        let mut down: INPUT = std::mem::zeroed();
        down.r#type = INPUT_KEYBOARD;
        down.Anonymous.ki = KEYBDINPUT {
            wVk: 0,
            wScan: code,
            dwFlags: KEYEVENTF_UNICODE,
            time: 0,
            dwExtraInfo: 0,
        };
        let mut up: INPUT = std::mem::zeroed();
        up.r#type = INPUT_KEYBOARD;
        up.Anonymous.ki = KEYBDINPUT {
            wVk: 0,
            wScan: code,
            dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(2, [down, up].as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

// ---------------------------------------------------------------------------
// Ações por mensagem (WM_SETTEXT / BM_CLICK) — equivalente leve a UIA
// ---------------------------------------------------------------------------

fn set_text(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "set_text")?;
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or("informe text")?;
    dock_activity_if_connected("definindo texto");
    let wide = crate::win::wide(text);
    let result = unsafe { SendMessageW(hwnd, WM_SETTEXT, 0, wide.as_ptr() as isize) };
    crate::win::log(&format!(
        "mcp: set_text hwnd=0x{:X} {} chars (ret={result})",
        hwnd as usize,
        text.chars().count()
    ));
    Ok(vec![text_content(&format!(
        "texto definido em 0x{:X}: {text:?}",
        hwnd as usize
    ))])
}

fn control_click(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "control_click")?;
    dock_activity_if_connected("acionando controle");
    let mut method = "BM_CLICK";
    let result = unsafe { SendMessageW(hwnd, BM_CLICK, 0, 0) };
    if result == 0 {
        // Fallback: clique no centro do controle via mensagens.
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(hwnd, &mut rect);
        }
        let cx = (rect.right - rect.left) / 2;
        let cy = (rect.bottom - rect.top) / 2;
        let lparam = (((cy as u16 as u32) << 16) | (cx as u16 as u32)) as isize;
        unsafe {
            PostMessageW(hwnd, WM_LBUTTONDOWN, MK_LBUTTON, lparam);
            PostMessageW(hwnd, WM_LBUTTONUP, 0, lparam);
        }
        method = "PostMessage(center)";
    }
    unsafe {
        CLICKS = CLICKS.saturating_add(1);
    }
    crate::win::log(&format!(
        "mcp: control_click hwnd=0x{:X} via {method}",
        hwnd as usize
    ));
    Ok(vec![text_content(&format!(
        "controle 0x{:X} acionado via {method}",
        hwnd as usize
    ))])
}

// ---------------------------------------------------------------------------
// Browser (Chrome/Edge) com GPU real e anti-throttle + endpoint CDP
// ---------------------------------------------------------------------------

fn find_browser() -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            candidates.push(format!(r"{base}\Google\Chrome\Application\chrome.exe"));
            candidates.push(format!(r"{base}\Microsoft\Edge\Application\msedge.exe"));
        }
    }
    candidates.push(r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string());
    candidates.push(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe".to_string());
    candidates
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
}

/// Consulta o endpoint CDP via curl.exe (confiável a partir do sidecar).
fn cdp_probe(port: u64) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let output = Command::new("curl.exe")
        .args(["-s", "--max-time", "2", url.as_str()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("curl: {err}"))?;
    if !output.status.success() {
        return Err(format!("curl saiu com {:?}", output.status.code()));
    }
    let body = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if body.is_empty() {
        return Err("curl: resposta vazia".to_string());
    }
    Ok(body)
}

fn browser_launch(args: &Value) -> Result<Vec<Value>, String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("about:blank")
        .to_string();
    let headless = args
        .get("headless")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let port = args.get("cdp_port").and_then(Value::as_u64).unwrap_or(9222);
    let exe = find_browser().ok_or("Chrome/Edge não encontrado nesta máquina")?;
    // Profile por porta: evita conflito de "singleton" com outra instância
    // (um Chrome órfão no mesmo user-data-dir faria o novo launch sair sem a porta).
    let profile = crate::config::data_dir().join(format!("browser-profile-{port}"));
    let _ = std::fs::create_dir_all(&profile);

    let mut flags: Vec<String> = vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-renderer-backgrounding".to_string(),
        "--disable-background-timer-throttling".to_string(),
        // Chave para não estrangular rAF por "janela ocluída".
        "--disable-features=CalculateNativeWinOcclusion".to_string(),
        "--window-size=1280,800".to_string(),
    ];
    if headless {
        flags.push("--headless=new".to_string());
    } else {
        // Janela nasce fora da tela; captura via WGC continua com GPU real.
        flags.push("--window-position=-2400,-2400".to_string());
    }
    flags.push(url.clone());

    // Sidecar é subsistema GUI: NÃO herdar stdio inválido no filho (o Chrome
    // pode falhar silenciosamente se receber handles inválidos).
    let mut child = Command::new(&exe)
        .args(&flags)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("não subi {exe:?}: {err}"))?;
    let pid = child.id();
    dock_activity_if_connected(if headless {
        "abrindo browser (headless)"
    } else {
        "abrindo browser (GPU)"
    });

    // Espera o endpoint CDP ficar disponível.
    let host = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{host}/json/version");
    let mut version = String::new();
    let mut port_open = false;
    let mut last_error = String::new();
    for attempt in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if let Ok(Some(status)) = child.try_wait() {
            last_error = format!("browser saiu cedo: {status}");
            crate::win::log(&format!("mcp: cdP filho saiu cedo: {status}"));
            break;
        }
        match cdp_probe(port) {
            Ok(body) => {
                port_open = true;
                if body.contains("webSocketDebuggerUrl") || body.contains("Browser") {
                    version = body;
                    break;
                }
                last_error = format!("resposta inesperada: {}", body.lines().next().unwrap_or(""));
            }
            Err(err) => {
                last_error = err;
                if attempt == 0 {
                    crate::win::log(&format!("mcp: cdp probe (1ª tentativa): {last_error}"));
                }
            }
        }
    }
    crate::win::log(&format!(
        "mcp: browser_launch pid={pid} headless={headless} cdp={endpoint} porta_aberta={port_open} versao={}",
        if version.is_empty() { "não" } else { "sim" }
    ));
    let version_short: String = if version.is_empty() {
        format!("(CDP indisponível: {last_error})")
    } else {
        version.chars().take(400).collect()
    };
    Ok(vec![text_content(&format!(
        "browser: {exe}\npid: {pid}\nheadless: {headless}\ncdp: {endpoint} \
         (porta_aberta={port_open})\nprofile: {}\nflags: {}\n\nversão CDP: {version_short}\n\n{}",
        profile.display(),
        flags.join(" "),
        if headless {
            "headless não tem WebGPU/WebGL2 confiável; use headless=false para baseline/GPU real."
        } else {
            "headed offscreen: GPU real, janela fora da tela e sem throttle de rAF (flags acima)."
        }
    ))])
}

// ---------------------------------------------------------------------------
// UIA (ponte PowerShell — protótipo; COM nativo entra depois)
// ---------------------------------------------------------------------------

fn ps_escape(text: &str) -> String {
    text.replace('\'', "''")
}

fn run_powershell(script: &str) -> Result<String, String> {
    let encoded: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&encoded);
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            b64.as_str(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|err| format!("powershell: {err}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "powershell falhou: {}",
            err.lines().next().unwrap_or("erro desconhecido")
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn uia_query(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "uia_query")?;
    let max = args
        .get("max")
        .and_then(Value::as_u64)
        .unwrap_or(60)
        .clamp(1, 300);
    let script = format!(
        r#"
Add-Type -AssemblyName UIAutomationClient
$h = [IntPtr]{hwnd}
$root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
if ($null -eq $root) {{ '[]'; exit 0 }}
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
$out = @()
$n = [Math]::Min({max}, $all.Count)
for ($i = 0; $i -lt $n; $i++) {{
  $e = $all.Item($i)
  $r = $e.Current.BoundingRectangle
  $ctrl = $e.Current.ControlType
  $out += [pscustomobject]@{{
    name = $e.Current.Name
    type = $(if ($ctrl) {{ $ctrl.ProgrammaticName }} else {{ '' }})
    id = $e.Current.AutomationId
    enabled = $e.Current.IsEnabled
    x = [int]$r.X; y = [int]$r.Y; w = [int]$r.Width; h = [int]$r.Height
  }}
}}
$out | ConvertTo-Json -Compress
"#,
        hwnd = hwnd as usize,
        max = max
    );
    dock_activity_if_connected("consultando UIA");
    let out = run_powershell(&script)?;
    crate::win::log(&format!(
        "mcp: uia_query hwnd=0x{:X} ({} bytes)",
        hwnd as usize,
        out.len()
    ));
    Ok(vec![text_content(if out.is_empty() { "[]" } else { &out })])
}

fn uia_invoke(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = ensure_target(args, "uia_invoke")?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or("informe name")?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("invoke");
    let value = args.get("value").and_then(Value::as_str).unwrap_or("");
    let script = format!(
        r#"
Add-Type -AssemblyName UIAutomationClient
$h = [IntPtr]{hwnd}
$root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
$target = $null
for ($i = 0; $i -lt $all.Count; $i++) {{
  $e = $all.Item($i)
  if ($e.Current.Name -eq '{name}' -or $e.Current.AutomationId -eq '{name}') {{ $target = $e; break }}
}}
if ($null -eq $target) {{ 'NOT_FOUND'; exit 0 }}
if ('{action}' -eq 'set') {{
  $p = $null
  if ($target.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$p)) {{ $p.SetValue('{value}'); 'SET_OK' }} else {{ 'NO_VALUE_PATTERN' }}
}} else {{
  $p = $null
  if ($target.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$p)) {{ $p.Invoke(); 'INVOKE_OK' }} else {{ 'NO_INVOKE_PATTERN' }}
}}
"#,
        hwnd = hwnd as usize,
        name = ps_escape(name),
        action = action,
        value = ps_escape(value)
    );
    dock_activity_if_connected("acionando UIA");
    let out = run_powershell(&script)?;
    crate::win::log(&format!(
        "mcp: uia_invoke hwnd=0x{:X} name={name:?} action={action} -> {out}",
        hwnd as usize
    ));
    Ok(vec![text_content(&format!(
        "uia {action} {name:?} em 0x{:X}: {out}",
        hwnd as usize
    ))])
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use std::io::Cursor;

    fn rng(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    fn random_bytes(state: &mut u64, max_len: usize) -> Vec<u8> {
        let len = (rng(state) as usize) % (max_len + 1);
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            bytes.push((rng(state) % 256) as u8);
        }
        bytes
    }

    #[test]
    fn parse_message_line_corpus_extremos() {
        let deep = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
        let corpus = [
            "",
            "{}",
            "null",
            "[]",
            "{",
            "}",
            "\"x\"",
            "{\"id\":1,\"method\":\"initialize\"}",
            "{\"id\":\"x\",\"method\":\"ping\"}",
            "{\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"status\",\"arguments\":{}}}",
            "{\"id\":1,\"method\":\"nope\"}",
            "{\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":123}}",
            "{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":null}}}",
            "{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"\u{0}\"}}}",
            deep.as_str(),
        ];
        for text in corpus {
            let _ = parse_message_line(text);
        }
    }

    #[test]
    fn parse_message_line_fuzz_random_never_panics() {
        let mut state = 0xDEAD_BEEF_1234_5678u64;
        for _ in 0..20_000 {
            let bytes = random_bytes(&mut state, 256);
            let text = String::from_utf8_lossy(&bytes);
            let _ = parse_message_line(&text);
        }
    }

    #[test]
    fn handle_mensagens_de_controle_never_panics() {
        let mut initialized = false;
        let messages = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "desconhecido"}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "tool-inexistente", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "id": 5}),
            json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
        ];
        for message in messages {
            let response = handle(&message, "0.1.0", &mut initialized);
            let _ = serde_json::to_string(&response).unwrap();
        }
    }

    #[test]
    fn read_line_capped_behaves() {
        let mut cursor = Cursor::new(b"a\n\nb".to_vec());
        assert_eq!(
            read_line_capped(&mut cursor, 16).unwrap(),
            Some(b"a".to_vec())
        );
        assert_eq!(read_line_capped(&mut cursor, 16).unwrap(), Some(Vec::new()));
        assert_eq!(
            read_line_capped(&mut cursor, 16).unwrap(),
            Some(b"b".to_vec())
        );
        assert_eq!(read_line_capped(&mut cursor, 16).unwrap(), None);

        let mut input = vec![b'x'; 100];
        input.push(b'\n');
        input.extend_from_slice(b"ok\n");
        let mut cursor = Cursor::new(input);
        assert_eq!(read_line_capped(&mut cursor, 32).unwrap(), Some(Vec::new()));
        assert_eq!(
            read_line_capped(&mut cursor, 32).unwrap(),
            Some(b"ok".to_vec())
        );

        // Linha gigante sem newline (EOF) não explode nem cresce sem limite.
        let mut cursor = Cursor::new(vec![b'y'; 100_000]);
        assert_eq!(read_line_capped(&mut cursor, 64).unwrap(), None);
    }

    #[test]
    fn read_line_capped_fuzz_random_never_panics() {
        let mut state = 0x0BAD_C0DE_5EED_1234u64;
        for _ in 0..2_000 {
            let bytes = random_bytes(&mut state, 4096);
            let mut cursor = Cursor::new(bytes);
            let mut count = 0;
            while let Ok(Some(line)) = read_line_capped(&mut cursor, 256) {
                let _ = line.len();
                count += 1;
                assert!(count < 10_000);
            }
        }
    }
}
