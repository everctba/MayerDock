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

use std::ffi::c_void;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
    GetWindowDC, ReleaseDC, ScreenToClient, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, mouse_event, IsWindowEnabled, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetDlgCtrlID, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    PostMessageW, SendMessageW, SetCursorPos, ShowWindow, GWL_EXSTYLE, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOWNOACTIVATE, WM_CHAR, WM_GETTEXT, WM_GETTEXTLENGTH, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WS_EX_TOOLWINDOW,
};

/// `MK_LBUTTON` (windows-sys não expõe direto no módulo de mensagens).
const MK_LBUTTON: usize = 1;

/// Contadores exibidos no HUD (cliques e capturas desta sessão).
static mut CLICKS: u32 = 0;
static mut CAPTURES: u32 = 0;

/// `PW_RENDERFULLCONTENT` (windows-sys não expõe a constante).
const PW_RENDERFULLCONTENT: u32 = 2;

pub fn serve(version: &str) -> i32 {
    crate::win::log("mcp: sidecar iniciado (stdio)");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut initialized = false;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
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
                    "title": { "type": "string", "description": "Trecho do título (case-insensitive)" }
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
    list
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

fn capture_window(args: &Value) -> Result<Vec<Value>, String> {
    let hwnd = find_window(args)?;
    let (png_bytes, width, height, dpi) = capture(hwnd)?;

    let dir = capture_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("não criei {:?}: {e}", dir))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let file = dir.join(format!("cap-{stamp}.png"));
    std::fs::write(&file, &png_bytes).map_err(|e| format!("não escrevi {:?}: {e}", file))?;

    let title = window_title(hwnd);
    let text = format!(
        "capturada: {title:?}\r\narquivo: {}\r\n{width}x{height} px dpi={:.0}%\r\nbytes: {}",
        file.display(),
        dpi * 100.0,
        png_bytes.len()
    );
    let data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    unsafe {
        CAPTURES = CAPTURES.saturating_add(1);
    }
    dock_activity_if_connected("capturando");
    crate::win::log(&format!(
        "mcp: captura hwnd=0x{:X} {}x{} {} bytes",
        hwnd as usize,
        width,
        height,
        png_bytes.len()
    ));
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

fn capture(hwnd: HWND) -> Result<(Vec<u8>, u32, u32, f64), String> {
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
        let printed = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT);
        if printed == 0 {
            BitBlt(mem_dc, 0, 0, width, height, window_dc, 0, 0, SRCCOPY);
        }

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
        Ok((png_bytes, width as u32, height as u32, dpi))
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
    let hwnd = find_window(args)?;
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
