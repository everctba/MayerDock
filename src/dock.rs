//! Janela do dock: AppBar na borda direita, um slot com janela encaixada,
//! esconder em fullscreen e atalho global.
//!
//! O app é single-thread (loop de mensagens Win32), então o estado vive num
//! `static mut` acessado somente pela thread da interface.

#![allow(static_mut_refs)]

use std::mem::size_of;
use std::ptr;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, HDC, HFONT, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::Shell::{
    SHAppBarMessage, SHQueryUserNotificationState, Shell_NotifyIconW, ABE_RIGHT, ABM_NEW,
    ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABN_FULLSCREENAPP, ABN_POSCHANGED, ABN_STATECHANGE,
    APPBARDATA, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowW,
    GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowLongW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, IsWindowVisible, KillTimer, LoadCursorW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, IDC_SIZEWE, IDI_APPLICATION,
    LoadIconW, MSG, SW_HIDE, SW_RESTORE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_HOTKEY,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use crate::config::Config;
use crate::menu;
use crate::win;

const HEADER_H: i32 = 40;
const PAD: i32 = 2;
const EDGE_GRAB: i32 = 6;
const MIN_W: i32 = 240;
const MAX_W: i32 = 1200;
const TIMER_TICK: usize = 1;
const HOTKEY_ID: i32 = 1;
const VK_M: u32 = 0x4D;

/// Callback do AppBar (recebido na janela do dock).
const WM_APPBAR: u32 = win::WM_APP + 10;
/// Pedido de outra instância para mostrar o dock.
const WM_SHOWME: u32 = win::WM_APP + 30;
/// Mudança de janela em primeiro plano (hook de acessibilidade).
const WM_FOREGROUND: u32 = win::WM_APP + 50;
/// Eventos do ícone da bandeja.
const WM_TRAY: u32 = win::WM_APP + 60;

static mut HOOK: HWINEVENTHOOK = std::ptr::null_mut();

const IDM_EXIT: usize = 101;
const IDM_TOGGLE: usize = 102;
/// "Divisões: 1/2/3" → 110..112.
const IDM_SLOTS_BASE: usize = 110;
const IDM_SLOTS_LAST: usize = 112;
/// "Liberar slot i" → 120..122.
const IDM_RELEASE_BASE: usize = 120;
const IDM_RELEASE_LAST: usize = 122;
const IDM_WINDOW_BASE: usize = 1000;

#[derive(Clone, Copy)]
struct Slot {
    hwnd: HWND,
    origin: RECT,
    valid_origin: bool,
}

struct State {
    hwnd: HWND,
    abd: APPBARDATA,
    registered: bool,
    busy: bool,
    width: i32,
    hidden: bool,
    manual_hidden: bool,
    /// Menu popup aberto: não re-promover a janela encaixada (cobriria o menu).
    menu_open: bool,
    resizing: bool,
    resize_start_x: i32,
    resize_start_w: i32,
    slot_count: usize,
    slots: Vec<Option<Slot>>,
    picking: Vec<HWND>,
    btn_menu: RECT,
    btn_close: RECT,
    btn_release: RECT,
    font_title: HFONT,
    font_ui: HFONT,
    font_hint: HFONT,
}

static mut STATE: Option<State> = None;

fn state_opt() -> Option<&'static mut State> {
    unsafe { STATE.as_mut() }
}

