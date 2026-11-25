//! MayerDock — entrada do programa.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod dock;
mod mcp;
mod menu;
mod win;

const HELP: &str = "MayerDock — dock lateral\r\n\
Uso:\r\n\
  mayerdock.exe                      inicia o dock (bandeja)\r\n\
  mayerdock.exe --mcp                servidor MCP stdio (agent workspace)\r\n\
\r\n\
Console / CI (não abre o dock):\r\n\
  mayerdock.exe --status [--json]    versão, modo, config e caminhos\r\n\
  mayerdock.exe --list-windows [--json]\r\n\
  mayerdock.exe --capture (--title T | --hwnd N) [--out ARQ] [--json]\r\n\
  mayerdock.exe --script ARQ.json [--json] [--timeout ms]\r\n\
  mayerdock.exe --script ARQ.json --ci --report-junit junit.xml --report-json out.json\r\n\
  mayerdock.exe --version | --help\r\n\
\r\n\
Flags: --json (saída legível por máquina), --silent/--quiet (só JSON/erros),\r\n\
       --ci (sem modais/lease; relatório estável), --headless/--no-ui (nunca abre o dock).\r\n\
Saída: 0 = ok, 1 = erro de execução, 2 = uso inválido.\r\n";

fn usage_error(message: &str) -> ! {
    win::console_print(&format!("erro: {message}\r\n\r\n{HELP}"));
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        run_dock();
    }
    if args[0] == "--mcp" {
        std::process::exit(mcp::serve(env!("CARGO_PKG_VERSION")));
    }

    let mut json = false;
    let mut silent = false;
    let mut status = false;
    let mut list = false;
    let mut capture = false;
    let mut title: Option<String> = None;
    let mut hwnd: Option<i64> = None;
    let mut out: Option<String> = None;
    let mut script: Option<String> = None;
    let mut timeout: Option<u64> = None;
    let mut ci = false;
    let mut headless = false;
    let mut junit: Option<String> = None;
    let mut report_json: Option<String> = None;
    let _allow_global_input = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "--silent" | "--quiet" => silent = true,
            "--ci" => ci = true,
            "--headless" | "--no-ui" => headless = true,
            "--report-junit" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--report-junit exige um caminho");
                };
                junit = Some(value.clone());
            }
            "--report-json" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--report-json exige um caminho");
                };
                report_json = Some(value.clone());
            }
            "--status" => status = true,
            "--list-windows" | "--windows" => list = true,
            "--capture" => capture = true,
            "--title" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--title exige um valor");
                };
                title = Some(value.clone());
            }
            "--hwnd" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--hwnd exige um número");
                };
                hwnd = Some(
                    value
                        .parse::<i64>()
                        .unwrap_or_else(|_| usage_error("--hwnd inválido")),
                );
            }
            "--out" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--out exige um caminho");
                };
                out = Some(value.clone());
            }
            "--script" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--script exige um arquivo .json");
                };
                script = Some(value.clone());
            }
            "--timeout" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    usage_error("--timeout exige milissegundos");
                };
                timeout = Some(
                    value
                        .parse::<u64>()
                        .unwrap_or_else(|_| usage_error("--timeout inválido")),
                );
            }
            "--version" | "-V" => {
                win::console_print(&format!("MayerDock {}\r\n", env!("CARGO_PKG_VERSION")));
                return;
            }
            "--help" | "-h" => {
                win::console_print(HELP);
                return;
            }
            other => usage_error(&format!("argumento desconhecido: {other}")),
        }
        index += 1;
    }

    // --silent/--ci implicam saída JSON (sem texto humano).
    let machine = json || silent || ci;

    let has_console_command = status || list || capture || script.is_some();
    if (ci || headless) && !has_console_command {
        usage_error(
            "--ci/--headless exigem um comando (--status, --list-windows, --capture, --script)",
        );
    }
    if (junit.is_some() || report_json.is_some()) && script.is_none() {
        usage_error("--report-junit/--report-json exigem --script");
    }

    if status {
        dock::print_status(machine);
        return;
    }
    if list {
        std::process::exit(mcp::cli_list_windows(machine));
    }
    if capture {
        std::process::exit(mcp::cli_capture(hwnd, title, out, machine));
    }
    if let Some(path) = script {
        std::process::exit(mcp::cli_script(
            &path,
            machine,
            timeout,
            ci,
            junit.as_deref(),
            report_json.as_deref(),
        ));
    }

    // Nenhum comando de console: inicia o dock normalmente.
    run_dock();
}

fn run_dock() {
    let cfg = config::Config::load();
    let code = dock::run(cfg);
    std::process::exit(code);
}
