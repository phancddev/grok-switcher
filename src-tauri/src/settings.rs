use crate::error::AppResult;
use crate::paths::{ensure_app_dirs, settings_path};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub grok_binary_path: Option<String>,
    #[serde(default)]
    pub grok_home: Option<String>,
}

pub fn load_settings() -> AppResult<Settings> {
    ensure_app_dirs()?;
    let path = settings_path()?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(Settings::default());
    }
    Ok(serde_json::from_str(&raw)?)
}

pub fn save_settings(settings: &Settings) -> AppResult<Settings> {
    ensure_app_dirs()?;
    let path = settings_path()?;
    let raw = serde_json::to_string_pretty(settings)?;
    fs::write(&path, raw)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(settings.clone())
}
