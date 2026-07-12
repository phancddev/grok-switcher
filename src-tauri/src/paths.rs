use crate::error::{AppError, AppResult};
use crate::settings::Settings;
use std::path::{Path, PathBuf};

pub fn home_dir() -> AppResult<PathBuf> {
    dirs::home_dir().ok_or_else(|| AppError::msg("Could not resolve home directory"))
}

/// Expand leading `~/` and simple `%USERPROFILE%` on Windows.
pub fn expand_user_path(raw: &str) -> AppResult<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::msg("Empty path"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }
    if trimmed == "~" {
        return home_dir();
    }
    #[cfg(windows)]
    {
        let upper = trimmed.to_ascii_uppercase();
        if upper == "%USERPROFILE%" {
            return home_dir();
        }
        if let Some(rest) = trimmed
            .strip_prefix("%USERPROFILE%\\")
            .or_else(|| trimmed.strip_prefix("%USERPROFILE%/"))
            .or_else(|| trimmed.strip_prefix("%userprofile%\\"))
            .or_else(|| trimmed.strip_prefix("%userprofile%/"))
        {
            return Ok(home_dir()?.join(rest));
        }
    }
    Ok(PathBuf::from(trimmed))
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
            return expand_user_path(trimmed);
        }
    }
    if let Ok(env) = std::env::var("GROK_HOME") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return expand_user_path(trimmed);
        }
    }
    Ok(home_dir()?.join(".grok"))
}

pub fn auth_json_path(settings: &Settings) -> AppResult<PathBuf> {
    Ok(grok_home(settings)?.join("auth.json"))
}

pub fn ensure_app_dirs() -> AppResult<()> {
    let app = app_data_dir()?;
    let accounts = accounts_dir()?;
    std::fs::create_dir_all(&accounts)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::set_permissions(&accounts, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Resolve grok binary path.
pub fn resolve_grok_binary(settings: &Settings) -> AppResult<PathBuf> {
    if let Some(ref p) = settings.grok_binary_path {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            let path = expand_user_path(trimmed)?;
            if path.is_file() {
                return Ok(path);
            }
            return Err(AppError::msg(format!(
                "Configured grok binary not found: {}",
                path.display()
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

#[allow(dead_code)]
pub fn path_exists(p: &Path) -> bool {
    p.exists()
}
