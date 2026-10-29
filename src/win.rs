//! Utilitários Win32 e de arquivo de log do MayerDock.

use std::path::PathBuf;
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY, MONITOR_DEFAULTTONEAREST,
};
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
