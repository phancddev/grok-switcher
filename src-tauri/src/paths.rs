use crate::error::{AppError, AppResult};
use crate::settings::Settings;
use std::path::PathBuf;

pub fn home_dir() -> AppResult<PathBuf> {
    dirs::home_dir().ok_or_else(|| AppError::msg("Could not resolve home directory"))
}

pub fn app_data_dir() -> AppResult<PathBuf> {
    Ok(home_dir()?.join(".grok-switcher"))
}

pub fn accounts_dir() -> AppResult<PathBuf> {
    Ok(app_data_dir()?.join("accounts"))
}

pub fn meta_path() -> AppResult<PathBuf> {
    Ok(app_data_dir()?.join("meta.json"))
}

pub fn settings_path() -> AppResult<PathBuf> {
    Ok(app_data_dir()?.join("settings.json"))
}

/// Resolve Grok home: settings.grokHome → GROK_HOME env → ~/.grok
pub fn grok_home(settings: &Settings) -> AppResult<PathBuf> {
    if let Some(ref p) = settings.grok_home {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Ok(env) = std::env::var("GROK_HOME") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    Ok(home_dir()?.join(".grok"))
}

pub fn auth_json_path(settings: &Settings) -> AppResult<PathBuf> {
    Ok(grok_home(settings)?.join("auth.json"))
}

pub fn ensure_app_dirs() -> AppResult<()> {
    std::fs::create_dir_all(accounts_dir()?)?;
    Ok(())
}

/// Resolve grok binary path.
pub fn resolve_grok_binary(settings: &Settings) -> AppResult<PathBuf> {
    if let Some(ref p) = settings.grok_binary_path {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_file() {
                return Ok(path);
            }
            return Err(AppError::msg(format!(
                "Configured grok binary not found: {trimmed}"
            )));
        }
    }

    let home = grok_home(settings)?;
    #[cfg(windows)]
    let candidates = [
        home.join("bin").join("grok.exe"),
        home.join("bin").join("grok"),
    ];
    #[cfg(not(windows))]
    let candidates = [home.join("bin").join("grok")];

    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
    }

    // PATH lookup
    #[cfg(windows)]
    let names = ["grok.exe", "grok"];
    #[cfg(not(windows))]
    let names = ["grok"];

    for name in names {
        if let Ok(path) = which::which(name) {
            return Ok(path);
        }
    }

    Err(AppError::msg(
        "Could not find `grok` binary. Install Grok Build or set the path in Settings.",
    ))
}