fn st() -> &'static mut State {
    state_opt().expect("MayerDock: estado não inicializado")
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn make_font(size: i32, weight: i32) -> HFONT {
    let face = win::wide("Segoe UI");
    unsafe {
        CreateFontW(
            -size,
            0,
            0,
            0,
            weight,
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
    }
}

impl State {
    fn new(hwnd: HWND, width: i32, slot_count: usize) -> State {
        let slot_count = slot_count.clamp(1, 3);
        State {
            hwnd,
            abd: APPBARDATA::default(),
            registered: false,
            busy: false,
            width,
            hidden: false,
            manual_hidden: false,
            menu_open: false,
            resizing: false,
            resize_start_x: 0,
            resize_start_w: width,
            slot_count,
            slots: vec![None; slot_count],
            picking: Vec::new(),
            btn_menu: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            btn_close: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            btn_release: RECT { left: 0, top: 0, right: 0, bottom: 0 },
            font_title: make_font(15, 600),
            font_ui: make_font(13, 400),
            font_hint: make_font(13, 400),
        }
    }
}

/// Ponto de entrada: cria o dock, registra o AppBar e roda o loop de mensagens.
pub fn run(cfg: Config) -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    register_class();
    menu::register_class();

    let class = win::wide(win::CLASS_DOCK);
    let existing = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    if !existing.is_null() {
        unsafe {
            PostMessageW(existing, WM_SHOWME, 0, 0);
        }
        win::log("instância já em execução; pedindo para mostrar");
        return 0;
    }

    let width = cfg.width.clamp(MIN_W, MAX_W);
    let (mon, _work) = win::primary_monitor_rects();
    let height = (mon.bottom - mon.top).max(200);
    let title = win::wide("MayerDock");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            mon.right - width,
            mon.top,
            width,
            height,
            ptr::null_mut(),
            ptr::null_mut(),
            win::hinstance(),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        let err = unsafe { GetLastError() };
        win::log(&format!("CreateWindowExW falhou (erro {err})"));
        return 1;
    }

    unsafe {
        STATE = Some(State::new(hwnd, width, cfg.slots));
        let s = st();

        register_appbar(s);
        if !s.registered {
            win::log("aviso: ABM_NEW falhou (shell não respondeu?)");
        }
        apply_appbar(s);

        let hotkey = RegisterHotKey(
            hwnd,
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_M,
        );
        if hotkey == 0 {
            win::log("aviso: Ctrl+Alt+M não registrado (combinação em uso?)");
        }

        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
        SetTimer(hwnd, TIMER_TICK, 1000, None);
        tray_add(s);

        HOOK = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            ptr::null_mut(),
            Some(foreground_hook),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        if HOOK.is_null() {
            win::log("aviso: SetWinEventHook falhou (dock pode não sair da frente do Start)");
        }

        win::log(&format!(
            "dock iniciado: largura={width} divisões={} monitor={}x{}",
            s.slot_count,
            mon.right - mon.left,
            mon.bottom - mon.top
        ));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        STATE = None;
    }

    0
}

fn register_class() {
    let class = win::wide(win::CLASS_DOCK);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(dock_proc),
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

// ---------------------------------------------------------------------------
// AppBar
// ---------------------------------------------------------------------------

fn register_appbar(s: &mut State) {
    let mut abd = APPBARDATA::default();
    abd.cbSize = size_of::<APPBARDATA>() as u32;
    abd.hWnd = s.hwnd;
    abd.uCallbackMessage = WM_APPBAR;
    let ok = unsafe { SHAppBarMessage(ABM_NEW, &mut abd) };
    s.abd = abd;
    s.registered = ok != 0;
}

fn remove_appbar(s: &mut State) {
    if !s.registered {
        return;
    }
    let mut abd = s.abd;
    unsafe {
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
    s.registered = false;
    win::log("appbar removida (área de trabalho restaurada)");
}

/// Recalcula a faixa reservada e move a janela para ela.
fn apply_appbar(s: &mut State) {
    if !s.registered || s.busy {
        return;
    }
    s.busy = true;

    let (mon, _work) = win::monitor_rects(s.hwnd);
    let mut abd = s.abd;
    abd.uEdge = ABE_RIGHT;
    abd.rc = RECT {
        left: mon.right - s.width,
        top: mon.top,
        right: mon.right,
        bottom: mon.bottom,
    };

    let queried = unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut abd) };
    // O shell pode ter afastado a faixa da taskbar; preserva a largura pedida.
    abd.rc.left = abd.rc.right - s.width;
    if abd.rc.left < mon.left {
        abd.rc.left = mon.left;
    }
    let applied = unsafe { SHAppBarMessage(ABM_SETPOS, &mut abd) };
    s.abd = abd;
    if queried == 0 || applied == 0 {
        win::log(&format!(
            "appbar: falha (query={queried} set={applied}) rc=({},{},{},{})",
            abd.rc.left, abd.rc.top, abd.rc.right, abd.rc.bottom
        ));
    }

    let r = abd.rc;
    unsafe {
        SetWindowPos(
            s.hwnd,
            HWND_TOPMOST,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE,
        );
    }

    s.busy = false;
    sync_slot(s, true);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

// ---------------------------------------------------------------------------
// Slot (janela encaixada)
// ---------------------------------------------------------------------------

fn content_rect(s: &State) -> RECT {
    let r = s.abd.rc;
    RECT {
        left: r.left + PAD,
        top: r.top + HEADER_H,
        right: (r.right - PAD).max(r.left + PAD + 80),
        bottom: (r.bottom - PAD).max(r.top + HEADER_H + 80),
    }
}

/// Retângulo da divisão `index` (empilhamento vertical, 1–3).
fn slot_rect(s: &State, index: usize) -> RECT {
    let c = content_rect(s);
    let n = s.slot_count.max(1) as i32;
    let gap = 4;
    let total = (c.bottom - c.top).max(n);
    let each = ((total - gap * (n - 1)) / n).max(40);
    let top = c.top + index as i32 * (each + gap);
    RECT {
        left: c.left,
        top,
        right: c.right,
        bottom: top + each,
    }
}

fn slot_label(index: usize, count: usize) -> &'static str {
    match (count, index) {
        (1, _) => "única",
        (2, 0) => "cima",
        (2, _) => "baixo",
        (3, 0) => "cima",
        (3, 1) => "meio",
        _ => "baixo",
    }
}

