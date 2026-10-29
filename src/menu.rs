//! Menu popup desenhado por nós, em janela topmost própria.
//!
//! O menu nativo do Windows vive numa faixa de z-order que fica atrás de
//! janelas topmost — e a janela encaixada no dock é topmost. Em vez de
//! esconder a janela encaixada (o que deixava o dock "em branco"), este menu
//! é uma janela topmost nossa criada depois: aparece por cima de tudo sem
//! mexer no que está embaixo.

#![allow(static_mut_refs)]

use std::ptr;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE,
    HDC, HFONT, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, LoadCursorW, PostMessageW,
    RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, IDC_ARROW,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, WM_CANCELMODE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

use crate::win;

/// Comando escolhido (wParam) enviado de volta ao dock. `0` = cancelou.
pub const WM_MENU_CHOICE: u32 = win::WM_APP + 41;

const CLASS_MENU: &str = "MayerDock.MenuWindow.v1";
const PAD: i32 = 6;
const ITEM_H: i32 = 30;
const HEADER_H: i32 = 26;
const SEP_H: i32 = 9;
const FONT_SIZE: i32 = 13;
const WIDTH: i32 = 400;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Header,
    Item,
    Sep,
}

pub struct MenuItem {
    pub label: String,
    pub cmd: usize,
    pub kind: Kind,
}

struct MenuState {
    hwnd: HWND,
    owner: HWND,
    items: Vec<MenuItem>,
    hover: Option<usize>,
    font: HFONT,
}

static mut MENU: Option<MenuState> = None;

fn menu() -> Option<&'static mut MenuState> {
    unsafe { MENU.as_mut() }
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn item_height(kind: Kind) -> i32 {
    match kind {
        Kind::Header => HEADER_H,
        Kind::Sep => SEP_H,
        Kind::Item => ITEM_H,
    }
}

pub fn register_class() {
    let class = win::wide(CLASS_MENU);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(menu_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: win::hinstance(),
        hIcon: ptr::null_mut(),
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    unsafe {
        RegisterClassW(&wc);
    }
}

/// Abre o menu ancorado perto de `anchor` (ponto em coordenadas de tela).
pub fn show(owner: HWND, anchor: POINT, items: Vec<MenuItem>) -> bool {
    if items.is_empty() || menu().is_some() {
        return false;
    }

    let (mon, _) = win::monitor_rects_at(anchor);
    let width = WIDTH.min((mon.right - mon.left) - 16).max(180);
    let total: i32 = PAD * 2 + items.iter().map(|i| item_height(i.kind)).sum::<i32>();
    let max_height = (mon.bottom - mon.top) - 16;
    let height = total.min(max_height).max(60);

    let mut x = anchor.x - width - 6;
    if x < mon.left + 4 {
        x = anchor.x + 6;
    }
    if x + width > mon.right - 4 {
        x = mon.right - 4 - width;
    }
    let mut y = anchor.y - 6;
    if y + height > mon.bottom - 4 {
        y = mon.bottom - 4 - height;
    }
    if y < mon.top + 4 {
        y = mon.top + 4;
    }

    let class = win::wide(CLASS_MENU);
    let title = win::wide("MayerDock — menu");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            x,
            y,
            width,
            height,
            ptr::null_mut(),
            ptr::null_mut(),
            win::hinstance(),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        win::log("menu: falha ao criar a janela");
        return false;
    }

    let face = win::wide("Segoe UI");
    let font = unsafe {
        CreateFontW(
            -FONT_SIZE,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        )
    };

    unsafe {
        MENU = Some(MenuState {
            hwnd,
            owner,
            items,
            hover: None,
            font,
        });
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        SetCapture(hwnd);
    }
    true
}

