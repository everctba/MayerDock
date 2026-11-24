//! Sidecar **MCP** (Model Context Protocol) do MayerDock — protótipo da v1.5.
//!
//! Modo de uso: `mayerdock.exe --mcp` (servidor MCP sobre stdio, JSON-RPC 2.0,
//! uma mensagem JSON por linha).
//!
//! Esta primeira fatia é **standalone** (não conversa com o dock ainda):
//! expõe inventário de janelas, captura de janela específica (PrintWindow/WGC),
//! abertura de app e estado de janela. O próximo passo é o named pipe com o
//! dock para slots/HUD/lease.
//!
//! Regras do guia embutido (entregue ao agente em `initialize`):
//! - preferir dados estruturados (árvore/lista) a pixels;
//! - nunca usar input global (nesta fatia não há input);
//! - capturar só a janela alvo, nunca a tela inteira.

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
    GetWindowDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
    SRCCOPY,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, ShowWindow, GWL_EXSTYLE,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNOACTIVATE, WS_EX_TOOLWINDOW,
};

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
    crate::win::log("mcp: sidecar encerrado");
    0
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
            result(
                id,
                json!({
                    "protocolVersion": protocol,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mayerdock-mcp", "version": version },
                    "instructions": "MayerDock agent workspace (protótipo). Prefira dados \
                        estruturados (list_windows/capture_window) a screenshots da tela \
                        inteira. Capture somente a janela alvo. Não há input global nesta \
                        versão; tools de input/lease entram na próxima fatia (dock + pipe)."
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
        }
    ])
}

fn call_tool(name: &str, args: &Value) -> Result<Vec<Value>, String> {
    match name {
        "status" => {
            let windows = list_windows();
            let text = format!(
                "mayerdock-mcp {} (standalone)\r\npid: {}\r\njanelas visíveis: {}\r\n\
                 modo: protótipo (dock ainda não conectado; sem slots/HUD/lease)",
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
        _ => Err(format!("tool desconhecida: {name}")),
    }
}

fn text_content(text: &str) -> Value {
    json!({ "type": "text", "text": text })
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
        "aberto sem ativar: {path} {arguments} (encaixe em slot do dock entra na próxima fatia)"
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
