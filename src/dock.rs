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
    CloseHandle, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, HDC, HFONT,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, MOD_SHIFT,
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
    IsWindow, IsWindowVisible, KillTimer, LoadCursorW, LoadIconW, MessageBoxW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SendMessageW, SetCursor,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, IDC_SIZEWE,
    IDI_APPLICATION, IDYES, MB_ICONQUESTION, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST,
    MB_YESNO, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE,
    SW_SHOWNOACTIVATE, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_CREATE,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_HOTKEY, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR,
    WM_SETTINGCHANGE, WM_SIZE, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use serde_json::{json, Value};

use crate::config::{self, Config};
use crate::menu;
use crate::win;

// Métricas em pixels LÓGICOS (96 dpi). Em runtime tudo é multiplicado pelo
// fator de escala do monitor (`dpi / 96`) — ver `px()`.
const HEADER_H: i32 = 40;
const PAD: i32 = 2;
const EDGE_GRAB: i32 = 6;
const MIN_W: i32 = 240;
const MAX_W: i32 = 1200;
const TIMER_TICK: usize = 1;
const HOTKEY_ID: i32 = 1;
const HOTKEY_STOP_ID: i32 = 2;
const VK_M: u32 = 0x4D;
const VK_K: u32 = 0x4B;

/// Callback do AppBar (recebido na janela do dock).
const WM_APPBAR: u32 = win::WM_APP + 10;
/// Pedido de outra instância para mostrar o dock.
const WM_SHOWME: u32 = win::WM_APP + 30;
/// Mudança de janela em primeiro plano (hook de acessibilidade).
const WM_FOREGROUND: u32 = win::WM_APP + 50;
/// Eventos do ícone da bandeja.
const WM_TRAY: u32 = win::WM_APP + 60;
/// Pedido de agente vindo do named pipe (executado na thread da UI).
const WM_AGENT_REQ: u32 = win::WM_APP + 70;

static mut HOOK: HWINEVENTHOOK = std::ptr::null_mut();
/// Mensagem registrada `TaskbarCreated` (enviada quando o Explorer reinicia).
static mut TASKBAR_CREATED: u32 = 0;
/// Handle da instância atual do pipe do agente (para o kill switch fechar).
static mut AGENT_PIPE: HANDLE = std::ptr::null_mut();
/// Encerra o loop do servidor de pipe (cleanup).
static mut AGENT_STOP: bool = false;

/// Sessão de agente visível no HUD do dock.
struct AgentInfo {
    name: String,
    activity: String,
    actions: u32,
    clicks: u32,
    captures: u32,
    /// PID do sidecar MCP (para o kill switch encerrar o processo).
    pid: u32,
}

/// Pedido do pipe entregue à thread da UI via `SendMessage`.
struct AgentMsg {
    req: Value,
    resp: Option<Value>,
}

const IDM_EXIT: usize = 101;
const IDM_TOGGLE: usize = 102;
/// "Parar agente" (kill switch pelo menu da bandeja).
const IDM_STOP_AGENT: usize = 103;
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

/// Focus lease aprovado pelo usuário: foco real por N segundos; depois restaura.
struct Lease {
    hwnd: HWND,
    previous: HWND,
    until: std::time::Instant,
}

