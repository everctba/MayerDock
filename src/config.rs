//! Configuração persistida em `%APPDATA%\MayerDock\config.ini`.

use std::path::PathBuf;

/// Largura padrão do dock, em pixels físicos.
pub const DEFAULT_WIDTH: i32 = 380;
/// Quantas janelas empilhadas verticalmente por padrão.
pub const DEFAULT_SLOTS: usize = 2;

/// Pasta de dados do app.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("MayerDock")
}

/// Caminho do `config.ini`.
pub fn config_path() -> PathBuf {
    data_dir().join("config.ini")
}

#[derive(Clone, Copy)]
pub struct Config {
    pub width: i32,
    /// Quantas divisões verticais o dock tem (1–3).
    pub slots: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            width: DEFAULT_WIDTH,
            slots: DEFAULT_SLOTS,
        }
    }
}

impl Config {
    /// Lê o `config.ini` (ausente/ inválido → padrões).
    pub fn load() -> Config {
        let mut cfg = Config::default();
        if let Ok(text) = std::fs::read_to_string(config_path()) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim().eq_ignore_ascii_case("width") {
                        if let Ok(n) = value.trim().parse::<i32>() {
                            cfg.width = n.clamp(240, 1200);
                        }
                    } else if key.trim().eq_ignore_ascii_case("slots") {
                        if let Ok(n) = value.trim().parse::<usize>() {
                            cfg.slots = n.clamp(1, 3);
                        }
                    }
                }
            }
        }
        cfg
    }

    /// Grava o `config.ini` de forma atômica (tmp + rename).
    pub fn save(&self) {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let text = format!(
            "# MayerDock — configuração\r\nwidth={}\r\nslots={}\r\n",
            self.width, self.slots
        );
        let tmp = dir.join("config.ini.tmp");
        if std::fs::write(&tmp, text.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, config_path());
        }
    }
}