fn any_slot_occupied(s: &State) -> bool {
    s.slots.iter().any(|slot| slot.is_some())
}

fn slot_process(s: &State, index: usize) -> String {
    match &s.slots[index] {
        Some(slot) => {
            let name = process_label(slot.hwnd);
            if name.is_empty() {
                "janela".to_string()
            } else {
                name
            }
        }
        None => String::new(),
    }
}

fn sync_slot(s: &mut State, force: bool) {
    // O timer de 1 s roda durante o loop modal do menu; se re-promovêssemos as
    // janelas agora, elas cobririam o menu de novo.
    if s.hidden || s.menu_open || s.resizing {
        return;
    }
    for index in 0..s.slot_count {
        let hwnd = match &s.slots[index] {
            Some(slot) => slot.hwnd,
            None => continue,
        };
        if unsafe { IsWindow(hwnd) } == 0 {
            win::log(&format!("janela do slot {index} fechou; liberando"));
            s.slots[index] = None;
            unsafe {
                InvalidateRect(s.hwnd, ptr::null(), 0);
            }
            continue;
        }

        let target = slot_rect(s, index);
        let mut current = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            GetWindowRect(hwnd, &mut current);
        }
        let same = current.left == target.left
            && current.top == target.top
            && current.right == target.right
            && current.bottom == target.bottom;

        let flags = if same && !force {
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE
        } else {
            SWP_SHOWWINDOW
        };
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                target.left,
                target.top,
                target.right - target.left,
                target.bottom - target.top,
                flags,
            );
        }
    }
}

/// Coloca a janela na primeira divisão livre.
fn pin_first_free(s: &mut State, hwnd: HWND) -> bool {
    if hwnd.is_null() || hwnd == s.hwnd {
        return false;
    }
    if s.slots.iter().flatten().any(|slot| slot.hwnd == hwnd) {
        return false;
    }
    let free = match s.slots.iter().position(|slot| slot.is_none()) {
        Some(index) => index,
        None => {
            win::log("todas as divisões ocupadas; libere uma janela antes");
            return false;
        }
    };
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
    }
    let mut origin = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe { GetWindowRect(hwnd, &mut origin) } != 0;
    s.slots[free] = Some(Slot {
        hwnd,
        origin,
        valid_origin: ok,
    });
    sync_slot(s, true);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log(&format!("janela encaixada na divisão {free}"));
    true
}