struct State {
    hwnd: HWND,
    abd: APPBARDATA,
    registered: bool,
    busy: bool,
    /// Largura em pixels lógicos (96 dpi); ver `physical_width()`.
    width: i32,
    /// DPI efetivo do monitor do dock (96 = 100%).
    dpi: u32,
    /// dpi / 96.
    scale: f64,
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
    /// Botão "Parar agente" (zeta quando não há sessão).
    btn_stop: RECT,
    /// Sessão de agente conectada via MCP (HUD).
    agent: Option<AgentInfo>,
    /// Lease de foco ativo (modal aprovado) e janela a restaurar.
    lease: Option<Lease>,
    /// Timestamps dos pedidos de lease (rate limit de 3/min).
    lease_requests: Vec<std::time::Instant>,
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

fn make_font(size_logical: i32, weight: i32, scale: f64) -> HFONT {
    let face = win::wide("Segoe UI");
    let height = -((size_logical as f64 * scale).round() as i32).max(8);
    unsafe {
        CreateFontW(
            height,
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
    fn new(hwnd: HWND, width: i32, slot_count: usize, dpi: u32) -> State {
        let slot_count = slot_count.clamp(1, 3);
        let dpi = dpi.clamp(48, 480);
        let scale = dpi as f64 / 96.0;
        State {
            hwnd,
            abd: APPBARDATA::default(),
            registered: false,
            busy: false,
            width,
            dpi,
            scale,
            hidden: false,
            manual_hidden: false,
            menu_open: false,
            resizing: false,
            resize_start_x: 0,
            resize_start_w: width,
            slot_count,
            slots: vec![None; slot_count],
            picking: Vec::new(),
            btn_menu: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            btn_close: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            btn_release: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            btn_stop: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            agent: None,
            lease: None,
            lease_requests: Vec::new(),
            font_title: make_font(15, 600, scale),
            font_ui: make_font(13, 400, scale),
            font_hint: make_font(13, 400, scale),
        }
    }

    /// Largura física (pixels reais) reservada no AppBar.
    fn physical_width(&self) -> i32 {
        ((self.width as f64) * self.scale).round().max(80.0) as i32
    }

    fn rebuild_fonts(&mut self) {
        unsafe {
            DeleteObject(self.font_title);
            DeleteObject(self.font_ui);
            DeleteObject(self.font_hint);
        }
        self.font_title = make_font(15, 600, self.scale);
        self.font_ui = make_font(13, 400, self.scale);
        self.font_hint = make_font(13, 400, self.scale);
    }

    /// Atualiza o DPI (mudança de escala do Windows ou de monitor).
    fn set_dpi(&mut self, dpi: u32) {
        let dpi = dpi.clamp(48, 480);
        if dpi == self.dpi {
            return;
        }
        self.dpi = dpi;
        self.scale = dpi as f64 / 96.0;
        self.rebuild_fonts();
        win::log(&format!(
            "dpi = {dpi} (escala {:.2}x) — métricas e fontes rescaladas",
            self.scale
        ));
    }
}

/// Converte uma métrica lógica (96 dpi) para pixels físicos no monitor atual.
fn px(s: &State, logical: i32) -> i32 {
    ((logical as f64) * s.scale).round() as i32
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

    // DPI do monitor do dock (a janela já existe e é per-monitor aware v2).
    let mut dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        dpi = 96;
    }
    // QA: permite simular outra escala sem mexer no Windows (ex.: 144 = 150%).
    if let Ok(value) = std::env::var("MAYERDOCK_DPI") {
        if let Ok(forced) = value.trim().parse::<u32>() {
            dpi = forced.clamp(48, 480);
            win::log(&format!("dpi forçado por MAYERDOCK_DPI={dpi}"));
        }
    }
    let scale = dpi as f64 / 96.0;

    // Migração: configs antigas guardavam a largura em pixels físicos.
    let mut width_logical = cfg.width.clamp(MIN_W, MAX_W);
    if let Some(legacy) = cfg.legacy_width {
        width_logical = ((legacy as f64) / scale).round() as i32;
        width_logical = width_logical.clamp(MIN_W, MAX_W);
        win::log(&format!(
            "config antiga migrada: largura {legacy}px físicos -> {width_logical} lógicos (dpi {dpi})"
        ));
    }

    unsafe {
        STATE = Some(State::new(hwnd, width_logical, cfg.slots, dpi));
        let s = st();

        // Persiste no formato novo (width_logical) se veio de config antiga.
        if cfg.legacy_width.is_some() {
            Config {
                width: s.width,
                slots: s.slot_count,
                legacy_width: None,
            }
            .save();
        }

        register_appbar(s);
        if !s.registered {
            win::log("aviso: ABM_NEW falhou (shell não respondeu?)");
        }
        apply_appbar(s);

        let hotkey = RegisterHotKey(hwnd, HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_M);
        if hotkey == 0 {
            win::log("aviso: Ctrl+Alt+M não registrado (combinação em uso?)");
        }
        // Parar tudo (kill switch global): Ctrl+Alt+Shift+K.
        let stop_hotkey = RegisterHotKey(
            hwnd,
            HOTKEY_STOP_ID,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
            VK_K,
        );
        if stop_hotkey == 0 {
            win::log("aviso: Ctrl+Alt+Shift+K (parar agente) não registrado");
        }

        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
        SetTimer(hwnd, TIMER_TICK, 1000, None);
        tray_add(s);

        // Named pipe do Agent Workspace (MCP): sidecar conecta e usa as tools.
        start_agent_pipe(hwnd);

        // Mensagem do shell: enviada quando o Explorer reinicia (a bandeja e o
        // AppBar são perdidos nesse momento).
        TASKBAR_CREATED = RegisterWindowMessageW(win::wide("TaskbarCreated").as_ptr());
        if TASKBAR_CREATED == 0 {
            win::log("aviso: RegisterWindowMessage(TaskbarCreated) falhou");
        }

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

        let packaged = win::package_full_name();
        if let Some(name) = &packaged {
            win::log(&format!("modo pacote MSIX: {name}"));
        }
        win::log(&format!(
            "dock iniciado: largura={} lógicos ({} físicos) dpi={} divisões={} empacotado={} monitor={}x{}",
            s.width,
            s.physical_width(),
            s.dpi,
            s.slot_count,
            packaged.is_some(),
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
    let width_px = s.physical_width();
    let mut abd = s.abd;
    abd.uEdge = ABE_RIGHT;
    abd.rc = RECT {
        left: mon.right - width_px,
        top: mon.top,
        right: mon.right,
        bottom: mon.bottom,
    };

    let queried = unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut abd) };
    // O shell pode ter afastado a faixa da taskbar; preserva a largura pedida.
    abd.rc.left = abd.rc.right - width_px;
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
    let pad = px(s, PAD);
    let header = px(s, HEADER_H);
    RECT {
        left: r.left + pad,
        top: r.top + header,
        right: (r.right - pad).max(r.left + pad + px(s, 80)),
        bottom: (r.bottom - pad).max(r.top + header + px(s, 80)),
    }
}

/// Retângulo da divisão `index` (empilhamento vertical, 1–3).
fn slot_rect(s: &State, index: usize) -> RECT {
    let c = content_rect(s);
    let n = s.slot_count.max(1) as i32;
    let gap = px(s, 4);
    let total = (c.bottom - c.top).max(n);
    let each = ((total - gap * (n - 1)) / n).max(px(s, 40));
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

/// Mantém a camada do FluxTV (app irmão, overlay de escurecimento) acima do
/// dock: os dois são topmost, e o dock se reergue em vários momentos — sem
/// isso a faixa do dock fica de fora do escurecimento.
fn keep_flux_above() {
    unsafe {
        EnumWindows(Some(flux_raise_proc), 0);
    }
}

unsafe extern "system" fn flux_raise_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd) != 0 {
        let class = class_name(hwnd);
        if class.starts_with("FluxTV.") {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    1
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
        let mut current = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
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

    // Depois de reerguer/normalizar as janelas encaixadas, garante o FluxTV no
    // topo para o escurecimento também cobrir o dock.
    keep_flux_above();
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
    let mut origin = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetWindowRect(hwnd, &mut origin) } != 0;
    s.slots[free] = Some(Slot {
        hwnd,
        origin,
        valid_origin: ok,
    });
    sync_slot(s, true);
    ensure_slot_fits(s);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log(&format!("janela encaixada na divisão {free}"));
    true
}

/// Janelas com tamanho mínimo grande (comum em telas com escala > 100%) podem
/// recusar encolher até a divisão e vazar do dock. Nesse caso, aumenta a largura
/// do dock (até o máximo) para caber.
fn ensure_slot_fits(s: &mut State) {
    let mut needed = 0i32;
    for index in 0..s.slot_count {
        let Some(slot) = &s.slots[index] else {
            continue;
        };
        let target = slot_rect(s, index);
        let mut actual = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetWindowRect(slot.hwnd, &mut actual);
        }
        let target_w = target.right - target.left;
        let actual_w = actual.right - actual.left;
        if actual_w > target_w + 2 {
            needed = needed.max(actual_w + 2 * px(s, PAD));
        }
    }
    if needed > 0 {
        let logical = ((needed as f64) / s.scale).ceil() as i32;
        let new_width = logical.clamp(MIN_W, MAX_W);
        if new_width > s.width {
            win::log(&format!(
                "janela não caberia na divisão; aumentando dock de {} para {} lógicos",
                s.width, new_width
            ));
            s.width = new_width;
            apply_appbar(s);
        }
    }
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
            // Garante que a janela esteja visível (pode ter sido SW_HIDE'd).
            ShowWindow(slot.hwnd, SW_SHOWNOACTIVATE);
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
        legacy_width: None,
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

/// Liga/desliga o dock manualmente (atalho, bandeja, menu).
///
/// Ao ocultar, as janelas encaixadas são **liberadas** (voltam ao lugar) em vez
/// de escondidas: o usuário continua vendo os apps dele.
fn toggle_manual(s: &mut State) {
    if !s.manual_hidden {
        release_all(s);
        s.manual_hidden = true;
    } else {
        s.manual_hidden = false;
    }
    update_visibility(s);
    win::log(if s.manual_hidden {
        "dock oculto manualmente (janelas liberadas)"
    } else {
        "dock visível (manual)"
    });
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

        let header_h = px(s, HEADER_H);
        let btn_w = px(s, 32);
        let btn_gap = px(s, 2);

        let hdr = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: header_h,
        };
        let hb = CreateSolidBrush(rgb(30, 30, 36));
        FillRect(hdc, &hdr, hb);
        DeleteObject(hb);

        let edge = RECT {
            left: 0,
            top: 0,
            right: px(s, 2).max(1),
            bottom: h,
        };
        let eb = CreateSolidBrush(rgb(74, 112, 240));
        FillRect(hdc, &edge, eb);
        DeleteObject(eb);

        let mut title = RECT {
            left: px(s, 14),
            top: 0,
            right: (w - px(s, 136)).max(px(s, 40)),
            bottom: header_h,
        };
        draw_text(
            hdc,
            s.font_title,
            rgb(236, 238, 244),
            "MayerDock",
            &mut title,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        let margin = px(s, 6);
        let close = RECT {
            left: w - btn_w - margin,
            top: px(s, 7),
            right: w - margin,
            bottom: header_h - px(s, 7),
        };
        let menu_b = RECT {
            left: close.left - btn_w - btn_gap,
            top: close.top,
            right: close.left - btn_gap,
            bottom: close.bottom,
        };
        let release = RECT {
            left: menu_b.left - btn_w - btn_gap,
            top: close.top,
            right: menu_b.left - btn_gap,
            bottom: close.bottom,
        };
        // Com sessão de agente ativa, entra o botão "Parar" (kill switch).
        let stop = if s.agent.is_some() {
            RECT {
                left: release.left - btn_w - btn_gap,
                top: close.top,
                right: release.left - btn_gap,
                bottom: close.bottom,
            }
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            }
        };
        draw_button(hdc, s.font_ui, &release, "←", any_slot_occupied(s));
        draw_button(hdc, s.font_ui, &menu_b, "☰", true);
        draw_button(hdc, s.font_ui, &close, "✕", true);
        if stop.right > stop.left {
            draw_button(hdc, s.font_ui, &stop, "■", true);
        }
        s.btn_menu = menu_b;
        s.btn_close = close;
        s.btn_release = release;
        s.btn_stop = stop;

        // HUD do agente no cabeçalho (nome + atividade atual).
        if let Some(agent) = &s.agent {
            let hud_right = if stop.right > stop.left {
                stop.left
            } else {
                release.left
            };
            let hud = RECT {
                left: px(s, 118),
                top: 0,
                right: hud_right - px(s, 8),
                bottom: header_h,
            };
            if hud.right > hud.left + px(s, 40) {
                let mut text = format!("● {} · {}", agent.name, agent.activity);
                if let Some(lease) = &s.lease {
                    let remaining = lease
                        .until
                        .saturating_duration_since(std::time::Instant::now());
                    text.push_str(&format!(" · lease {}s", remaining.as_secs() + 1));
                }
                if agent.clicks > 0 || agent.captures > 0 {
                    text.push_str(&format!(" · {}c {}s", agent.clicks, agent.captures));
                }
                draw_text(
                    hdc,
                    s.font_hint,
                    rgb(126, 186, 255),
                    &text,
                    &mut { hud },
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
            }
        }

        // Separadores entre as divisões verticais.
        if s.slot_count > 1 {
            let line = CreateSolidBrush(rgb(62, 66, 82));
            let inset = px(s, 4);
            let esp = px(s, 2).max(1);
            for index in 1..s.slot_count {
                let r = slot_rect(s, index);
                FillRect(
                    hdc,
                    &RECT {
                        left: r.left + inset,
                        top: r.top - 2 * esp,
                        right: r.right - inset,
                        bottom: r.top - esp,
                    },
                    line,
                );
            }
            DeleteObject(line);
        }

        if !any_slot_occupied(s) {
            let mut hint = RECT {
                left: px(s, 14),
                top: header_h + px(s, 24),
                right: w - px(s, 14),
                bottom: h - px(s, 24),
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
    let bg = if enabled {
        rgb(44, 44, 54)
    } else {
        rgb(30, 30, 36)
    };
    let brush = CreateSolidBrush(bg);
    FillRect(hdc, rc, brush);
    DeleteObject(brush);
    let mut text_rc = *rc;
    let color = if enabled {
        rgb(210, 214, 226)
    } else {
        rgb(92, 96, 108)
    };
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
    if x <= px(s, EDGE_GRAB) {
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
            legacy_width: None,
        }
        .save();
        win::log(&format!(
            "largura salva: {} lógicos ({} físicos)",
            s.width,
            s.physical_width()
        ));
        return;
    }

    let (x, y) = client_point(lparam);
    if y < px(s, HEADER_H) {
        if point_in(&s.btn_stop, x, y) {
            stop_agent(s);
            return;
        }
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
    let dx = (s.resize_start_x - p.x) as f64 / s.scale;
    let new_width = ((s.resize_start_w as f64 + dx).round() as i32).clamp(MIN_W, MAX_W);
    if new_width != s.width {
        s.width = new_width;
        apply_appbar(s);
    }
}

fn set_cursor(hwnd: HWND, s: &State) {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let cursor = if p.x - rc.left <= px(s, EDGE_GRAB) {
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
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        GetWindowRect(s.hwnd, &mut wr);
    }
    let anchor = POINT {
        x: wr.left + s.btn_menu.left,
        y: wr.top + s.btn_menu.top,
    };
    s.menu_open = menu::show(s.hwnd, anchor, items, s.scale);
    keep_flux_above();
    if !s.menu_open {
        win::log("menu: não foi possível abrir");
    }
}

fn on_menu_command(s: &mut State, id: usize) {
    match id {
        IDM_EXIT => unsafe {
            PostMessageW(s.hwnd, WM_CLOSE, 0, 0);
        },
        IDM_TOGGLE => toggle_manual(s),
        IDM_STOP_AGENT => stop_agent(s),
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
fn lease_allow_path() -> std::path::PathBuf {
    config::data_dir().join("lease-allow.txt")
}

fn lease_is_allowed(app: &str) -> bool {
    if app.is_empty() {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(lease_allow_path()) else {
        return false;
    };
    content
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case(app))
}

fn lease_allow_add(app: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(config::data_dir());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(lease_allow_path())
    {
        let _ = writeln!(file, "{app}");
    }
}

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
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size);
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

/// Chamado quando o Explorer reinicia (`TaskbarCreated`): re-adiciona o ícone
/// da bandeja, re-registra o AppBar e restaura o z-order.
fn shell_restart(s: &mut State) {
    win::log("shell reiniciou (TaskbarCreated): re-registrando bandeja e AppBar");
    // Remove antes de adicionar: idempotente e cobre tanto o caso real (o shell
    // perdeu os registros) quanto uma notificação duplicada.
    tray_remove(s);
    tray_add(s);
    if !s.hidden {
        remove_appbar(s);
        register_appbar(s);
        if s.registered {
            win::log("appbar re-registrada após restart do shell");
            apply_appbar(s);
            let r = s.abd.rc;
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
            sync_slot(s, true);
        } else {
            win::log("aviso: AppBar não re-registrou após restart do shell");
        }
    }
    keep_flux_above();
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

/// `--status`: versão, modo (pacote/portátil), config e caminhos reais.
pub fn print_status() {
    let cfg = Config::load();
    let packaged = win::package_full_name();
    let data = config::resolved_data_dir();
    let mut out = String::new();
    out.push_str(&format!("version: {}\r\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!(
        "packaged: {}\r\n",
        if packaged.is_some() { "true" } else { "false" }
    ));
    if let Some(name) = &packaged {
        out.push_str(&format!("package: {name}\r\n"));
    }
    out.push_str(&format!("width_logical: {}\r\n", cfg.width));
    out.push_str(&format!("slots: {}\r\n", cfg.slots));
    out.push_str(&format!("data_dir: {}\r\n", data.display()));
    out.push_str(&format!(
        "config: {}\r\n",
        data.join("config.ini").display()
    ));
    out.push_str(&format!(
        "log: {}\r\n",
        data.join("mayerdock.log").display()
    ));
    win::console_print(&out);
}

// ---------------------------------------------------------------------------
// Agent Workspace — servidor do named pipe (sidecar MCP ↔ dock)
// ---------------------------------------------------------------------------

/// Nome do pipe usado pelo sidecar MCP (`\\.\pipe\MayerDock.Agent`).
const AGENT_PIPE_NAME: &str = r"\\.\pipe\MayerDock.Agent";

fn agent_pipe_close() {
    unsafe {
        if !AGENT_PIPE.is_null() {
            CloseHandle(AGENT_PIPE);
            AGENT_PIPE = ptr::null_mut();
        }
    }
}

/// Token local do pipe (protege contra outro usuário/processo se conectar).
fn make_agent_token() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let stack = &now as *const _ as usize;
    format!(
        "{:016x}{:016x}{:08x}",
        now as u64,
        (now >> 64) as u64,
        pid ^ (stack as u32)
    )
}

/// Sobe o servidor do pipe numa thread; escreve o token para o sidecar ler.
fn start_agent_pipe(hwnd: HWND) {
    let token = make_agent_token();
    let dir = config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let token_path = dir.join("agent-token");
    match std::fs::write(&token_path, &token) {
        Ok(_) => win::log(&format!(
            "agent pipe: pronto (token em {})",
            token_path.display()
        )),
        Err(err) => win::log(&format!("agent pipe: falhou escrever token: {err}")),
    }
    let raw_hwnd = hwnd as usize;
    std::thread::spawn(move || agent_pipe_loop(raw_hwnd as HWND, token));
}

fn agent_pipe_loop(hwnd: HWND, token: String) {
    let name = win::wide(AGENT_PIPE_NAME);
    loop {
        unsafe {
            if AGENT_STOP {
                win::log("agent pipe: servidor encerrado");
                return;
            }
            let pipe = CreateNamedPipeW(
                name.as_ptr(),
                0x0000_0003, // PIPE_ACCESS_DUPLEX
                0,           // PIPE_TYPE_BYTE | READMODE_BYTE | WAIT
                1,           // uma instância (um agente por vez)
                65536,
                65536,
                0,
                ptr::null(),
            );
            if pipe.is_null() || pipe as isize == -1 {
                win::log("agent pipe: CreateNamedPipeW falhou");
                return;
            }
            AGENT_PIPE = pipe;
            let connected = ConnectNamedPipe(pipe, ptr::null_mut());
            if AGENT_STOP {
                return;
            }
            // 535 = ERROR_PIPE_CONNECTED: cliente conectou antes do ConnectNamedPipe.
            if connected == 0 && GetLastError() != 535 {
                if AGENT_PIPE == pipe {
                    CloseHandle(pipe);
                    AGENT_PIPE = ptr::null_mut();
                }
                continue;
            }
            win::log("agent pipe: cliente conectado");
            serve_agent(pipe, hwnd, &token);
            if AGENT_PIPE == pipe {
                AGENT_PIPE = ptr::null_mut();
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
            win::log("agent pipe: cliente desconectado");
        }
    }
}

fn serve_agent(pipe: HANDLE, hwnd: HWND, token: &str) {
    let mut authenticated = false;
    loop {
        let Some(line) = read_pipe_line(pipe) else {
            return;
        };
        let req: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                if write_pipe_line(
                    pipe,
                    &json!({ "id": 0, "ok": false, "error": "JSON inválido" }),
                )
                .is_none()
                {
                    return;
                }
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(json!(0));
        let tool = req.get("tool").and_then(Value::as_str).unwrap_or("");

        if tool == "auth" {
            let ok = req
                .pointer("/args/token")
                .and_then(Value::as_str)
                .map(|value| value == token)
                .unwrap_or(false);
            let resp = if ok {
                json!({ "id": id, "ok": true, "result": { "ok": true } })
            } else {
                json!({ "id": id, "ok": false, "error": "token inválido" })
            };
            if write_pipe_line(pipe, &resp).is_none() {
                return;
            }
            if !ok {
                return;
            }
            authenticated = true;
            continue;
        }
        if !authenticated {
            if write_pipe_line(
                pipe,
                &json!({ "id": id, "ok": false, "error": "não autenticado" }),
            )
            .is_none()
            {
                return;
            }
            continue;
        }

        let mut msg = AgentMsg {
            req: req.clone(),
            resp: None,
        };
        unsafe {
            SendMessageW(hwnd, WM_AGENT_REQ, &mut msg as *mut AgentMsg as WPARAM, 0);
        }
        let result = msg
            .resp
            .unwrap_or_else(|| json!({ "ok": false, "error": "sem resposta da UI" }));
        let resp = match result.get("ok").and_then(Value::as_bool) {
            Some(true) => {
                json!({ "id": id, "ok": true, "result": result.get("result").cloned().unwrap_or(Value::Null) })
            }
            _ => json!({
                "id": id,
                "ok": false,
                "error": result.get("error").and_then(Value::as_str).unwrap_or("erro")
            }),
        };
        if write_pipe_line(pipe, &resp).is_none() {
            return;
        }
    }
}

fn read_pipe_line(pipe: HANDLE) -> Option<String> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                pipe,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            return None;
        }
        if buf[0] == b'\n' {
            break;
        }
        if buf[0] != b'\r' {
            bytes.push(buf[0]);
        }
        if bytes.len() > 1 << 20 {
            return None;
        }
    }
    Some(String::from_utf8_lossy(&bytes).to_string())
}

fn write_pipe_line(pipe: HANDLE, value: &Value) -> Option<()> {
    let mut line = serde_json::to_string(value).ok()?;
    line.push('\n');
    let bytes = line.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk = &bytes[offset..];
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                pipe,
                chunk.as_ptr(),
                chunk.len() as u32,
                &mut written,
                ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return None;
        }
        offset += written as usize;
    }
    Some(())
}

fn slots_json(s: &State) -> Value {
    let mut items = Vec::new();
    for index in 0..s.slot_count {
        let rect = slot_rect(s, index);
        match &s.slots[index] {
            Some(slot) => {
                let title = window_title(slot.hwnd).unwrap_or_default();
                items.push(json!({
                    "index": index,
                    "occupied": true,
                    "hwnd": slot.hwnd as usize,
                    "title": title,
                    "rect": { "left": rect.left, "top": rect.top, "right": rect.right, "bottom": rect.bottom }
                }));
            }
            None => items.push(json!({ "index": index, "occupied": false })),
        }
    }
    json!(items)
}

/// Executa um pedido do agente na thread da UI (chamado via `WM_AGENT_REQ`).
fn handle_agent_request(s: &mut State, req: &Value) -> Value {
    let tool = req.get("tool").and_then(Value::as_str).unwrap_or("");
    if let Some(agent) = s.agent.as_mut() {
        let label = match tool {
            "list_slots" => "lendo slots",
            "dock_pin" => "encaixando janela",
            "dock_unpin" => "liberando janela",
            "dock_release_all" => "liberando janelas",
            "status" => "consultando dock",
            "session_hello" => "conectando",
            other => other,
        };
        agent.activity = label.to_string();
        agent.actions = agent.actions.saturating_add(1);
    }
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }

    let ok = |value: Value| json!({ "ok": true, "result": value });
    let err = |message: &str| json!({ "ok": false, "error": message });

    match tool {
        "status" => ok(json!({
            "width_logical": s.width,
            "slots": s.slot_count,
            "docked": s.slots.iter().flatten().count(),
            "agent": s.agent.as_ref().map(|agent| json!({
                "name": agent.name,
                "activity": agent.activity,
                "actions": agent.actions
            }))
        })),
        "list_slots" => ok(slots_json(s)),
        "dock_pin" => match req.pointer("/args/hwnd").and_then(Value::as_i64) {
            Some(raw) => {
                let hwnd = raw as usize as HWND;
                let pinned = pin_first_free(s, hwnd);
                ok(json!({ "pinned": pinned }))
            }
            None => err("informe args.hwnd"),
        },
        "dock_unpin" => match req.pointer("/args/hwnd").and_then(Value::as_i64) {
            Some(raw) => {
                let hwnd = raw as usize as HWND;
                let index = s
                    .slots
                    .iter()
                    .position(|slot| slot.as_ref().map(|s| s.hwnd) == Some(hwnd));
                if let Some(index) = index {
                    release_slot(s, index);
                    ok(json!({ "released": true, "index": index }))
                } else {
                    ok(json!({ "released": false }))
                }
            }
            None => err("informe args.hwnd"),
        },
        "dock_release_all" => {
            let count = s.slots.iter().flatten().count();
            release_all(s);
            ok(json!({ "released": count }))
        }
        "session_hello" => {
            let name = req
                .pointer("/args/name")
                .and_then(Value::as_str)
                .unwrap_or("agente")
                .to_string();
            let pid = req
                .pointer("/args/pid")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            s.agent = Some(AgentInfo {
                activity: "conectado".to_string(),
                name: name.clone(),
                actions: 0,
                clicks: 0,
                captures: 0,
                pid,
            });
            win::log(&format!("agent: sessão iniciada ({name}, pid {pid})"));
            ok(json!({ "session": name }))
        }
        "session_bye" => {
            s.agent = None;
            win::log("agent: sessão encerrada");
            ok(json!({ "bye": true }))
        }
        "activity" => {
            if let Some(agent) = s.agent.as_mut() {
                if let Some(text) = req.pointer("/args/text").and_then(Value::as_str) {
                    agent.activity = text.to_string();
                }
                if let Some(clicks) = req.pointer("/args/clicks").and_then(Value::as_u64) {
                    agent.clicks = clicks.min(u32::MAX as u64) as u32;
                }
                if let Some(captures) = req.pointer("/args/captures").and_then(Value::as_u64) {
                    agent.captures = captures.min(u32::MAX as u64) as u32;
                }
            }
            ok(json!({ "ok": true }))
        }
        "focus_lease" => {
            let raw = req
                .pointer("/args/hwnd")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let hwnd = raw as usize as HWND;
            if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
                return err("hwnd inválido");
            }
            if s.lease.is_some() {
                return err("já existe um lease de foco ativo");
            }
            // Rate limit: no máximo 3 pedidos de lease por minuto.
            let now = std::time::Instant::now();
            s.lease_requests
                .retain(|at| now.duration_since(*at).as_secs() < 60);
            if s.lease_requests.len() >= 3 {
                win::log("agent: lease bloqueado pelo rate limit (3/min)");
                return err("limite de 3 leases por minuto atingido");
            }
            s.lease_requests.push(now);

            let seconds = req
                .pointer("/args/seconds")
                .and_then(Value::as_u64)
                .unwrap_or(15)
                .clamp(3, 120);
            let reason = req
                .pointer("/args/reason")
                .and_then(Value::as_str)
                .unwrap_or("testes do agente");
            let title = window_title(hwnd).unwrap_or_default();
            let app = process_label(hwnd);
            // "Sempre para este app": allowlist persistida em lease-allow.txt.
            let allowed = lease_is_allowed(&app);
            // QA: MAYERDOCK_LEASE_AUTO=1 aprova sem modal (só para testes).
            let auto = std::env::var("MAYERDOCK_LEASE_AUTO")
                .map(|value| value == "1")
                .unwrap_or(false);
            let granted = if auto || allowed {
                true
            } else {
                let message = format!(
                    "O agente quer controlar mouse e teclado por {seconds} s.\n\n\
                     Janela: «{title}»\nMotivo: {reason}\n\nPermitir?"
                );
                let body = win::wide(&message);
                let caption = win::wide("MayerDock — pedido do agente");
                unsafe {
                    MessageBoxW(
                        s.hwnd,
                        body.as_ptr(),
                        caption.as_ptr(),
                        MB_YESNO | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
                    ) == IDYES as i32
                }
            };
            if granted && !auto && !allowed && !app.is_empty() {
                let body = win::wide(&format!(
                    "Permitir sempre para «{app}»?\n\nA regra vale para futuros leases deste \
                     app e fica em:\n{}\n\n(apague o arquivo para revogar)",
                    lease_allow_path().display()
                ));
                let caption = win::wide("MayerDock — lembrar permissão?");
                let remember = unsafe {
                    MessageBoxW(
                        s.hwnd,
                        body.as_ptr(),
                        caption.as_ptr(),
                        MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
                    ) == IDYES as i32
                };
                if remember {
                    lease_allow_add(&app);
                    win::log(&format!("agent: lease sempre permitido para {app}"));
                }
            }
            if granted {
                let previous = unsafe { GetForegroundWindow() };
                unsafe {
                    SetForegroundWindow(hwnd);
                }
                s.lease = Some(Lease {
                    hwnd,
                    previous,
                    until: std::time::Instant::now() + std::time::Duration::from_secs(seconds),
                });
                if let Some(agent) = s.agent.as_mut() {
                    agent.activity = "lease ativo".to_string();
                }
                win::log(&format!(
                    "agent: lease concedido ({seconds}s) hwnd=0x{:X}",
                    hwnd as usize
                ));
                ok(json!({ "granted": true, "seconds": seconds }))
            } else {
                win::log("agent: lease negado pelo usuário");
                ok(json!({ "granted": false }))
            }
        }
        other => err(&format!("tool desconhecida: {other}")),
    }
}

