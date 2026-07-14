use crate::error::{AppError, AppResult};
use crate::types::{AuthEntry, AuthFile};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub fn read_auth_file(path: &Path) -> AppResult<Option<AuthFile>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let map: AuthFile = serde_json::from_str(&raw)?;
    if map.is_empty() {
        return Ok(None);
    }
    Ok(Some(map))
}

pub fn write_auth_file_atomic(path: &Path, auth: &AuthFile) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(auth)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &raw)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    // rename is atomic on same filesystem
    fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn primary_entry(auth: &AuthFile) -> AppResult<(&String, &AuthEntry)> {
    auth.iter()
        .next()
        .ok_or_else(|| AppError::msg("auth.json has no entries"))
}

pub fn extract_user_id(entry: &AuthEntry) -> AppResult<String> {
    if let Some(ref id) = entry.user_id {
        if !id.is_empty() {
            return Ok(id.clone());
        }
    }
    if let Some(ref id) = entry.principal_id {
        if !id.is_empty() {
            return Ok(id.clone());
        }
    }
    // fallback: JWT sub
    if let Some(sub) = jwt_claim(&entry.key, "sub") {
        return Ok(sub);
    }
    Err(AppError::msg("Could not determine user_id from auth entry"))
}

pub fn extract_email(entry: &AuthEntry) -> String {
    entry
        .email
        .clone()
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "unknown@local".into())
}

pub fn jwt_tier(token: &str) -> Option<i64> {
    jwt_claim(token, "tier").and_then(|s| s.parse().ok())
}

pub fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = parts[1];
    // Try common JWT base64 variants (URL-safe unpadded first, then padded / standard).
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .or_else(|_| STANDARD_NO_PAD.decode(payload))
        .or_else(|_| STANDARD.decode(payload))
        .ok()?;
    let v: Value = serde_json::from_slice(&decoded).ok()?;
    match v.get(claim)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

pub fn fingerprint(auth: &AuthFile) -> String {
    if let Ok((_, entry)) = primary_entry(auth) {
        let uid = entry.user_id.clone().unwrap_or_default();
        let email = entry.email.clone().unwrap_or_default();
        let created = entry.create_time.clone().unwrap_or_default();
        // Include token prefix so refresh/re-login is detected even if create_time is stable.
        let key_fp = if entry.key.len() > 24 {
            &entry.key[..24]
        } else {
            entry.key.as_str()
        };
        return format!("{uid}|{email}|{created}|{key_fp}");
    }
    String::new()
}

pub fn backup_path(auth_path: &Path) -> PathBuf {
    auth_path.with_extension("json.bak-switcher")
}