fn release_slot(s: &mut State, index: usize) {
    if index >= s.slot_count {
        return;
    }
    let Some(slot) = s.slots[index].take() else {
        return;
    };
    unsafe {
        if IsWindow(slot.hwnd) != 0 {
            if slot.valid_origin {
                let w = slot.origin.right - slot.origin.left;
                let h = slot.origin.bottom - slot.origin.top;
                if w > 0 && h > 0 {
                    SetWindowPos(
                        slot.hwnd,
                        HWND_NOTOPMOST,
                        slot.origin.left,
                        slot.origin.top,
                        w,
                        h,
                        SWP_NOACTIVATE,
                    );
                }
            } else {
                SetWindowPos(
                    slot.hwnd,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log(&format!("divisão {index} liberada"));
}

fn release_all(s: &mut State) {
    for index in 0..s.slot_count {
        release_slot(s, index);
    }
}

fn set_slot_count(s: &mut State, count: usize) {
    let count = count.clamp(1, 3);
    if count == s.slot_count {
        return;
    }
    if count < s.slot_count {
        for index in count..s.slot_count {
            release_slot(s, index);
        }
    }
    s.slot_count = count;
    s.slots.resize(count, None);
    sync_slot(s, true);
    Config {
        width: s.width,
        slots: s.slot_count,
    }
    .save();
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log(&format!("divisões = {count}"));
}

// ---------------------------------------------------------------------------
// Visibilidade / fullscreen
// ---------------------------------------------------------------------------

fn hide_dock(s: &mut State) {
    if s.hidden {
        return;
    }
    s.hidden = true;
    remove_appbar(s);
    unsafe {
        ShowWindow(s.hwnd, SW_HIDE);
        for slot in s.slots.iter().flatten() {
            ShowWindow(slot.hwnd, SW_HIDE);
        }
    }
    win::log("dock oculto");
}

fn show_dock(s: &mut State) {
    if !s.hidden {
        return;
    }
    s.hidden = false;
    register_appbar(s);
    apply_appbar(s);
    unsafe {
        ShowWindow(s.hwnd, SW_SHOWNOACTIVATE);
        for slot in s.slots.iter().flatten() {
            ShowWindow(slot.hwnd, SW_SHOWNOACTIVATE);
        }
    }
    sync_slot(s, true);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log("dock visível");
}

fn update_visibility(s: &mut State) {
    // Start/pesquisa/Task View/flyouts do shell: oculta o dock e libera a
    // área reservada — assim o painel abre no tamanho cheio. Ao fechar,
    // o dock volta com a reserva.
    let want_hidden = is_fullscreen(s) || is_shell_flyout() || s.manual_hidden;
    if want_hidden && !s.hidden {
        hide_dock(s);
    } else if !want_hidden && s.hidden {
        show_dock(s);
    }
}

fn is_fullscreen(s: &State) -> bool {
    unsafe {
        let mut q: i32 = 0;
        if SHQueryUserNotificationState(&mut q) == 0 && q == QUNS_RUNNING_D3D_FULL_SCREEN {
            return true;
        }

        let fg = GetForegroundWindow();
        if fg.is_null() || fg == s.hwnd {
            return false;
        }
        if s.slots.iter().flatten().any(|slot| slot.hwnd == fg) {
            return false;
        }
        let class = class_name(fg);
        if is_shell_class(&class) {
            return false;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(fg, &mut r) == 0 {
            return false;
        }
        let (mon, _) = win::monitor_rects(fg);
        r.left <= mon.left + 2
            && r.top <= mon.top + 2
            && r.right >= mon.right - 2
            && r.bottom >= mon.bottom - 2
    }
}

fn is_shell_process(process: &str) -> bool {
    matches!(
        process.to_lowercase().as_str(),
        "startmenuexperiencehost"
            | "searchhost"
            | "searchapp"
            | "searchui"
            | "cortana"
            | "shellexperiencehost"
    )
}

/// Start / pesquisa / Task View / flyouts do shell estão em primeiro plano?
///
/// Só o foreground: procurar janelas visíveis desses processos dava falso
/// positivo (janelas residuais do shell ficam "visíveis" e prendiam o dock).
fn is_shell_flyout() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        if class_name(fg) == "XamlExplorerHostIslandWindow" {
            return true;
        }
        is_shell_process(&process_label(fg))
    }
}

// ---------------------------------------------------------------------------
// Pintura
// ---------------------------------------------------------------------------

fn paint(hwnd: HWND, s: &mut State) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;

        let bg = CreateSolidBrush(rgb(18, 18, 22));
        FillRect(hdc, &rc, bg);
        DeleteObject(bg);

        let hdr = RECT { left: 0, top: 0, right: w, bottom: HEADER_H };
        let hb = CreateSolidBrush(rgb(30, 30, 36));
        FillRect(hdc, &hdr, hb);
        DeleteObject(hb);

        let edge = RECT { left: 0, top: 0, right: 2, bottom: h };
        let eb = CreateSolidBrush(rgb(74, 112, 240));
        FillRect(hdc, &edge, eb);
        DeleteObject(eb);

        let mut title = RECT {
            left: 14,
            top: 0,
            right: (w - 136).max(40),
            bottom: HEADER_H,
        };
        draw_text(
            hdc,
            s.font_title,
            rgb(236, 238, 244),
            "MayerDock",
            &mut title,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        let close = RECT { left: w - 38, top: 7, right: w - 6, bottom: HEADER_H - 7 };
        let menu_b = RECT {
            left: close.left - 40,
            top: 7,
            right: close.left - 6,
            bottom: HEADER_H - 7,
        };
        let release = RECT {
            left: menu_b.left - 40,
            top: 7,
            right: menu_b.left - 6,
            bottom: HEADER_H - 7,
        };
        draw_button(hdc, s.font_ui, &release, "←", any_slot_occupied(s));
        draw_button(hdc, s.font_ui, &menu_b, "☰", true);
        draw_button(hdc, s.font_ui, &close, "✕", true);
        s.btn_menu = menu_b;
        s.btn_close = close;
        s.btn_release = release;

        // Separadores entre as divisões verticais.
        if s.slot_count > 1 {
            let line = CreateSolidBrush(rgb(62, 66, 82));
            for index in 1..s.slot_count {
                let r = slot_rect(s, index);
                FillRect(
                    hdc,
                    &RECT {
                        left: r.left + 4,
                        top: r.top - 4,
                        right: r.right - 4,
                        bottom: r.top - 2,
                    },
                    line,
                );
            }
            DeleteObject(line);
        }

        if !any_slot_occupied(s) {
            let mut hint = RECT {
                left: 14,
                top: HEADER_H + 24,
                right: w - 14,
                bottom: h - 24,
            };
            draw_text(
                hdc,
                s.font_hint,
                rgb(146, 150, 164),
                "Nenhuma janela encaixada.\r\n\r\nClique aqui ou no ☰\r\npara escolher uma janela\r\ne ocupar o dock.",
                &mut hint,
                DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
            );
        }

        EndPaint(hwnd, &ps);
    }
}

unsafe fn draw_text(hdc: HDC, font: HFONT, color: u32, text: &str, rc: &mut RECT, format: u32) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, color);
    SetBkMode(hdc, TRANSPARENT as i32);
    let wide = win::wide(text);
    DrawTextW(hdc, wide.as_ptr(), -1, rc, format);
    SelectObject(hdc, old);
}