/// Kill switch: encerra a sessão e derruba a conexão do pipe.
/// Expira o lease: restaura o foco da janela anterior e limpa o HUD.
fn expire_lease(s: &mut State) {
    let Some(lease) = s.lease.as_ref() else {
        return;
    };
    if std::time::Instant::now() < lease.until {
        return;
    }
    let previous = lease.previous;
    let leased = lease.hwnd;
    s.lease = None;
    if !previous.is_null() && unsafe { IsWindow(previous) } != 0 {
        unsafe {
            SetForegroundWindow(previous);
        }
    }
    if let Some(agent) = s.agent.as_mut() {
        agent.activity = "lease expirado".to_string();
    }
    win::log(&format!(
        "agent: lease expirado (foco restaurado; era hwnd=0x{:X})",
        leased as usize
    ));
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

fn stop_agent(s: &mut State) {
    if let Some(lease) = s.lease.take() {
        if !lease.previous.is_null() && unsafe { IsWindow(lease.previous) } != 0 {
            unsafe {
                SetForegroundWindow(lease.previous);
            }
        }
    }
    // Kill switch: encerra o processo do sidecar (o harness vê o MCP cair) e
    // derruba a conexão do pipe. Sem pid (ex.: sessão antiga), só desconecta.
    let pid = s.agent.as_ref().map(|agent| agent.pid).unwrap_or(0);
    win::log(&format!(
        "agent: kill switch (Parar) — encerrando sidecar pid {pid}"
    ));
    if pid != 0 {
        unsafe {
            let process = OpenProcess(0x0001, 0, pid); // PROCESS_TERMINATE
            if !process.is_null() {
                TerminateProcess(process, 1);
                CloseHandle(process);
            } else {
                win::log("agent: não abri o processo do sidecar para terminar");
            }
        }
    }
    s.agent = None;
    agent_pipe_close();
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

fn cleanup(s: &mut State) {
    unsafe {
        KillTimer(s.hwnd, TIMER_TICK);
        UnregisterHotKey(s.hwnd, HOTKEY_ID);
        UnregisterHotKey(s.hwnd, HOTKEY_STOP_ID);
        if !HOOK.is_null() {
            UnhookWinEvent(HOOK);
            HOOK = ptr::null_mut();
        }
        // Encerra o servidor do pipe de agentes (unblock do ConnectNamedPipe).
        AGENT_STOP = true;
        agent_pipe_close();
    }
    s.agent = None;
    release_all(s);
    remove_appbar(s);
    tray_remove(s);
    let _ = std::fs::remove_file(config::data_dir().join("agent-token"));
    Config {
        width: s.width,
        slots: s.slot_count,
        legacy_width: None,
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

    // Ícone embutido no exe pelo build.rs (recurso 1); fallback: genérico.
    let embedded = unsafe { LoadIconW(win::hinstance(), 1usize as *const u16) };
    nid.hIcon = if embedded.is_null() {
        unsafe { LoadIconW(ptr::null_mut(), IDI_APPLICATION) }
    } else {
        embedded
    };
    copy_wide(&mut nid.szTip, "MayerDock — dock lateral");
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
    if ok == 0 {
        win::log("aviso: ícone da bandeja não foi criado");
    } else if embedded.is_null() {
        win::log("ícone da bandeja criado (genérico; recurso embutido ausente)");
    } else {
        win::log("ícone da bandeja criado (embutido)");
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
    if let Some(agent) = &s.agent {
        items.push(menu::MenuItem {
            label: format!("Parar agente ({})", agent.name),
            cmd: IDM_STOP_AGENT,
            kind: menu::Kind::Item,
        });
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
    s.menu_open = menu::show(s.hwnd, p, items, s.scale);
    keep_flux_above();
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

    // O Explorer reiniciou: bandeja e AppBar precisam ser re-registrados.
    if TASKBAR_CREATED != 0 && msg == TASKBAR_CREATED {
        shell_restart(s);
        return 0;
    }

    // Pedido do sidecar MCP: executa na thread da UI e devolve a resposta.
    if msg == WM_AGENT_REQ {
        let msg_ptr = wparam as *mut AgentMsg;
        if !msg_ptr.is_null() {
            let request = (*msg_ptr).req.clone();
            let resp = handle_agent_request(s, &request);
            (*msg_ptr).resp = Some(resp);
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
                    toggle_manual(s);
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
            expire_lease(s);
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
            set_cursor(hwnd, s);
            1
        }
        WM_FOREGROUND => {
            update_visibility(s);
            0
        }
        WM_HOTKEY => {
            let id = wparam as i32;
            if id == HOTKEY_ID {
                toggle_manual(s);
            } else if id == HOTKEY_STOP_ID && s.agent.is_some() {
                win::log("agent: hotkey Ctrl+Alt+Shift+K (parar agente)");
                stop_agent(s);
            }
            0
        }
        WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            s.set_dpi(dpi);
            apply_appbar(s);
            sync_slot(s, true);
            unsafe {
                InvalidateRect(s.hwnd, ptr::null(), 0);
            }
            0
        }
        WM_DISPLAYCHANGE => {
            let dpi = unsafe { GetDpiForWindow(hwnd) };
            if dpi > 0 {
                s.set_dpi(dpi);
            }
            apply_appbar(s);
            0
        }
        WM_SETTINGCHANGE => {
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
