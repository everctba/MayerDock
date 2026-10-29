//! MayerDock — entrada do programa.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod dock;
mod menu;
mod win;

fn main() {
    let cfg = config::Config::load();
    let code = dock::run(cfg);
    std::process::exit(code);
}