unsafe fn draw_button(hdc: HDC, font: HFONT, rc: &RECT, label: &str, enabled: bool) {
    let bg = if enabled { rgb(44, 44, 54) } else { rgb(30, 30, 36) };
    let brush = CreateSolidBrush(bg);
    FillRect(hdc, rc, brush);
    DeleteObject(brush);
    let mut text_rc = *rc;
    let color = if enabled { rgb(210, 214, 226) } else { rgb(92, 96, 108) };
    draw_text(
        hdc,
        font,
        color,
        label,
        &mut text_rc,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

// ---------------------------------------------------------------------------
// Mouse / cursores
// ---------------------------------------------------------------------------

fn client_point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

fn on_lbutton_down(s: &mut State, lparam: LPARAM) {
    // Só o arrasto da borda esquerda começa no botão pressionado; cliques em
    // botões/menu são tratados no WM_LBUTTONUP (senão o próprio LBUTTONUP do
    // clique dispensa o menu que acabou de abrir).
    let (x, _y) = client_point(lparam);
    if x <= EDGE_GRAB {
        s.resizing = true;
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut p);
            SetCapture(s.hwnd);
        }
        s.resize_start_x = p.x;
        s.resize_start_w = s.width;
    }
}

fn on_lbutton_up(s: &mut State, lparam: LPARAM) {
    if s.resizing {
        s.resizing = false;
        unsafe {
            ReleaseCapture();
        }
        Config {
            width: s.width,
            slots: s.slot_count,
        }
        .save();
        win::log(&format!("largura salva: {}", s.width));
        return;
    }

    let (x, y) = client_point(lparam);
    if y < HEADER_H {
        if point_in(&s.btn_close, x, y) {
            unsafe {
                PostMessageW(s.hwnd, WM_CLOSE, 0, 0);
            }
            return;
        }
        if point_in(&s.btn_menu, x, y) {
            open_menu(s);
            return;
        }
        if point_in(&s.btn_release, x, y) {
            if any_slot_occupied(s) {
                release_all(s);
                win::log("atalho ←: janelas liberadas");
            }
            return;
        }
    } else if !any_slot_occupied(s) {
        open_menu(s);
    }
}

