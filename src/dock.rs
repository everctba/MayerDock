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
    BeginPaint, ClientToScreen, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, UpdateWindow,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_TOP, DT_VCENTER, DT_WORDBREAK,
    FF_DONTCARE, HDC, HFONT, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
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
    SHAppBarMessage, SHQueryUserNotificationState, ShellExecuteW, Shell_NotifyIconW, ABE_RIGHT,
    ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABN_FULLSCREENAPP, ABN_POSCHANGED,
    ABN_STATECHANGE, APPBARDATA, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowW,
    GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics,
    GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, KillTimer, LoadCursorW, LoadIconW, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SendMessageW, SetCursor,
    SetForegroundWindow, SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE, HWND_BOTTOM, HWND_NOTOPMOST,
    HWND_TOPMOST, IDC_ARROW, IDC_SIZENS, IDC_SIZEWE, IDI_APPLICATION, IDYES, LWA_COLORKEY,
    MB_ICONQUESTION, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MSG,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE, SW_SHOWNOACTIVATE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_CREATE, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR,
    WM_SETTINGCHANGE, WM_SIZE, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
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
/// Timer curto do confetti (partículas do fim de sessão).
const TIMER_CONFETTI: usize = 2;
/// Timer do overlay de ações (screencast key).
const TIMER_OVERLAY: usize = 3;
/// Retry de bandeja/AppBar depois do restart do Explorer.
const TIMER_SHELL_RETRY: usize = 4;
const HOTKEY_ID: i32 = 1;
const HOTKEY_STOP_ID: i32 = 2;
/// Undo/redo de layout (Memento) e cancelamento de edição.
const HOTKEY_UNDO_ID: i32 = 3;
const HOTKEY_REDO_ID: i32 = 4;
const HOTKEY_CANCEL_ID: i32 = 5;
const VK_Z: u32 = 0x5A;
const VK_ESCAPE: u32 = 0x1B;
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
    /// Projeto/chat de onde veio a sessão (cwd do harness ou MAYERDOCK_PROJECT).
    project: String,
    activity: String,
    actions: u32,
    clicks: u32,
    captures: u32,
    /// PID do sidecar MCP (para o kill switch encerrar o processo).
    pid: u32,
}

/// Pílula persistente na janela-alvo: "cliente · projeto".
#[derive(Clone)]
struct OverlayPill {
    text: String,
    hwnd: HWND,
    until: std::time::Instant,
}

