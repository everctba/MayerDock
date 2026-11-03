//! Embute o ícone (`assets/icon.ico`) e as informações de versão no executável
//! do Windows do MayerDock.
//!
//! Usa o compilador de recursos do toolchain em uso:
//! - MSVC: `rc.exe` (Windows SDK) → `.res`;
//! - GNU/MinGW: `windres` → `.o` COFF.
//!
//! Se nenhum compilador de recursos for encontrado, o build continua e o exe
//! fica sem ícone embutido; a bandeja cai no ícone genérico do Windows.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_default());
    let icon = manifest.join("assets").join("icon.ico");
    if !icon.is_file() {
        println!("cargo:warning=assets/icon.ico nao encontrado; o exe ficara sem icone");
        return;
    }

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let (v0, v1, v2, v3) = version_parts(&version);
    let icon_path = icon.to_string_lossy().replace('\\', "/");

    let rc = format!(
        "1 ICON \"{icon_path}\"\n\
         1 VERSIONINFO\n\
         FILEVERSION {v0},{v1},{v2},{v3}\n\
         PRODUCTVERSION {v0},{v1},{v2},{v3}\n\
         FILEFLAGSMASK 0x3fL\n\
         FILEFLAGS 0x0L\n\
         FILEOS 0x40004L\n\
         FILETYPE 0x1L\n\
         FILESUBTYPE 0x0L\n\
         BEGIN\n\
           BLOCK \"StringFileInfo\"\n\
           BEGIN\n\
             BLOCK \"040904b0\"\n\
             BEGIN\n\
               VALUE \"CompanyName\", \"MayerDock\"\n\
               VALUE \"FileDescription\", \"MayerDock - dock lateral que reserva uma faixa da tela\"\n\
               VALUE \"FileVersion\", \"{version}\"\n\
               VALUE \"InternalName\", \"mayerdock\"\n\
               VALUE \"OriginalFilename\", \"mayerdock.exe\"\n\
               VALUE \"ProductName\", \"MayerDock\"\n\
               VALUE \"ProductVersion\", \"{version}\"\n\
             END\n\
           END\n\
           BLOCK \"VarFileInfo\"\n\
           BEGIN\n\
             VALUE \"Translation\", 0x409, 1200\n\
           END\n\
         END\n"
    );
    let rc_path = out_dir.join("mayerdock_app.rc");
    if let Err(e) = fs::write(&rc_path, rc) {
        println!("cargo:warning=nao foi possivel escrever o .rc: {e}");
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let ok = if target_env == "msvc" {
        embed_msvc(&rc_path, &out_dir)
    } else {
        embed_gnu(&rc_path, &out_dir)
    };

    if !ok {
        println!(
            "cargo:warning=compilador de recursos (windres/rc.exe) nao encontrado; exe sem icone embutido"
        );
    }
}

fn version_parts(version: &str) -> (u32, u32, u32, u32) {
    let mut parts = [0u32; 4];
    for (i, piece) in version.split('.').take(4).enumerate() {
        parts[i] = piece.parse().unwrap_or(0);
    }
    (parts[0], parts[1], parts[2], parts[3])
}

/// Procura um executável: variável de ambiente, diretório do linker do target,
/// depois o PATH.
fn find_tool(exe_names: &[&str], env_vars: &[&str]) -> Option<PathBuf> {
    for var in env_vars {
        if let Ok(value) = env::var(var) {
            let path = PathBuf::from(value);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    let mut linker_dirs: Vec<PathBuf> = Vec::new();
    for (key, value) in env::vars() {
        if key.starts_with("CARGO_TARGET_") && key.ends_with("_LINKER") {
            if let Some(dir) = Path::new(&value).parent() {
                linker_dirs.push(dir.to_path_buf());
            }
        }
    }
    for dir in &linker_dirs {
        for name in exe_names {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    if let Ok(paths) = env::var("PATH") {
        for dir in env::split_paths(&paths) {
            for name in exe_names {
                let path = dir.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn embed_gnu(rc_path: &Path, out_dir: &Path) -> bool {
    let Some(windres) = find_tool(
        &["x86_64-w64-mingw32-windres.exe", "windres.exe", "windres"],
        &["WINDRES"],
    ) else {
        return false;
    };
    let obj = out_dir.join("mayerdock_app_res.o");
    let status = Command::new(&windres)
        .arg("-O")
        .arg("coff")
        .arg("-i")
        .arg(rc_path)
        .arg("-o")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() && obj.is_file() => {
            println!("cargo:rustc-link-arg={}", obj.display());
            true
        }
        Ok(s) => {
            println!("cargo:warning=windres falhou (status {s})");
            false
        }
        Err(e) => {
            println!("cargo:warning=windres nao executou: {e}");
            false
        }
    }
}

fn embed_msvc(rc_path: &Path, out_dir: &Path) -> bool {
    let Some(rc) = find_tool(&["rc.exe", "rc"], &["RC"]) else {
        return false;
    };
    let res = out_dir.join("mayerdock_app_res.res");
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(rc_path)
        .status();
    match status {
        Ok(s) if s.success() && res.is_file() => {
            println!("cargo:rustc-link-arg={}", res.display());
            true
        }
        Ok(s) => {
            println!("cargo:warning=rc.exe falhou (status {s})");
            false
        }
        Err(e) => {
            println!("cargo:warning=rc.exe nao executou: {e}");
            false
        }
    }
}