fn on_mouse_move(s: &mut State) {
    if !s.resizing {
        return;
    }
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut p);
    }
    let dx = s.resize_start_x - p.x;
    let new_width = (s.resize_start_w + dx).clamp(MIN_W, MAX_W);
    if new_width != s.width {
        s.width = new_width;
        apply_appbar(s);
    }
}

fn set_cursor(hwnd: HWND) {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let cursor = if p.x - rc.left <= EDGE_GRAB {
            LoadCursorW(ptr::null_mut(), IDC_SIZEWE)
        } else {
            LoadCursorW(ptr::null_mut(), IDC_ARROW)
        };
        SetCursor(cursor);
    }
}

fn point_in(rc: &RECT, x: i32, y: i32) -> bool {
    x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom
}

// ---------------------------------------------------------------------------
// Menu
// ---------------------------------------------------------------------------

fn open_menu(s: &mut State) {
    let list = enumerate_windows();
    win::log(&format!("menu: {} janelas candidatas", list.len()));
    s.picking = list.iter().map(|(hwnd, _)| *hwnd).collect();

    let mut items: Vec<menu::MenuItem> = Vec::with_capacity(list.len() + 12);

    items.push(menu::MenuItem {
        label: "Encaixar janela".to_string(),
        cmd: 0,
        kind: menu::Kind::Header,
    });
    if list.is_empty() {
        items.push(menu::MenuItem {
            label: "(nenhuma janela encontrada)".to_string(),
            cmd: 0,
            kind: menu::Kind::Item,
        });
    } else {
        for (index, (_, label)) in list.iter().enumerate() {
            items.push(menu::MenuItem {
                label: truncate_title(label, 52),
                cmd: IDM_WINDOW_BASE + index,
                kind: menu::Kind::Item,
            });
        }
    }

    items.push(menu::MenuItem {
        label: String::new(),
        cmd: 0,
        kind: menu::Kind::Sep,
    });
    items.push(menu::MenuItem {
        label: "Divisões".to_string(),
        cmd: 0,
        kind: menu::Kind::Header,
    });
    for n in 1..=3usize {
        let mark = if n == s.slot_count { "• " } else { "   " };
        let label = format!("{mark}{n} janela{}", if n > 1 { "s" } else { "" });
        items.push(menu::MenuItem {
            label,
            cmd: IDM_SLOTS_BASE + (n - 1),
            kind: menu::Kind::Item,
        });
    }

    let occupied: Vec<usize> = (0..s.slot_count)
        .filter(|index| s.slots[*index].is_some())
        .collect();
    if !occupied.is_empty() {
        items.push(menu::MenuItem {
            label: String::new(),
            cmd: 0,
            kind: menu::Kind::Sep,
        });
        items.push(menu::MenuItem {
            label: "Liberar".to_string(),
            cmd: 0,
            kind: menu::Kind::Header,
        });
        for index in occupied {
            let label = format!(
                "Liberar: {} — {}",
                truncate_title(&slot_process(s, index), 28),
                slot_label(index, s.slot_count)
            );
            items.push(menu::MenuItem {
                label,
                cmd: IDM_RELEASE_BASE + index,
                kind: menu::Kind::Item,
            });
        }
    }

    items.push(menu::MenuItem {
        label: String::new(),
        cmd: 0,
        kind: menu::Kind::Sep,
    });
    items.push(menu::MenuItem {
        label: "Sair do MayerDock".to_string(),
        cmd: IDM_EXIT,
        kind: menu::Kind::Item,
    });

    // Ancora no botão ☰: o menu abre à esquerda do dock.
    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        GetWindowRect(s.hwnd, &mut wr);
    }
    let anchor = POINT {
        x: wr.left + s.btn_menu.left,
        y: wr.top + s.btn_menu.top,
    };
    s.menu_open = menu::show(s.hwnd, anchor, items);
    if !s.menu_open {
        win::log("menu: não foi possível abrir");
    }
}

