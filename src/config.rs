//! Configuração persistida em `%APPDATA%\MayerDock\config.ini`.

use std::path::PathBuf;

/// Largura padrão do dock, em pixels lógicos (96 dpi).
pub const DEFAULT_WIDTH: i32 = 380;
/// Quantas janelas empilhadas verticalmente por padrão.
pub const DEFAULT_SLOTS: usize = 2;
/// Limites da largura, em pixels lógicos.
pub const MIN_WIDTH: i32 = 240;
pub const MAX_WIDTH: i32 = 1200;

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

/// Pasta de dados com o caminho **físico real** (resolvido). No modo MSIX o
/// `%APPDATA%` é redirecionado para dentro do pacote — o `--status` mostra este
/// caminho para o usuário encontrar config/log.
pub fn resolved_data_dir() -> PathBuf {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::canonicalize(&dir) {
        Ok(path) => {
            let shown = path.to_string_lossy();
            let stripped = shown.strip_prefix(r"\\?\").unwrap_or(&shown);
            PathBuf::from(stripped.to_string())
        }
        Err(_) => dir,
    }
}

#[derive(Clone, Copy)]
pub struct Config {
    /// Largura em pixels **lógicos** (96 dpi). Em runtime é multiplicada pelo
    /// fator de escala do monitor (`dpi / 96`).
    pub width: i32,
    /// Quantas divisões verticais o dock tem (1–3).
    pub slots: usize,
    /// Largura física de uma config antiga (antes do suporte a DPI), só para
    /// migrar: é convertida para lógica na inicialização e não é salva.
    pub legacy_width: Option<i32>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            width: DEFAULT_WIDTH,
            slots: DEFAULT_SLOTS,
            legacy_width: None,
        }
    }
}

impl Config {
    /// Lê o `config.ini` (ausente/inválido → padrões). Aceita `width`
    /// (formato antigo, físico) apenas para migração.
    pub fn load() -> Config {
        let mut cfg = Config::default();
        let mut has_logical = false;
        let mut legacy: Option<i32> = None;

        if let Ok(text) = std::fs::read_to_string(config_path()) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let key = key.trim();
                if key.eq_ignore_ascii_case("width_logical") {
                    if let Ok(n) = value.trim().parse::<i32>() {
                        cfg.width = n.clamp(MIN_WIDTH, MAX_WIDTH);
                        has_logical = true;
                    }
                } else if key.eq_ignore_ascii_case("width") {
                    if let Ok(n) = value.trim().parse::<i32>() {
                        legacy = Some(n);
                    }
                } else if key.eq_ignore_ascii_case("slots") {
                    if let Ok(n) = value.trim().parse::<usize>() {
                        cfg.slots = n.clamp(1, 3);
                    }
                }
            }
        }

        if !has_logical {
            cfg.legacy_width = legacy;
        }
        cfg
    }

    /// Grava o `config.ini` de forma atômica (tmp + rename).
    pub fn save(&self) {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let text = format!(
            "# MayerDock — configuração\r\n\
             # width_logical em pixels lógicos (96 dpi); o app escala pelo DPI do monitor\r\n\
             width_logical={}\r\n\
             slots={}\r\n",
            self.width, self.slots
        );
        let tmp = dir.join("config.ini.tmp");
        if std::fs::write(&tmp, text.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, config_path());
        }
    }
}