/// App esperando um slot livre (fila mostrada no rodapé do dock).
#[derive(Clone)]
struct PendingApp {
    hwnd: HWND,
    title: String,
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
/// "Assumir/devolver controle" (pair testing).
const IDM_CONTROL: usize = 104;
/// Liga/desliga o overlay de ações do agente.
const IDM_OVERLAY: usize = 105;
/// Abre o diálogo com instruções MCP (copiar).
const IDM_MCP_HELP: usize = 106;
/// Restaura a área de trabalho (remove reserva órfã de AppBar).
const IDM_RESET_WORKAREA: usize = 107;
/// Liga/desliga o "modo atrás" (janelas do dock no fundo).
const IDM_BOTTOM: usize = 108;
/// Favoritos do launcher (menu): 200 + índice.
const IDM_FAVORITE_BASE: usize = 200;
const IDM_FAVORITE_LAST: usize = 208;
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

#[derive(Clone, Copy)]
struct LayoutSnapshot {
    width: i32,
    split0: i32,
    split1: i32,
}

/// Tooltip efêmero do "screencast key" (overlay de ações do agente).
#[derive(Clone)]
struct OverlayEvent {
    text: String,
    x: i32,
    y: i32,
    until: std::time::Instant,
}

struct State {
    hwnd: HWND,
    abd: APPBARDATA,
    registered: bool,
    busy: bool,
    /// Largura em pixels lógicos (96 dpi); ver `physical_width()`.
    width: i32,
    /// Fim da 1ª divisão em % da área útil (2 ou 3 slots).
    split0: i32,
    /// Fim da 2ª divisão em % da área útil (3 slots).
    split1: i32,
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
    /// Divisória sendo arrastada (índice da fronteira entre slots).
    drag_split: Option<usize>,
    /// Snapshot do início da edição atual (Unit of Work).
    edit_pre: Option<LayoutSnapshot>,
    /// Pilhas de undo/redo (Memento) por commit de layout.
    undo_stack: Vec<LayoutSnapshot>,
    redo_stack: Vec<LayoutSnapshot>,
    /// Tema claro/escuro atual (acompanha o Windows a cada ~5 s).
    theme_dark: bool,
    theme_check: u32,
    /// Confetti de fim de sessão (visual + resumo no HUD).
    confetti_until: Option<std::time::Instant>,
    confetti_summary: String,
    /// Favoritos do launcher: (rótulo, comando).
    favorites: Vec<(String, String)>,
    /// Overlay "screencast key": tooltips das ações do agente.
    overlay_enabled: bool,
    overlay_hwnd: HWND,
    overlay_events: Vec<OverlayEvent>,
    overlay_pill: Option<OverlayPill>,
    /// Fila de apps aguardando slot livre (máx. slots = 3).
    pending: Vec<PendingApp>,
    /// "Modo atrás": janelas encaixadas no fundo da pilha Z (não topmost).
    bottom_mode: bool,
    /// Bandeja criada com sucesso (usado no retry pós-restart do Explorer).
    tray_created: bool,
    /// Tentativas restantes de re-registrar bandeja/AppBar após shell restart.
    shell_retry: u32,
    slot_count: usize,
    slots: Vec<Option<Slot>>,
    picking: Vec<HWND>,
    btn_menu: RECT,
    btn_close: RECT,
    btn_release: RECT,
    /// Botão "Parar agente" (zeta quando não há sessão).
    btn_stop: RECT,
    /// Botão "Assumir/Devolver" o controle (pair testing, só com sessão).
    btn_control: RECT,
    /// Sessão de agente conectada via MCP (HUD).
    agent: Option<AgentInfo>,
    /// Humano assumiu o controle: tools do agente ficam pausadas.
    agent_paused: bool,
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

/// Cor estável por sessão de agente (hash FNV do nome na paleta).
fn session_color(name: &str) -> u32 {
    let palette = [
        rgb(74, 112, 240), // azul
        rgb(139, 92, 246), // violeta
        rgb(20, 184, 166), // teal
        rgb(236, 72, 153), // rosa
        rgb(34, 197, 94),  // verde
        rgb(234, 179, 8),  // amarelo
    ];
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    palette[(hash % palette.len() as u64) as usize]
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
    fn new(hwnd: HWND, width: i32, slot_count: usize, dpi: u32, split0: i32, split1: i32) -> State {
        let slot_count = slot_count.clamp(1, 3);
        let dpi = dpi.clamp(48, 480);
        let scale = dpi as f64 / 96.0;
        State {
            hwnd,
            abd: APPBARDATA::default(),
            registered: false,
            busy: false,
            width,
            split0: split0.clamp(10, 90),
            split1: split1.clamp(10, 90),
            dpi,
            scale,
            hidden: false,
            manual_hidden: false,
            menu_open: false,
            resizing: false,
            resize_start_x: 0,
            resize_start_w: width,
            drag_split: None,
            edit_pre: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            theme_dark: win::theme_is_dark(),
            theme_check: 0,
            confetti_until: None,
            confetti_summary: String::new(),
            favorites: load_favorites(),
            overlay_enabled: load_overlay_enabled(),
            overlay_hwnd: ptr::null_mut(),
            overlay_events: Vec::new(),
            overlay_pill: None,
            pending: Vec::new(),
            bottom_mode: load_bottom_mode(),
            tray_created: false,
            shell_retry: 0,
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
            btn_control: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            agent: None,
            agent_paused: false,
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

    // Limpa reserva órfã de uma instância anterior (crash/taskkill).
    let _ = win::reset_workarea();

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
        STATE = Some(State::new(
            hwnd,
            width_logical,
            cfg.slots,
            dpi,
            cfg.split0,
            cfg.split1,
        ));
        let s = st();

        // Persiste no formato novo (width_logical) se veio de config antiga.
        if cfg.legacy_width.is_some() {
            save_layout(s);
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
        // Undo/redo de layout e cancelamento da edição (PreviewSession).
        let undo_hotkey = RegisterHotKey(
            hwnd,
            HOTKEY_UNDO_ID,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_Z,
        );
        if undo_hotkey == 0 {
            win::log("aviso: Ctrl+Alt+Z (undo layout) não registrado");
        }
        let redo_hotkey = RegisterHotKey(
            hwnd,
            HOTKEY_REDO_ID,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
            VK_Z,
        );
        if redo_hotkey == 0 {
            win::log("aviso: Ctrl+Alt+Shift+Z (redo layout) não registrado");
        }
        let cancel_hotkey = RegisterHotKey(
            hwnd,
            HOTKEY_CANCEL_ID,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
            VK_ESCAPE,
        );
        if cancel_hotkey == 0 {
            win::log("aviso: Ctrl+Alt+Shift+Esc (cancelar edição) não registrado");
        }

        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
        SetTimer(hwnd, TIMER_TICK, 1000, None);
        s.tray_created = tray_add(s);

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
    let ok = unsafe { SHAppBarMessage(ABM_REMOVE, &mut abd) };
    s.registered = false;
    if ok == 0 {
        // Shell pode ter deixado a workarea reservada; restaura como fallback.
        let _ = win::reset_workarea();
    }
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
    // Fila visível no rodapé reserva uma faixa fina.
    let queue = if s.pending.is_empty() { 0 } else { px(s, 26) };
    RECT {
        left: r.left + pad,
        top: r.top + header,
        right: (r.right - pad).max(r.left + pad + px(s, 80)),
        bottom: (r.bottom - pad - queue).max(r.top + header + px(s, 80)),
    }
}

/// Retângulo da divisão `index` (empilhamento vertical, 1–3).
fn slot_rect(s: &State, index: usize) -> RECT {
    let c = content_rect(s);
    let n = s.slot_count.max(1);
    let gap = px(s, 4);
    let total = (c.bottom - c.top).max(n as i32);
    let usable = (total - gap * (n as i32 - 1)).max(n as i32);
    let heights = slot_heights(s, usable);
    let top = c.top + heights[..index].iter().sum::<i32>() + gap * index as i32;
    RECT {
        left: c.left,
        top,
        right: c.right,
        bottom: top + heights[index],
    }
}

/// Alturas (px físicos) de cada slot a partir dos percentuais e da área útil.
fn slot_heights(s: &State, usable: i32) -> [i32; 3] {
    let usable = usable.max(3);
    match s.slot_count.max(1) {
        1 => [usable, 0, 0],
        2 => {
            let first = (usable * s.split0 / 100).clamp(1, usable - 1);
            [first, usable - first, 0]
        }
        _ => {
            let first = (usable * s.split0 / 100).clamp(1, usable - 2);
            let second = (usable * s.split1 / 100).clamp(first + 1, usable - 1);
            [first, second - first, usable - second]
        }
    }
}

/// Unit of Work: snapshot do início de uma edição de layout.
fn layout_snapshot(s: &State) -> LayoutSnapshot {
    LayoutSnapshot {
        width: s.width,
        split0: s.split0,
        split1: s.split1,
    }
}

fn begin_edit(s: &mut State) {
    if s.edit_pre.is_none() {
        s.edit_pre = Some(layout_snapshot(s));
    }
}

/// Commit atômico: persiste config (tmp+rename) e empilha o snapshot para undo.
fn commit_edit(s: &mut State) {
    if let Some(pre) = s.edit_pre.take() {
        s.undo_stack.push(pre);
        if s.undo_stack.len() > 32 {
            s.undo_stack.remove(0);
        }
        s.redo_stack.clear();
        save_layout(s);
        win::log(&format!(
            "layout commit: width={} splits={},{} (undo={})",
            s.width,
            s.split0,
            s.split1,
            s.undo_stack.len()
        ));
    }
}

fn apply_snapshot(s: &mut State, snapshot: LayoutSnapshot) {
    s.width = snapshot.width;
    s.split0 = snapshot.split0;
    s.split1 = snapshot.split1;
    apply_appbar(s);
    sync_slot(s, true);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

/// Cancel: restaura o snapshot sem gravar (Esc durante o arrasto).
fn cancel_edit(s: &mut State) {
    if let Some(pre) = s.edit_pre.take() {
        apply_snapshot(s, pre);
        win::log("layout cancelado (Esc): estado restaurado, nada gravado");
    }
}

fn undo_layout(s: &mut State) {
    if let Some(previous) = s.undo_stack.pop() {
        s.redo_stack.push(layout_snapshot(s));
        apply_snapshot(s, previous);
        save_layout(s);
        win::log("layout undo");
    }
}

fn redo_layout(s: &mut State) {
    if let Some(next) = s.redo_stack.pop() {
        s.undo_stack.push(layout_snapshot(s));
        apply_snapshot(s, next);
        save_layout(s);
        win::log("layout redo");
    }
}

fn save_layout(s: &State) {
    Config {
        width: s.width,
        slots: s.slot_count,
        split0: s.split0,
        split1: s.split1,
        legacy_width: None,
    }
    .save();
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
                if s.bottom_mode {
                    HWND_BOTTOM
                } else {
                    HWND_TOPMOST
                },
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
    pin_window(s, hwnd, None)
}

/// Encaixa em um slot específico (se livre); senão, no primeiro livre.
fn pin_window(s: &mut State, hwnd: HWND, preferred: Option<usize>) -> bool {
    if hwnd.is_null() || hwnd == s.hwnd {
        return false;
    }
    if s.slots.iter().flatten().any(|slot| slot.hwnd == hwnd) {
        return false;
    }
    let free = preferred
        .filter(|index| *index < s.slot_count && s.slots[*index].is_none())
        .or_else(|| s.slots.iter().position(|slot| slot.is_none()));
    let Some(free) = free else {
        win::log("todas as divisões ocupadas; libere uma janela antes");
        return false;
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
    // O SetWindowPos do sync_slot é assíncrono para a janela alvo: medir na hora
    // pode ler o tamanho antigo (ex.: 1092px) e alargar o dock sem necessidade.
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        needed = 0;
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
        if needed == 0 {
            break;
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

fn free_slot_index(s: &State) -> Option<usize> {
    s.slots.iter().position(|slot| slot.is_none())
}

/// Encaixa se houver slot livre; senão esconde e põe na fila do rodapé.
fn enqueue_or_pin(s: &mut State, hwnd: HWND) -> Value {
    if let Some(index) = free_slot_index(s) {
        if pin_window(s, hwnd, Some(index)) {
            return json!({ "pinned": true, "slot": index });
        }
    }
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
    let title = window_title(hwnd)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| {
            let label = process_label(hwnd);
            if label.is_empty() {
                "janela".to_string()
            } else {
                label
            }
        });
    s.pending.push(PendingApp { hwnd, title });
    sync_slot(s, true);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log(&format!(
        "fila: janela 0x{:X} aguardando slot (posição {})",
        hwnd as usize,
        s.pending.len()
    ));
    json!({
        "queued": true,
        "position": s.pending.len(),
        "pending": s.pending.len(),
        "slot_count": s.slot_count
    })
}

/// Promove o próximo da fila para cada slot que vagar.
fn promote_pending(s: &mut State) {
    let mut changed = false;
    loop {
        let Some(index) = free_slot_index(s) else {
            break;
        };
        let Some(pending) = s.pending.first().cloned() else {
            break;
        };
        s.pending.remove(0);
        if unsafe { IsWindow(pending.hwnd) } == 0 {
            changed = true;
            continue;
        }
        if pin_window(s, pending.hwnd, Some(index)) {
            unsafe {
                ShowWindow(pending.hwnd, SW_SHOWNOACTIVATE);
            }
            win::log(&format!(
                "fila: encaixando {:?} no slot {index}",
                pending.title
            ));
            changed = true;
        } else {
            s.pending.insert(0, pending);
            break;
        }
    }
    if changed || !s.pending.is_empty() {
        sync_slot(s, true);
        unsafe {
            InvalidateRect(s.hwnd, ptr::null(), 0);
        }
    }
}

/// Esvazia a fila e devolve as janelas ao desktop (usado ao liberar tudo).
fn clear_pending(s: &mut State) {
    let pending = std::mem::take(&mut s.pending);
    for app in pending {
        if unsafe { IsWindow(app.hwnd) } != 0 {
            unsafe {
                ShowWindow(app.hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }
    sync_slot(s, true);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
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
    promote_pending(s);
}

fn release_all(s: &mut State) {
    clear_pending(s);
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
        split0: s.split0,
        split1: s.split1,
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

/// "Fechar" (✕) = esconder para a bandeja e liberar a área do desktop.
/// A saída real fica no menu da bandeja ("Sair do MayerDock").
fn close_to_tray(s: &mut State) {
    if !s.manual_hidden {
        release_all(s);
        s.manual_hidden = true;
    }
    update_visibility(s);
    tray_update(s);
    win::log("dock: fechado para a bandeja (clique no ícone reabre; menu Sair encerra)");
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

        // Tema automático (claro/escuro do Windows); atualizado a cada ~5 s.
        let dark = s.theme_dark;
        let bg_color = if dark {
            rgb(18, 18, 22)
        } else {
            rgb(244, 245, 249)
        };
        let header_color = if dark {
            rgb(30, 30, 36)
        } else {
            rgb(230, 232, 240)
        };
        let title_color = if dark {
            rgb(236, 238, 244)
        } else {
            rgb(24, 26, 34)
        };
        let dim_color = if dark {
            rgb(146, 150, 164)
        } else {
            rgb(88, 92, 106)
        };
        let bg = CreateSolidBrush(bg_color);
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
        let hb = CreateSolidBrush(header_color);
        FillRect(hdc, &hdr, hb);
        DeleteObject(hb);

        let edge = RECT {
            left: 0,
            top: 0,
            right: px(s, 2).max(1),
            bottom: h,
        };
        // Borda de foco por sessão (md-d33.3.5): lease = âmbar, agente = cor
        // estável pelo nome (hash), sem sessão = azul padrão.
        let edge_color = if s.lease.is_some() {
            rgb(245, 158, 11)
        } else if let Some(agent) = &s.agent {
            session_color(&agent.name)
        } else {
            rgb(74, 112, 240)
        };
        let eb = CreateSolidBrush(edge_color);
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
            title_color,
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
        // Pair testing: "✋" assume o controle (pausa tools); "▶" devolve ao agente.
        let control = if s.agent.is_some() {
            RECT {
                left: stop.left - btn_w - btn_gap,
                top: close.top,
                right: stop.left - btn_gap,
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
        if control.right > control.left {
            draw_button(
                hdc,
                s.font_ui,
                &control,
                if s.agent_paused { "▶" } else { "✋" },
                true,
            );
        }
        s.btn_menu = menu_b;
        s.btn_close = close;
        s.btn_release = release;
        s.btn_stop = stop;
        s.btn_control = control;

        // HUD do agente no cabeçalho (nome + atividade atual).
        if let Some(agent) = &s.agent {
            let hud_right = if control.right > control.left {
                control.left
            } else if stop.right > stop.left {
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
                let mut text = format!("● {} · {} · {}", agent.name, agent.project, agent.activity);
                if s.agent_paused {
                    text.push_str(" · você no controle");
                }
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
                dim_color,
                "Nenhuma janela encaixada.\r\n\r\nClique aqui ou no ☰\r\npara escolher uma janela\r\ne ocupar o dock.",
                &mut hint,
                DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
            );
        }

        // Confetti de fim de sessão (>= 10 ações) por ~0,7 s no cabeçalho.
        if let Some(until) = s.confetti_until {
            if std::time::Instant::now() < until {
                paint_confetti(hdc, s, w, header_h);
            }
        }

        // Fila de espera no rodapé: "▶ Próximo: <título> (+N)".
        if !s.pending.is_empty() {
            let bar_h = px(s, 26);
            let bar = RECT {
                left: px(s, 8),
                top: h - bar_h - px(s, 4),
                right: w - px(s, 8),
                bottom: h - px(s, 4),
            };
            let bar_bg = CreateSolidBrush(header_color);
            FillRect(hdc, &bar, bar_bg);
            DeleteObject(bar_bg);
            let mut label = format!("▶ Próximo: {}", s.pending[0].title);
            if s.pending.len() > 1 {
                label.push_str(&format!(" (+{} na fila)", s.pending.len() - 1));
            }
            let mut text_rect = bar;
            text_rect.left += px(s, 8);
            draw_text(
                hdc,
                s.font_hint,
                title_color,
                &label,
                &mut text_rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
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
    let (x, y) = client_point(lparam);
    if x <= px(s, EDGE_GRAB) {
        begin_edit(s);
        s.resizing = true;
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut p);
            SetCapture(s.hwnd);
        }
        s.resize_start_x = p.x;
        s.resize_start_w = s.width;
        return;
    }
    // Divisórias arrastáveis: pega a fronteira entre slots se o clique estiver
    // perto dela (abaixo do cabeçalho).
    if y > px(s, HEADER_H) && s.slot_count > 1 {
        let c = content_rect(s);
        let gap = px(s, 4);
        for boundary in 0..s.slot_count - 1 {
            let rect = slot_rect(s, boundary);
            let center = rect.bottom + gap / 2;
            if (y - center).abs() <= px(s, 7).max(3) && x >= c.left && x <= c.right {
                begin_edit(s);
                s.drag_split = Some(boundary);
                unsafe {
                    SetCapture(s.hwnd);
                }
                win::log(&format!("divisória {boundary}: arrasto iniciado"));
                return;
            }
        }
    }
}

fn on_lbutton_up(s: &mut State, lparam: LPARAM) {
    if s.resizing {
        s.resizing = false;
        unsafe {
            ReleaseCapture();
        }
        commit_edit(s);
        win::log(&format!(
            "largura salva: {} lógicos ({} físicos)",
            s.width,
            s.physical_width()
        ));
        return;
    }
    if s.drag_split.take().is_some() {
        unsafe {
            ReleaseCapture();
        }
        commit_edit(s);
        win::log(&format!("divisórias salvas: {},{}", s.split0, s.split1));
        return;
    }

    let (x, y) = client_point(lparam);
    if y < px(s, HEADER_H) {
        if point_in(&s.btn_control, x, y) {
            toggle_human_control(s);
            return;
        }
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
    if let Some(boundary) = s.drag_split {
        let (_, y) = cursor_point();
        let c = content_rect(s);
        let gap = px(s, 4);
        let usable = (c.bottom - c.top - gap * (s.slot_count as i32 - 1)).max(1);
        let percent = (((y - c.top) as f64 / usable as f64) * 100.0).round() as i32;
        // Mínimo de ~12% por slot; mantém percentuais crescentes.
        const MIN_PCT: i32 = 12;
        if boundary == 0 {
            let max = if s.slot_count >= 3 {
                s.split1 - MIN_PCT
            } else {
                100 - MIN_PCT
            };
            s.split0 = percent.clamp(MIN_PCT, max.max(MIN_PCT));
        } else {
            s.split1 = percent.clamp(s.split0 + MIN_PCT, 100 - MIN_PCT);
        }
        sync_slot(s, true);
        unsafe {
            InvalidateRect(s.hwnd, ptr::null(), 0);
        }
        return;
    }
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

fn cursor_point() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

fn set_cursor(hwnd: HWND, s: &State) {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc);
        let cursor = if p.x - rc.left <= px(s, EDGE_GRAB) {
            LoadCursorW(ptr::null_mut(), IDC_SIZEWE)
        } else if s.slot_count > 1 {
            // Perto de uma divisória horizontal → cursor de resize vertical.
            let gap = px(s, 4);
            if (0..s.slot_count - 1).any(|boundary| {
                let rect = slot_rect(s, boundary);
                let center = rc.top + rect.bottom + gap / 2;
                (p.y - center).abs() <= px(s, 7).max(3)
            }) {
                LoadCursorW(ptr::null_mut(), IDC_SIZENS)
            } else {
                LoadCursorW(ptr::null_mut(), IDC_ARROW)
            }
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

    if !s.favorites.is_empty() {
        items.push(menu::MenuItem {
            label: String::new(),
            cmd: 0,
            kind: menu::Kind::Sep,
        });
        items.push(menu::MenuItem {
            label: "Launcher".to_string(),
            cmd: 0,
            kind: menu::Kind::Header,
        });
        for (index, (label, _)) in s.favorites.iter().enumerate() {
            if IDM_FAVORITE_BASE + index > IDM_FAVORITE_LAST {
                break;
            }
            items.push(menu::MenuItem {
                label: format!("▶ {label}"),
                cmd: IDM_FAVORITE_BASE + index,
                kind: menu::Kind::Item,
            });
        }
    }

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
            // Sair de verdade: destroy (cleanup remove AppBar/tray).
            DestroyWindow(s.hwnd);
        },
        IDM_TOGGLE => toggle_manual(s),
        IDM_STOP_AGENT => stop_agent(s),
        IDM_CONTROL => toggle_human_control(s),
        IDM_OVERLAY => toggle_overlay(s),
        IDM_MCP_HELP => crate::mcp_help::show(s.hwnd),
        IDM_RESET_WORKAREA => {
            let ok = win::reset_workarea();
            win::log(&format!(
                "área de trabalho restaurada: {}",
                if ok { "ok" } else { "falhou" }
            ));
        }
        IDM_BOTTOM => toggle_bottom(s),
        IDM_FAVORITE_BASE..=IDM_FAVORITE_LAST => {
            if let Some((label, command)) = s.favorites.get(id - IDM_FAVORITE_BASE).cloned() {
                win::log(&format!("launcher: favorito {label:?}"));
                launch_favorite(&command);
            }
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
    std::path::Path::new(&process_path(hwnd))
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Caminho completo do executável dono da janela (para workspaces).
fn process_path(hwnd: HWND) -> String {
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
        String::from_utf16_lossy(&buf[..size as usize])
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
    s.tray_created = tray_add(s);
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
    // O Explorer pode ainda estar subindo; tenta de novo por ~6 s.
    if !s.tray_created || (!s.hidden && !s.registered) {
        s.shell_retry = 15;
        unsafe {
            SetTimer(s.hwnd, TIMER_SHELL_RETRY, 400, None);
        }
        win::log("shell: agendando retry de bandeja/AppBar");
    }
    keep_flux_above();
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

/// Uma tentativa de re-registrar bandeja/AppBar após restart do Explorer.
fn retry_shell_registration(s: &mut State) {
    s.shell_retry = s.shell_retry.saturating_sub(1);
    if !s.tray_created {
        tray_remove(s);
        s.tray_created = tray_add(s);
    }
    if !s.hidden && !s.registered {
        remove_appbar(s);
        register_appbar(s);
        if s.registered {
            win::log("appbar re-registrada (retry)");
            apply_appbar(s);
            sync_slot(s, true);
        }
    }
    let done = s.tray_created && (s.hidden || s.registered);
    if done {
        win::log("shell: bandeja/AppBar re-registradas com sucesso");
        unsafe {
            KillTimer(s.hwnd, TIMER_SHELL_RETRY);
        }
    } else if s.shell_retry == 0 {
        win::log("aviso: bandeja/AppBar não voltaram após retries; reabra o dock se persistir");
        unsafe {
            KillTimer(s.hwnd, TIMER_SHELL_RETRY);
        }
    }
}

/// `--status`: versão, modo (pacote/portátil), config e caminhos reais.
pub fn print_status(json_output: bool) {
    let cfg = Config::load();
    let packaged = win::package_full_name();
    let data = config::resolved_data_dir();
    let config_path = data.join("config.ini");
    let log_path = data.join("mayerdock.log");
    if json_output {
        let value = json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "packaged": packaged.is_some(),
            "package": packaged,
            "width_logical": cfg.width,
            "slots": cfg.slots,
            "split0": cfg.split0,
            "split1": cfg.split1,
            "data_dir": data.display().to_string(),
            "config": config_path.display().to_string(),
            "log": log_path.display().to_string(),
        });
        win::console_print(&format!(
            "{}\r\n",
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
        ));
        return;
    }
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
    if cfg.slots > 1 {
        out.push_str(&format!("splits: {},{} %%\r\n", cfg.split0, cfg.split1));
    }
    out.push_str(&format!("data_dir: {}\r\n", data.display()));
    out.push_str(&format!("config: {}\r\n", config_path.display()));
    out.push_str(&format!("log: {}\r\n", log_path.display()));
    win::console_print(&out);
}

// ---------------------------------------------------------------------------
// Agent Workspace — servidor do named pipe (sidecar MCP ↔ dock)
// ---------------------------------------------------------------------------

/// Nome do pipe usado pelo sidecar MCP (`\\.\pipe\MayerDock.Agent`).
const AGENT_PIPE_NAME: &str = r"\\.\pipe\MayerDock.Agent";

/// Nome do pipe do agente (override `MAYERDOCK_PIPE` para testes isolados).
fn agent_pipe_name() -> String {
    std::env::var("MAYERDOCK_PIPE").unwrap_or_else(|_| AGENT_PIPE_NAME.to_string())
}

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
    let name = win::wide(&agent_pipe_name());
    let mut create_failures: u32 = 0;
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
                // Durante o shutdown (kill switch/cleanup) a falha é esperada.
                if AGENT_STOP {
                    win::log("agent pipe: servidor encerrado (durante shutdown)");
                    return;
                }
                create_failures += 1;
                if create_failures >= 5 {
                    win::log("agent pipe: CreateNamedPipeW falhou 5x; desistindo");
                    return;
                }
                win::log("agent pipe: CreateNamedPipeW falhou (retry)");
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            create_failures = 0;
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
            if AGENT_STOP {
                win::log("agent pipe: servidor encerrado");
                return;
            }
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

/// Limite de linha no pipe do agente (proteção contra payloads gigantes).
const MAX_PIPE_LINE: usize = 1 << 20;

/// Converte os bytes de uma linha do pipe em `String` (lossy, sem panic).
fn decode_pipe_line(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_PIPE_LINE {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).to_string())
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
        if bytes.len() > MAX_PIPE_LINE {
            return None;
        }
    }
    decode_pipe_line(&bytes)
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
                    "exe": process_path(slot.hwnd),
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
            "dock_pin_at" => "encaixando janela",
            "dock_unpin" => "liberando janela",
            "dock_release_all" => "liberando janelas",
            "status" => "consultando dock",
            "session_hello" => "conectando",
            "overlay_event" => "mostrando ação",
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
            "split0": s.split0,
            "split1": s.split1,
            "docked": s.slots.iter().flatten().count(),
            "pending": s.pending.len(),
            "bottom_mode": s.bottom_mode,
            "paused": s.agent_paused,
            "agent": s.agent.as_ref().map(|agent| json!({
                "name": agent.name,
                "project": agent.project,
                "activity": agent.activity,
                "actions": agent.actions
            }))
        })),
        "layout_info" => {
            let c = content_rect(s);
            let slot_rects: Vec<Value> = (0..s.slot_count)
                .map(|index| {
                    let r = slot_rect(s, index);
                    json!({
                        "index": index,
                        "left": r.left,
                        "top": r.top,
                        "right": r.right,
                        "bottom": r.bottom
                    })
                })
                .collect();
            ok(json!({
                "width_logical": s.width,
                "width_px": s.physical_width(),
                "slots": s.slot_count,
                "split0": s.split0,
                "split1": s.split1,
                "content": {
                    "left": c.left,
                    "top": c.top,
                    "right": c.right,
                    "bottom": c.bottom
                },
                "slot_rects": slot_rects,
                "pending": s.pending.iter().map(|app| app.title.clone()).collect::<Vec<String>>(),
                "bottom_mode": s.bottom_mode,
                "undo": s.undo_stack.len(),
                "redo": s.redo_stack.len(),
                "editing": s.edit_pre.is_some(),
                "theme": if s.theme_dark { "dark" } else { "light" },
                "confetti": s.confetti_until.is_some(),
                "tray_tip": tray_tip(s),
                "favorites": s.favorites.iter().map(|(label, _)| label.clone()).collect::<Vec<String>>()
            }))
        }
        "control_status" => ok(json!({
            "paused": s.agent_paused,
            "agent": s.agent.as_ref().map(|agent| json!({
                "name": agent.name,
                "activity": agent.activity
            }))
        })),
        "layout_apply" => {
            let width = req
                .pointer("/args/width")
                .and_then(Value::as_i64)
                .unwrap_or(s.width as i64) as i32;
            let split0 = req
                .pointer("/args/split0")
                .and_then(Value::as_i64)
                .unwrap_or(s.split0 as i64) as i32;
            let split1 = req
                .pointer("/args/split1")
                .and_then(Value::as_i64)
                .unwrap_or(s.split1 as i64) as i32;
            let name = req
                .pointer("/args/name")
                .and_then(Value::as_str)
                .unwrap_or("workspace");
            let auto = std::env::var("MAYERDOCK_APPROVE_AUTO")
                .map(|value| value == "1")
                .unwrap_or(false);
            let approved = if auto {
                true
            } else {
                let text = win::wide(&format!(
                    "Aplicar workspace \"{name}\"?\n\nlargura={width}, splits={split0},{split1}\n\nProposto por agente — aprovação humana necessária."
                ));
                let caption = win::wide("MayerDock — aprovar workspace");
                unsafe {
                    MessageBoxW(
                        s.hwnd,
                        text.as_ptr(),
                        caption.as_ptr(),
                        MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
                    ) == IDYES as i32
                }
            };
            if !approved {
                win::log(&format!("workspace {name:?}: recusado pelo humano"));
                return ok(json!({ "approved": false, "applied": false }));
            }
            begin_edit(s);
            s.width = width.clamp(MIN_W, MAX_W);
            s.split0 = split0.clamp(10, 90);
            s.split1 = split1.clamp(10, 90);
            if s.split0 >= s.split1 {
                s.split1 = (s.split0 + 10).min(90);
            }
            apply_appbar(s);
            sync_slot(s, true);
            commit_edit(s);
            win::log(&format!(
                "workspace aplicado: {name} (width={} splits={},{})",
                s.width, s.split0, s.split1
            ));
            ok(json!({
                "approved": true,
                "applied": true,
                "width_logical": s.width,
                "split0": s.split0,
                "split1": s.split1
            }))
        }
        "list_slots" => ok(slots_json(s)),
        "dock_pin" => match req.pointer("/args/hwnd").and_then(Value::as_i64) {
            Some(raw) => {
                let hwnd = raw as usize as HWND;
                let pinned = pin_first_free(s, hwnd);
                ok(json!({ "pinned": pinned }))
            }
            None => err("informe args.hwnd"),
        },
        "dock_pin_at" => match req.pointer("/args/hwnd").and_then(Value::as_i64) {
            Some(raw) => {
                let hwnd = raw as usize as HWND;
                let slot = req
                    .pointer("/args/slot")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                let pinned = pin_window(s, hwnd, slot);
                let placed = s
                    .slots
                    .iter()
                    .position(|slot| slot.as_ref().map(|slot| slot.hwnd) == Some(hwnd));
                ok(json!({ "pinned": pinned, "slot": placed }))
            }
            None => err("informe args.hwnd"),
        },
        "dock_pin_or_queue" => match req.pointer("/args/hwnd").and_then(Value::as_i64) {
            Some(raw) => {
                let hwnd = raw as usize as HWND;
                ok(enqueue_or_pin(s, hwnd))
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
            let project = req
                .pointer("/args/project")
                .and_then(Value::as_str)
                .unwrap_or("projeto")
                .to_string();
            s.agent = Some(AgentInfo {
                activity: "conectado".to_string(),
                name: name.clone(),
                project: project.clone(),
                actions: 0,
                clicks: 0,
                captures: 0,
                pid,
            });
            win::log(&format!(
                "agent: sessão iniciada ({name} @ {project}, pid {pid})"
            ));
            tray_update(s);
            ok(json!({ "session": name }))
        }
        "session_bye" => {
            finish_agent_session(s, "session_bye");
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
            tray_update(s);
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
                tray_update(s);
                ok(json!({ "granted": true, "seconds": seconds }))
            } else {
                win::log("agent: lease negado pelo usuário");
                ok(json!({ "granted": false }))
            }
        }
        "overlay_event" => {
            overlay_add(s, req);
            ok(json!({ "shown": s.overlay_enabled }))
        }
        "session_pill" => {
            session_pill(s, req);
            ok(json!({ "shown": !s.overlay_hwnd.is_null() }))
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
    tray_update(s);
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
    finish_agent_session(s, "Parar");
    agent_pipe_close();
}

/// Pair testing: humano assume/devolve o controle (pausa tools do agente).
fn toggle_human_control(s: &mut State) {
    if s.agent.is_none() {
        return;
    }
    s.agent_paused = !s.agent_paused;
    if s.agent_paused {
        // Assumir cancela o lease: o humano está no comando.
        if let Some(lease) = s.lease.take() {
            if !lease.previous.is_null() && unsafe { IsWindow(lease.previous) } != 0 {
                unsafe {
                    SetForegroundWindow(lease.previous);
                }
            }
        }
    }
    let name = s
        .agent
        .as_ref()
        .map(|agent| agent.name.clone())
        .unwrap_or_default();
    win::log(&format!(
        "agent: controle humano {} ({name})",
        if s.agent_paused {
            "assumido"
        } else {
            "devolvido"
        }
    ));
    tray_update(s);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

fn cleanup(s: &mut State) {
    unsafe {
        KillTimer(s.hwnd, TIMER_TICK);
        KillTimer(s.hwnd, TIMER_OVERLAY);
        KillTimer(s.hwnd, TIMER_SHELL_RETRY);
        overlay_destroy(s);
        UnregisterHotKey(s.hwnd, HOTKEY_ID);
        UnregisterHotKey(s.hwnd, HOTKEY_STOP_ID);
        UnregisterHotKey(s.hwnd, HOTKEY_UNDO_ID);
        UnregisterHotKey(s.hwnd, HOTKEY_REDO_ID);
        UnregisterHotKey(s.hwnd, HOTKEY_CANCEL_ID);
        if !HOOK.is_null() {
            UnhookWinEvent(HOOK);
            HOOK = ptr::null_mut();
        }
        // Encerra o servidor do pipe de agentes (unblock do ConnectNamedPipe).
        AGENT_STOP = true;
        agent_pipe_close();
    }
    s.agent = None;
    s.overlay_pill = None;
    release_all(s);
    remove_appbar(s);
    tray_remove(s);
    let _ = std::fs::remove_file(config::data_dir().join("agent-token"));
    Config {
        width: s.width,
        slots: s.slot_count,
        split0: s.split0,
        split1: s.split1,
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

// ---------------------------------------------------------------------------
// Badge da bandeja + launcher de favoritos + confetti
// ---------------------------------------------------------------------------

fn favorites_path() -> std::path::PathBuf {
    crate::config::data_dir().join("favorites.txt")
}

fn load_favorites() -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(favorites_path()) else {
        return Vec::new();
    };
    let mut favorites = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (label, command) = match line.split_once('|') {
            Some((label, command)) => (label.trim(), command.trim()),
            None => (line, line),
        };
        if !command.is_empty() {
            favorites.push((label.to_string(), command.to_string()));
        }
        if favorites.len() >= 8 {
            break;
        }
    }
    favorites
}

fn launch_favorite(command: &str) {
    let file = crate::win::wide(command);
    let verb = crate::win::wide("open");
    unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNOACTIVATE,
        );
    }
    win::log(&format!("launcher: abrindo favorito {command:?}"));
}

fn tray_tip(s: &State) -> String {
    match &s.agent {
        Some(agent) => {
            let mut tip = if s.agent_paused {
                format!("MayerDock — humano no controle ({})", agent.name)
            } else {
                format!("MayerDock — ● {} @ {}", agent.name, agent.project)
            };
            if let Some(lease) = &s.lease {
                let remaining = lease
                    .until
                    .saturating_duration_since(std::time::Instant::now());
                tip.push_str(&format!(" · lease {}s", remaining.as_secs() + 1));
            }
            tip.push_str(&format!(" · {} ações", agent.actions));
            tip
        }
        None => {
            if s.manual_hidden {
                "MayerDock — na bandeja (clique para abrir)".to_string()
            } else {
                "MayerDock — ocioso".to_string()
            }
        }
    }
}

fn tray_update(s: &State) {
    let mut nid = tray_nid(s.hwnd);
    nid.uFlags = NIF_TIP;
    copy_wide(&mut nid.szTip, &tray_tip(s));
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Encerra a sessão (bye/kill) guardando stats e disparando o confetti.
fn finish_agent_session(s: &mut State, reason: &str) {
    let stats = s
        .agent
        .as_ref()
        .map(|agent| (agent.name.clone(), agent.actions, agent.captures));
    s.agent = None;
    s.agent_paused = false;
    if let Some((name, actions, captures)) = stats {
        if actions >= 10 {
            let summary =
                format!("sessão de {name} encerrada — {actions} ações, {captures} capturas");
            win::log(&format!("confetti: {summary}"));
            s.confetti_summary = summary;
            s.confetti_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(700));
            unsafe {
                SetTimer(s.hwnd, TIMER_CONFETTI, 40, None);
            }
        }
    }
    tray_update(s);
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
    win::log(&format!("agent: sessão encerrada ({reason})"));
}

/// Partículas determinísticas de 0,5–0,7 s no cabeçalho (GDI, sem deps).
fn paint_confetti(hdc: HDC, s: &State, width: i32, header_h: i32) {
    unsafe {
        let palette = [
            rgb(74, 112, 240),
            rgb(245, 158, 11),
            rgb(34, 197, 94),
            rgb(236, 72, 153),
            rgb(139, 92, 246),
            rgb(59, 130, 246),
        ];
        let elapsed = s
            .confetti_until
            .and_then(|until| until.checked_duration_since(std::time::Instant::now()))
            .map(|_| 0)
            .unwrap_or(0);
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15 ^ elapsed as u64;
        for index in 0..48u32 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let x = (seed % width.max(1) as u64) as i32;
            let y = ((seed >> 16) % header_h.max(1) as u64) as i32;
            let size = px(s, 3).max(2);
            let brush = CreateSolidBrush(palette[(index as usize) % palette.len()]);
            let particle = RECT {
                left: x,
                top: y,
                right: x + size,
                bottom: y + size,
            };
            FillRect(hdc, &particle, brush);
            DeleteObject(brush);
        }
        if !s.confetti_summary.is_empty() {
            let mut summary = RECT {
                left: px(s, 14),
                top: 0,
                right: width - px(s, 150),
                bottom: header_h,
            };
            draw_text(
                hdc,
                s.font_hint,
                rgb(245, 248, 255),
                &s.confetti_summary,
                &mut summary,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Overlay "screencast key": tooltips efêmeros das ações do agente
// ---------------------------------------------------------------------------

fn overlay_path() -> std::path::PathBuf {
    crate::config::data_dir().join("overlay.txt")
}

fn load_overlay_enabled() -> bool {
    if std::env::var("MAYERDOCK_OVERLAY").ok().as_deref() == Some("1") {
        return true;
    }
    std::fs::read_to_string(overlay_path())
        .map(|text| text.trim() == "1")
        .unwrap_or(false)
}

fn save_overlay_enabled(enabled: bool) {
    let _ = std::fs::create_dir_all(crate::config::data_dir());
    let _ = std::fs::write(overlay_path(), if enabled { "1" } else { "0" });
}

// ---------------------------------------------------------------------------
// Modo atrás (bottom mode): janelas do dock no fundo da pilha Z
// ---------------------------------------------------------------------------

fn bottom_path() -> std::path::PathBuf {
    crate::config::data_dir().join("bottom.txt")
}

fn load_bottom_mode() -> bool {
    if std::env::var("MAYERDOCK_BOTTOM").ok().as_deref() == Some("1") {
        return true;
    }
    std::fs::read_to_string(bottom_path())
        .map(|text| text.trim() == "1")
        .unwrap_or(false)
}

fn save_bottom_mode(enabled: bool) {
    let _ = std::fs::create_dir_all(crate::config::data_dir());
    let _ = std::fs::write(bottom_path(), if enabled { "1" } else { "0" });
}

fn toggle_bottom(s: &mut State) {
    s.bottom_mode = !s.bottom_mode;
    save_bottom_mode(s.bottom_mode);
    sync_slot(s, true);
    win::log(&format!(
        "modo atrás: {}",
        if s.bottom_mode { "ligado" } else { "desligado" }
    ));
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

/// Se uma janela do dock subiu para foreground durante a sessão do agente,
/// rebaixa de novo (suspenso quando o humano assume o controle).
fn reassert_bottom(s: &mut State) {
    if !s.bottom_mode || s.agent.is_none() || s.agent_paused {
        return;
    }
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() || foreground == s.hwnd {
        return;
    }
    if s.slots.iter().flatten().any(|slot| slot.hwnd == foreground) {
        unsafe {
            SetWindowPos(
                foreground,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        win::log("modo atrás: janela do teste rebaixada de novo");
    }
}

fn overlay_destroy(s: &mut State) {
    unsafe {
        if !s.overlay_hwnd.is_null() {
            DestroyWindow(s.overlay_hwnd);
            s.overlay_hwnd = ptr::null_mut();
        }
    }
}

fn overlay_ensure(s: &mut State) {
    if !s.overlay_enabled {
        return;
    }
    overlay_window_ensure(s);
}

fn overlay_window_ensure(s: &mut State) {
    unsafe {
        if s.overlay_hwnd.is_null() {
            let class = win::wide("MayerDock.Overlay.v1");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(overlay_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: win::hinstance(),
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE,
                class.as_ptr(),
                win::wide("MayerDock Overlay").as_ptr(),
                WS_POPUP,
                vx,
                vy,
                vw,
                vh,
                ptr::null_mut(),
                ptr::null_mut(),
                win::hinstance(),
                ptr::null_mut(),
            );
            if hwnd.is_null() {
                win::log("overlay: CreateWindowExW falhou");
                return;
            }
            SetLayeredWindowAttributes(hwnd, rgb(0, 0, 0), 255, LWA_COLORKEY);
            s.overlay_hwnd = hwnd;
            win::log("overlay: criado (screencast key)");
        }
        ShowWindow(s.overlay_hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(
            s.overlay_hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        SetTimer(s.hwnd, TIMER_OVERLAY, 50, None);
        InvalidateRect(s.overlay_hwnd, ptr::null(), 0);
    }
}

fn toggle_overlay(s: &mut State) {
    s.overlay_enabled = !s.overlay_enabled;
    save_overlay_enabled(s.overlay_enabled);
    if s.overlay_enabled {
        overlay_ensure(s);
    } else {
        s.overlay_events.clear();
        unsafe {
            KillTimer(s.hwnd, TIMER_OVERLAY);
        }
        if !s.overlay_hwnd.is_null() {
            unsafe {
                ShowWindow(s.overlay_hwnd, SW_HIDE);
            }
        }
    }
    win::log(&format!(
        "overlay: {}",
        if s.overlay_enabled {
            "ligado"
        } else {
            "desligado"
        }
    ));
    unsafe {
        InvalidateRect(s.hwnd, ptr::null(), 0);
    }
}

fn overlay_add(s: &mut State, req: &Value) {
    let kind = req
        .pointer("/args/kind")
        .and_then(Value::as_str)
        .unwrap_or("action");
    let text = req
        .pointer("/args/text")
        .and_then(Value::as_str)
        .unwrap_or(kind);
    let raw_x = req.pointer("/args/x").and_then(Value::as_i64).unwrap_or(-1) as i32;
    let raw_y = req.pointer("/args/y").and_then(Value::as_i64).unwrap_or(-1) as i32;
    let mut point = POINT { x: raw_x, y: raw_y };
    let target = req
        .pointer("/args/hwnd")
        .and_then(Value::as_i64)
        .map(|raw| raw as usize as HWND)
        .unwrap_or(ptr::null_mut());
    unsafe {
        if !target.is_null() && IsWindow(target) != 0 && raw_x >= 0 && raw_y >= 0 {
            ClientToScreen(target, &mut point);
        } else if raw_x < 0 || raw_y < 0 {
            GetCursorPos(&mut point);
        }
    }
    s.overlay_events.push(OverlayEvent {
        text: format!("{} · {}", kind, text),
        x: point.x,
        y: point.y,
        until: std::time::Instant::now() + std::time::Duration::from_millis(550),
    });
    if s.overlay_events.len() > 12 {
        s.overlay_events.remove(0);
    }
    if s.overlay_enabled {
        overlay_ensure(s);
    }
}

fn session_pill(s: &mut State, req: &Value) {
    let text = req
        .pointer("/args/text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let hwnd = req
        .pointer("/args/hwnd")
        .and_then(Value::as_i64)
        .map(|raw| raw as usize as HWND)
        .unwrap_or(ptr::null_mut());
    if text.is_empty() || hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    s.overlay_pill = Some(OverlayPill {
        text,
        hwnd,
        until: std::time::Instant::now() + std::time::Duration::from_millis(4500),
    });
    overlay_window_ensure(s);
    unsafe {
        SetTimer(s.hwnd, TIMER_OVERLAY, 50, None);
        if !s.overlay_hwnd.is_null() {
            InvalidateRect(s.overlay_hwnd, ptr::null(), 0);
        }
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            let black = CreateSolidBrush(rgb(0, 0, 0));
            FillRect(hdc, &rc, black);
            DeleteObject(black);
            if let Some(s) = state_opt() {
                let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                let now = std::time::Instant::now();
                for event in &s.overlay_events {
                    let remaining = event.until.saturating_duration_since(now);
                    if remaining.is_zero() {
                        continue;
                    }
                    let fade = (remaining.as_millis() as f32 / 550.0).clamp(0.0, 1.0);
                    let base = if event.text.starts_with("click") {
                        rgb(120, 200, 255)
                    } else if event.text.starts_with("scroll") {
                        rgb(120, 255, 160)
                    } else if event.text.starts_with("capture") {
                        rgb(255, 220, 120)
                    } else if event.text.starts_with("type") || event.text.starts_with("key") {
                        rgb(255, 150, 230)
                    } else {
                        rgb(255, 255, 255)
                    };
                    let r = ((base & 0xFF) as f32 * fade) as u8;
                    let g = (((base >> 8) & 0xFF) as f32 * fade) as u8;
                    let b = (((base >> 16) & 0xFF) as f32 * fade) as u8;
                    let mut text = RECT {
                        left: event.x - vx + px(s, 14),
                        top: event.y - vy - px(s, 12),
                        right: event.x - vx + px(s, 360),
                        bottom: event.y - vy + px(s, 12),
                    };
                    draw_text(
                        hdc,
                        s.font_hint,
                        rgb(r, g, b),
                        &event.text,
                        &mut text,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                // Pílula "cliente · projeto" na janela que o agente está usando.
                if let Some(pill) = &s.overlay_pill {
                    if now < pill.until && IsWindow(pill.hwnd) != 0 {
                        let mut window_rect: RECT = std::mem::zeroed();
                        GetWindowRect(pill.hwnd, &mut window_rect);
                        let label = format!("● {}", pill.text);
                        let label_wide = win::wide(&label);
                        let old_font = SelectObject(hdc, s.font_hint);
                        let mut measure: RECT = std::mem::zeroed();
                        DrawTextW(
                            hdc,
                            label_wide.as_ptr(),
                            -1,
                            &mut measure,
                            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
                        );
                        let pad = px(s, 8);
                        let text_w = measure.right - measure.left;
                        let text_h = measure.bottom - measure.top;
                        let x = window_rect.right - text_w - px(s, 20);
                        let y = window_rect.top + px(s, 10);
                        let card = RECT {
                            left: x - pad,
                            top: y - pad / 2,
                            right: x + text_w + pad,
                            bottom: y + text_h + pad / 2,
                        };
                        let bg = CreateSolidBrush(rgb(18, 20, 28));
                        FillRect(hdc, &card, bg);
                        DeleteObject(bg);
                        let accent = CreateSolidBrush(rgb(74, 112, 240));
                        FillRect(
                            hdc,
                            &RECT {
                                left: card.left,
                                top: card.top,
                                right: card.left + px(s, 3),
                                bottom: card.bottom,
                            },
                            accent,
                        );
                        DeleteObject(accent);
                        let mut tr = RECT {
                            left: x,
                            top: y,
                            right: x + text_w,
                            bottom: y + text_h,
                        };
                        draw_text(
                            hdc,
                            s.font_hint,
                            rgb(232, 238, 255),
                            &label,
                            &mut tr,
                            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
                        );
                        SelectObject(hdc, old_font);
                    }
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(hwnd, msg, _wparam, _lparam),
    }
}

fn tray_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

fn tray_add(s: &State) -> bool {
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
        false
    } else {
        if embedded.is_null() {
            win::log("ícone da bandeja criado (genérico; recurso embutido ausente)");
        } else {
            win::log("ícone da bandeja criado (embutido)");
        }
        true
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
            label: if s.agent_paused {
                format!("Devolver controle ({})", agent.name)
            } else {
                format!("Assumir controle ({})", agent.name)
            },
            cmd: IDM_CONTROL,
            kind: menu::Kind::Item,
        });
        items.push(menu::MenuItem {
            label: format!("Parar agente ({})", agent.name),
            cmd: IDM_STOP_AGENT,
            kind: menu::Kind::Item,
        });
    }
    // Launcher: favoritos (favorites.txt) no menu da bandeja.
    for (index, (label, _)) in s.favorites.iter().enumerate() {
        if IDM_FAVORITE_BASE + index > IDM_FAVORITE_LAST {
            break;
        }
        items.push(menu::MenuItem {
            label: format!("▶ {label}"),
            cmd: IDM_FAVORITE_BASE + index,
            kind: menu::Kind::Item,
        });
    }
    items.push(menu::MenuItem {
        label: format!(
            "{} ações do agente (overlay)",
            if s.overlay_enabled {
                "Ocultar"
            } else {
                "Mostrar"
            }
        ),
        cmd: IDM_OVERLAY,
        kind: menu::Kind::Item,
    });
    items.push(menu::MenuItem {
        label: String::new(),
        cmd: 0,
        kind: menu::Kind::Sep,
    });
    items.push(menu::MenuItem {
        label: format!(
            "Modo atrás (janelas no fundo): {}",
            if s.bottom_mode { "ligado" } else { "desligado" }
        ),
        cmd: IDM_BOTTOM,
        kind: menu::Kind::Item,
    });
    items.push(menu::MenuItem {
        label: "Restaurar área de trabalho".to_string(),
        cmd: IDM_RESET_WORKAREA,
        kind: menu::Kind::Item,
    });
    items.push(menu::MenuItem {
        label: "Instruções MCP (copiar)".to_string(),
        cmd: IDM_MCP_HELP,
        kind: menu::Kind::Item,
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
            if wparam == TIMER_OVERLAY {
                let now = std::time::Instant::now();
                s.overlay_events.retain(|event| event.until > now);
                if s.overlay_pill
                    .as_ref()
                    .map(|pill| now >= pill.until)
                    .unwrap_or(false)
                {
                    s.overlay_pill = None;
                }
                let active = !s.overlay_events.is_empty() || s.overlay_pill.is_some();
                if !active {
                    unsafe {
                        KillTimer(s.hwnd, TIMER_OVERLAY);
                    }
                    if !s.overlay_hwnd.is_null() {
                        unsafe {
                            ShowWindow(s.overlay_hwnd, SW_HIDE);
                        }
                    }
                } else if !s.overlay_hwnd.is_null() {
                    unsafe {
                        InvalidateRect(s.overlay_hwnd, ptr::null(), 0);
                    }
                }
                return 0;
            }
            if wparam == TIMER_SHELL_RETRY {
                retry_shell_registration(s);
                return 0;
            }
            if wparam == TIMER_CONFETTI {
                let finished = s
                    .confetti_until
                    .map(|until| std::time::Instant::now() >= until)
                    .unwrap_or(true);
                if finished {
                    s.confetti_until = None;
                    s.confetti_summary.clear();
                    unsafe {
                        KillTimer(s.hwnd, TIMER_CONFETTI);
                    }
                }
                unsafe {
                    InvalidateRect(s.hwnd, ptr::null(), 0);
                }
                return 0;
            }
            // Tema automático: consulta o registro a cada ~5 s.
            s.theme_check = s.theme_check.wrapping_add(1);
            if s.theme_check % 5 == 0 {
                let dark = win::theme_is_dark();
                if dark != s.theme_dark {
                    s.theme_dark = dark;
                    win::log(&format!("tema: {}", if dark { "escuro" } else { "claro" }));
                    unsafe {
                        InvalidateRect(s.hwnd, ptr::null(), 0);
                    }
                }
            }
            expire_lease(s);
            update_visibility(s);
            sync_slot(s, false);
            if !s.pending.is_empty() {
                promote_pending(s);
            }
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
            reassert_bottom(s);
            0
        }
        WM_HOTKEY => {
            let id = wparam as i32;
            if id == HOTKEY_ID {
                toggle_manual(s);
            } else if id == HOTKEY_STOP_ID && s.agent.is_some() {
                win::log("agent: hotkey Ctrl+Alt+Shift+K (parar agente)");
                stop_agent(s);
            } else if id == HOTKEY_UNDO_ID {
                undo_layout(s);
            } else if id == HOTKEY_REDO_ID {
                redo_layout(s);
            } else if id == HOTKEY_CANCEL_ID {
                cancel_edit(s);
            }
            0
        }
        WM_KEYDOWN => {
            // Esc durante uma edição de layout cancela (quando o dock tem foco).
            if wparam as u32 == VK_ESCAPE && (s.edit_pre.is_some() || s.drag_split.is_some()) {
                s.drag_split = None;
                s.resizing = false;
                unsafe {
                    ReleaseCapture();
                }
                cancel_edit(s);
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
            // "Fechar" = esconder para a bandeja (libera a área). Sair de
            // verdade só pelo menu da bandeja (IDM_EXIT).
            close_to_tray(s);
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

#[cfg(test)]
mod fuzz_tests {
    use super::*;

    fn rng(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    #[test]
    fn decode_pipe_line_corpus_extremos() {
        let cases: &[&[u8]] = &[
            b"",
            b"{}",
            b"\x00\x00\x00",
            b"{\"id\":1,\"tool\":\"auth\"}",
            b"\xff\xfe\xfd invalid utf8",
            b"{\"tool\":\"dock_pin\",\"args\":{\"hwnd\":-1}}",
            b"\r\n",
        ];
        for bytes in cases {
            let _ = decode_pipe_line(bytes);
        }
        // No limite e acima dele.
        assert!(decode_pipe_line(&vec![b'a'; MAX_PIPE_LINE]).is_some());
        assert!(decode_pipe_line(&vec![b'a'; MAX_PIPE_LINE + 1]).is_none());
    }

    #[test]
    fn decode_pipe_line_fuzz_random_never_panics() {
        let mut state = 0x1234_ABCD_5678_EF90u64;
        for _ in 0..20_000 {
            let len = (rng(&mut state) as usize) % 2048;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                bytes.push((rng(&mut state) % 256) as u8);
            }
            let decoded = decode_pipe_line(&bytes);
            if let Some(text) = decoded {
                // Lossy pode expandir bytes inválidos para U+FFFD (3 bytes), mas
                // o número de caracteres nunca passa do número de bytes.
                assert!(text.chars().count() <= bytes.len());
            }
        }
    }
}