fn on_menu_command(s: &mut State, id: usize) {
    match id {
        IDM_EXIT => unsafe {
            PostMessageW(s.hwnd, WM_CLOSE, 0, 0);
        },
        IDM_TOGGLE => {
            s.manual_hidden = !s.manual_hidden;
            update_visibility(s);
        }
        IDM_SLOTS_BASE..=IDM_SLOTS_LAST => set_slot_count(s, id - IDM_SLOTS_BASE + 1),
        IDM_RELEASE_BASE..=IDM_RELEASE_LAST => release_slot(s, id - IDM_RELEASE_BASE),
        _ if id >= IDM_WINDOW_BASE => {
            if let Some(hwnd) = s.picking.get(id - IDM_WINDOW_BASE).copied() {
                pin_first_free(s, hwnd);
            }
        }
        _ => {}
    }
}

fn truncate_title(title: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in title.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// Enumeração de janelas
// ---------------------------------------------------------------------------

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Progman"
            | "WorkerW"
            | "TrayNotifyWnd"
            | "Windows.UI.Core.CoreWindow"
    )
}

fn window_title(hwnd: HWND) -> Option<String> {
    unsafe {
        if IsWindow(hwnd) == 0 || IsWindowVisible(hwnd) == 0 {
            return None;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW != 0 {
            return None;
        }
        let class = class_name(hwnd);
        if class == win::CLASS_DOCK || is_shell_class(&class) {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// Nome do processo (sem `.exe`) dono da janela, para o menu ficar legível.
fn process_label(hwnd: HWND) -> String {
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
        let mut buf = [0u16; 512];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size);
        CloseHandle(process);
        if ok == 0 {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Rótulo do menu: "Processo — Título da janela".
fn window_label(hwnd: HWND, title: &str) -> String {
    let process = process_label(hwnd);
    if process.is_empty() {
        title.to_string()
    } else {
        format!("{process} — {title}")
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam as *mut Vec<(HWND, String)>);
    if let Some(title) = window_title(hwnd) {
        list.push((hwnd, window_label(hwnd, &title)));
    }
    1
}

fn enumerate_windows() -> Vec<(HWND, String)> {
    let mut list: Vec<(HWND, String)> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc), &mut list as *mut _ as LPARAM);
    }
    list.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    list
}

// ---------------------------------------------------------------------------
// Encerramento
// ---------------------------------------------------------------------------

fn cleanup(s: &mut State) {
    unsafe {
        KillTimer(s.hwnd, TIMER_TICK);
        UnregisterHotKey(s.hwnd, HOTKEY_ID);
        if !HOOK.is_null() {
            UnhookWinEvent(HOOK);
            HOOK = ptr::null_mut();
        }
    }
    release_all(s);
    remove_appbar(s);
    tray_remove(s);
    Config {
        width: s.width,
        slots: s.slot_count,
    }
    .save();
    unsafe {
        DeleteObject(s.font_title);
        DeleteObject(s.font_ui);
        DeleteObject(s.font_hint);
    }
    win::log("dock encerrado");
}

// ---------------------------------------------------------------------------
// Ícone da bandeja
// ---------------------------------------------------------------------------

fn copy_wide(dst: &mut [u16], text: &str) {
    for b in dst.iter_mut() {
        *b = 0;
    }
    for (i, ch) in text.encode_utf16().enumerate() {
        if i + 1 >= dst.len() {
            break;
        }
        dst[i] = ch;
    }
}

fn tray_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

fn tray_add(s: &State) {
    let mut nid = tray_nid(s.hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = unsafe { LoadIconW(ptr::null_mut(), IDI_APPLICATION) };
    copy_wide(&mut nid.szTip, "MayerDock — dock lateral");
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
    if ok == 0 {
        win::log("aviso: ícone da bandeja não foi criado");
    } else {
        win::log("ícone da bandeja criado");
    }
}

fn tray_remove(s: &State) {
    let nid = tray_nid(s.hwnd);
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn tray_menu(s: &mut State) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut p);
    }
    let mut items = Vec::new();
    items.push(menu::MenuItem {
        label: "Mostrar/ocultar dock".to_string(),
        cmd: IDM_TOGGLE,
        kind: menu::Kind::Item,
    });
    items.push(menu::MenuItem {
        label: String::new(),
        cmd: 0,
        kind: menu::Kind::Sep,
    });
    items.push(menu::MenuItem {
        label: "Sair do MayerDock".to_string(),
        cmd: IDM_EXIT,
        kind: menu::Kind::Item,
    });
    s.menu_open = menu::show(s.hwnd, p, items);
    if !s.menu_open {
        win::log("menu: não foi possível abrir (bandeja)");
    }
}

// ---------------------------------------------------------------------------
// Window proc
// ---------------------------------------------------------------------------

/// Avisa o dock quando a janela em primeiro plano muda.
unsafe extern "system" fn foreground_hook(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND {
        return;
    }
    if let Some(s) = state_opt() {
        let dock = s.hwnd;
        if !dock.is_null() {
            PostMessageW(dock, WM_FOREGROUND, 0, 0);
        }
    }
}

unsafe extern "system" fn dock_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        return 0;
    }
    let Some(s) = state_opt() else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };

    if msg == WM_APPBAR {
        match wparam as u32 {
            ABN_POSCHANGED => apply_appbar(s),
            ABN_FULLSCREENAPP => update_visibility(s),
            ABN_STATECHANGE => {}
            _ => {}
        }
        return 0;
    }

    if msg == menu::WM_MENU_CHOICE {
        s.menu_open = false;
        let cmd = wparam as usize;
        win::log(&format!("menu: escolha id={cmd}"));
        if cmd != 0 {
            on_menu_command(s, cmd);
        }
        sync_slot(s, true);
        unsafe {
            InvalidateRect(s.hwnd, ptr::null(), 0);
        }
        return 0;
    }

    match msg {
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    s.manual_hidden = !s.manual_hidden;
                    update_visibility(s);
                }
                WM_RBUTTONUP => tray_menu(s),
                _ => {}
            }
            0
        }
        WM_PAINT => {
            paint(hwnd, s);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            sync_slot(s, false);
            0
        }
        WM_TIMER => {
            update_visibility(s);
            sync_slot(s, false);
            0
        }
        WM_LBUTTONDOWN => {
            on_lbutton_down(s, lparam);
            0
        }
        WM_LBUTTONUP => {
            on_lbutton_up(s, lparam);
            0
        }
        WM_MOUSEMOVE => {
            on_mouse_move(s);
            0
        }
        WM_RBUTTONUP => {
            open_menu(s);
            0
        }
        WM_SETCURSOR => {
            set_cursor(hwnd);
            1
        }
        WM_FOREGROUND => {
            update_visibility(s);
            0
        }
        WM_HOTKEY => {
            if wparam as i32 == HOTKEY_ID {
                s.manual_hidden = !s.manual_hidden;
                update_visibility(s);
            }
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            apply_appbar(s);
            0
        }
        WM_SHOWME => {
            if s.hidden {
                s.manual_hidden = false;
                update_visibility(s);
            }
            SetForegroundWindow(hwnd);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            cleanup(s);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
