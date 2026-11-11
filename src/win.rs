//! Utilitários Win32 e de arquivo de log do MayerDock.

use std::path::PathBuf;
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    APPMODEL_ERROR_NO_PACKAGE, CloseHandle, HINSTANCE, HWND, POINT, RECT,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, WriteFile};
use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// Classe da janela do dock.
pub const CLASS_DOCK: &str = "MayerDock.DockWindow.v1";

/// Base das mensagens privadas (`WM_APP`).
pub const WM_APP: u32 = 0x8000;

/// String Rust → buffer UTF-16 terminado em NUL.
pub fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// HINSTANCE do executável atual.
pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(ptr::null()) }
}

fn monitor_info(mon: HMONITOR) -> (RECT, RECT) {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) != 0 {
            (mi.rcMonitor, mi.rcWork)
        } else {
            (
                RECT { left: 0, top: 0, right: 1920, bottom: 1080 },
                RECT { left: 0, top: 0, right: 1920, bottom: 1040 },
            )
        }
    }
}

/// Retângulo (físico) e área de trabalho do monitor primário.
pub fn primary_monitor_rects() -> (RECT, RECT) {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        monitor_info(mon)
    }
}

/// Retângulo (físico) e área de trabalho do monitor que contém a janela.
pub fn monitor_rects(hwnd: HWND) -> (RECT, RECT) {
    unsafe { monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)) }
}

/// Retângulo (físico) e área de trabalho do monitor que contém o ponto.
pub fn monitor_rects_at(pt: POINT) -> (RECT, RECT) {
    unsafe { monitor_info(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)) }
}

/// Caminho do log.
pub fn log_path() -> PathBuf {
    crate::config::data_dir().join("mayerdock.log")
}

/// Nome completo do pacote MSIX, se o processo estiver empacotado.
///
/// Fora de um pacote retorna `None`. No modo MSIX os arquivos em `%APPDATA%`
/// são redirecionados para a área do pacote — este é o teste usado para
/// detectar o modo empacotado (`--status`, log e decisões de UI).
pub fn package_full_name() -> Option<String> {
    unsafe {
        let mut length: u32 = 0;
        let rc = GetCurrentPackageFullName(&mut length, ptr::null_mut());
        if rc == APPMODEL_ERROR_NO_PACKAGE || length == 0 {
            return None;
        }
        // Esperado: ERROR_INSUFFICIENT_BUFFER (122) com o tamanho necessário.
        let mut buffer: Vec<u16> = vec![0; length as usize];
        let mut length2 = length;
        let rc = GetCurrentPackageFullName(&mut length2, buffer.as_mut_ptr());
        if rc != 0 {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

/// Escreve texto no console do processo pai (para `--status`).
///
/// Funciona com stdout redirecionado (pipe/arquivo) e, quando não há redireção
/// (app GUI), anexa ao console do cmd/PowerShell que o iniciou.
pub fn console_print(text: &str) {
    use std::io::Write;
    // Normaliza para CRLF sem duplicar (texto pode já vir com \r\n).
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let bytes: Vec<u8> = normalized.into_bytes();

    {
        let mut out = std::io::stdout();
        if out.write_all(&bytes).and_then(|_| out.flush()).is_ok() {
            return;
        }
    }

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let name = wide("CONOUT$");
        let handle = CreateFileW(
            name.as_ptr(),
            0x4000_0000 | 0x8000_0000, // GENERIC_WRITE | GENERIC_READ
            0x0000_0001 | 0x0000_0002, // FILE_SHARE_READ | FILE_SHARE_WRITE
            ptr::null(),
            3, // OPEN_EXISTING
            0,
            ptr::null_mut(),
        );
        if handle.is_null() || handle as isize == -1 {
            return;
        }
        let mut written: u32 = 0;
        WriteFile(
            handle,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        );
        CloseHandle(handle);
    }
}

/// Acrescenta uma linha ao log (ignora falhas de disco).
pub fn log(msg: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{ts}] {msg}\r\n");
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}