fn close(cmd: usize) {
    let Some(m) = (unsafe { MENU.take() }) else {
        return;
    };
    unsafe {
        ReleaseCapture();
        DestroyWindow(m.hwnd);
        DeleteObject(m.font);
        if !m.owner.is_null() {
            PostMessageW(m.owner, WM_MENU_CHOICE, cmd, 0);
        }
    }
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

fn index_at(y: i32) -> Option<usize> {
    let m = menu()?;
    let mut top = PAD;
    for (i, item) in m.items.iter().enumerate() {
        let h = item_height(item.kind);
        if y >= top && y < top + h {
            return Some(i);
        }
        top += h;
    }
    None
}

fn next_enabled(m: &MenuState, forward: bool) -> Option<usize> {
    let n = m.items.len();
    if n == 0 {
        return None;
    }
    let current = m
        .hover
        .map(|i| i as isize)
        .unwrap_or(if forward { -1 } else { n as isize });
    for step in 1..=(n as isize) {
        let idx = if forward {
            (current + step).rem_euclid(n as isize)
        } else {
            (current - step).rem_euclid(n as isize)
        } as usize;
        if m.items[idx].cmd != 0 {
            return Some(idx);
        }
    }
    None
}

unsafe fn draw_text(hdc: HDC, font: HFONT, color: u32, text: &str, rc: &mut RECT) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, color);
    SetBkMode(hdc, TRANSPARENT as i32);
    let wide = win::wide(text);
    DrawTextW(
        hdc,
        wide.as_ptr(),
        -1,
        rc,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(hdc, old);
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);

        let bg = CreateSolidBrush(rgb(26, 26, 32));
        FillRect(hdc, &rc, bg);
        DeleteObject(bg);

        let border = CreateSolidBrush(rgb(74, 78, 94));
        FillRect(hdc, &RECT { left: 0, top: 0, right: rc.right, bottom: 1 }, border);
        FillRect(hdc, &RECT { left: 0, top: rc.bottom - 1, right: rc.right, bottom: rc.bottom }, border);
        FillRect(hdc, &RECT { left: 0, top: 0, right: 1, bottom: rc.bottom }, border);
        FillRect(hdc, &RECT { left: rc.right - 1, top: 0, right: rc.right, bottom: rc.bottom }, border);
        DeleteObject(border);

        let Some(m) = menu() else {
            EndPaint(hwnd, &ps);
            return;
        };

        let mut y = PAD;
        for (index, item) in m.items.iter().enumerate() {
            let h = item_height(item.kind);
            if y + h > rc.bottom {
                break;
            }
            let row = RECT { left: 1, top: y, right: rc.right - 1, bottom: y + h };
            match item.kind {
                Kind::Header => {
                    let mut tr = RECT {
                        left: row.left + 10,
                        top: y,
                        right: row.right - 10,
                        bottom: y + h,
                    };
                    draw_text(hdc, m.font, rgb(146, 150, 168), &item.label, &mut tr);
                }
                Kind::Sep => {
                    let line = CreateSolidBrush(rgb(52, 54, 64));
                    FillRect(
                        hdc,
                        &RECT { left: row.left + 8, top: y + 4, right: row.right - 8, bottom: y + 5 },
                        line,
                    );
                    DeleteObject(line);
                }
                Kind::Item => {
                    let enabled = item.cmd != 0;
                    if enabled && m.hover == Some(index) {
                        let hl = CreateSolidBrush(rgb(50, 54, 72));
                        FillRect(hdc, &row, hl);
                        DeleteObject(hl);
                    }
                    let mut tr = RECT {
                        left: row.left + 10,
                        top: y,
                        right: row.right - 10,
                        bottom: y + h,
                    };
                    let color = if enabled {
                        rgb(232, 234, 240)
                    } else {
                        rgb(118, 122, 134)
                    };
                    draw_text(hdc, m.font, color, &item.label, &mut tr);
                }
            }
            y += h;
        }

        EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn menu_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            let (_, y) = point_of(lparam);
            let index = index_at(y);
            if let Some(m) = menu() {
                if m.hover != index {
                    m.hover = index;
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_of(lparam);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            if x < 0 || y < 0 || x >= rc.right || y >= rc.bottom {
                close(0);
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = point_of(lparam);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            if x < 0 || y < 0 || x >= rc.right || y >= rc.bottom {
                close(0);
                return 0;
            }
            match index_at(y) {
                Some(i) => {
                    let (cmd, kind) = match menu() {
                        Some(m) => (m.items[i].cmd, m.items[i].kind),
                        None => return 0,
                    };
                    match kind {
                        Kind::Item if cmd != 0 => close(cmd),
                        _ => close(0),
                    }
                }
                None => close(0),
            }
            0
        }
        WM_RBUTTONUP | WM_CANCELMODE => {
            close(0);
            0
        }
        WM_KEYDOWN => {
            let mut choice: Option<usize> = None;
            if let Some(m) = menu() {
                match wparam as u32 {
                    0x1B => choice = Some(0), // Esc
                    0x0D => {
                        // Enter
                        if let Some(i) = m.hover {
                            if m.items[i].cmd != 0 {
                                choice = Some(m.items[i].cmd);
                            }
                        }
                    }
                    0x26 => {
                        // Seta para cima
                        let next = next_enabled(&*m, false);
                        m.hover = next;
                        InvalidateRect(hwnd, ptr::null(), 0);
                    }
                    0x28 => {
                        // Seta para baixo
                        let next = next_enabled(&*m, true);
                        m.hover = next;
                        InvalidateRect(hwnd, ptr::null(), 0);
                    }
                    _ => {}
                }
            }
            if let Some(cmd) = choice {
                close(cmd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
