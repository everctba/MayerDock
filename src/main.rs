//! MayerDock — entrada do programa.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod dock;
mod menu;
mod win;

const HELP: &str = "MayerDock — dock lateral\r\n\
Uso:\r\n\
  mayerdock.exe            inicia o dock (bandeja)\r\n\
  mayerdock.exe --status   versão, modo (pacote), config e caminhos reais\r\n\
  mayerdock.exe --version  mostra a versão\r\n\
  mayerdock.exe --help     esta ajuda\r\n";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--status") => {
            dock::print_status();
            return;
        }
        Some("--version") | Some("-V") => {
            win::console_print(&format!("MayerDock {}\r\n", env!("CARGO_PKG_VERSION")));
            return;
        }
        Some("--help") | Some("-h") => {
            win::console_print(HELP);
            return;
        }
        _ => {}
    }

    let cfg = config::Config::load();
    let code = dock::run(cfg);
    std::process::exit(code);
}
