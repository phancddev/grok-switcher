use crate::auth::{
    extract_email, extract_user_id, jwt_tier, primary_entry, read_auth_file, write_auth_file_atomic,
};
use crate::error::{AppError, AppResult};
use crate::paths::{accounts_dir, ensure_app_dirs, meta_path};
use crate::settings::Settings;
use crate::types::{AccountMeta, AccountSummary, AuthFile, MetaFile, QuotaInfo};
use std::fs;
use std::path::PathBuf;

pub fn load_meta() -> AppResult<MetaFile> {
    ensure_app_dirs()?;
    let path = meta_path()?;
    if !path.exists() {
        return Ok(MetaFile::default());
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(MetaFile::default());
    }
    Ok(serde_json::from_str(&raw)?)
}

pub fn save_meta(meta: &MetaFile) -> AppResult<()> {
    ensure_app_dirs()?;
    let path = meta_path()?;
    let raw = serde_json::to_string_pretty(meta)?;
    fs::write(&path, raw)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn account_snapshot_path(user_id: &str) -> AppResult<PathBuf> {
    // sanitize path component
    let safe: String = user_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(accounts_dir()?.join(format!("{safe}.json")))
}

pub fn save_account_snapshot(user_id: &str, auth: &AuthFile) -> AppResult<()> {
    ensure_app_dirs()?;
    let path = account_snapshot_path(user_id)?;
    write_auth_file_atomic(&path, auth)
}

pub fn load_account_snapshot(user_id: &str) -> AppResult<AuthFile> {
    let path = account_snapshot_path(user_id)?;
    read_auth_file(&path)?.ok_or_else(|| AppError::msg(format!("No snapshot for user {user_id}")))
}

pub fn remove_account_snapshot(user_id: &str) -> AppResult<()> {
    let path = account_snapshot_path(user_id)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn import_auth_as_account(auth: &AuthFile) -> AppResult<(String, AccountMeta)> {
    let (_, entry) = primary_entry(auth)?;
    let user_id = extract_user_id(entry)?;
    let email = extract_email(entry);
    let now = chrono::Utc::now().to_rfc3339();
    let meta = AccountMeta {
        email,
        first_name: entry.first_name.clone(),
        last_name: entry.last_name.clone(),
        label: None,
        last_used: Some(now.clone()),
        created_at: entry.create_time.clone().or(Some(now)),
        quota: None,
        tier: jwt_tier(&entry.key),
    };
    save_account_snapshot(&user_id, auth)?;
    Ok((user_id, meta))
}

pub fn list_summaries(settings: &Settings) -> AppResult<Vec<AccountSummary>> {
    let meta = load_meta()?;
    let active_from_file = detect_active_user_id(settings)?;
    let active = meta
        .active_user_id
        .clone()
        .or(active_from_file.clone());

    let mut out: Vec<AccountSummary> = meta
        .accounts
        .iter()
        .map(|(id, m)| AccountSummary {
            user_id: id.clone(),
            email: m.email.clone(),
            first_name: m.first_name.clone(),
            last_name: m.last_name.clone(),
            label: m.label.clone(),
            is_active: active.as_ref() == Some(id),
            last_used: m.last_used.clone(),
            created_at: m.created_at.clone(),
            quota: m.quota.clone(),
            tier: m.tier,
        })
        .collect();

    out.sort_by(|a, b| {
        b.is_active
            .cmp(&a.is_active)
            .then_with(|| a.email.to_lowercase().cmp(&b.email.to_lowercase()))
    });
    Ok(out)
}

pub fn detect_active_user_id(settings: &Settings) -> AppResult<Option<String>> {
    use crate::paths::auth_json_path;
    let path = auth_json_path(settings)?;
    let Some(auth) = read_auth_file(&path)? else {
        return Ok(None);
    };
    let (_, entry) = primary_entry(&auth)?;
    Ok(Some(extract_user_id(entry)?))
}

pub fn set_active(user_id: &str) -> AppResult<()> {
    let mut meta = load_meta()?;
    if !meta.accounts.contains_key(user_id) {
        return Err(AppError::msg(format!("Unknown account: {user_id}")));
    }
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(m) = meta.accounts.get_mut(user_id) {
        m.last_used = Some(now);
    }
    meta.active_user_id = Some(user_id.to_string());
    save_meta(&meta)
}

pub fn upsert_meta_account(user_id: &str, account: AccountMeta) -> AppResult<()> {
    let mut meta = load_meta()?;
    meta.accounts.insert(user_id.to_string(), account);
    meta.active_user_id = Some(user_id.to_string());
    save_meta(&meta)
}

pub fn update_quota(user_id: &str, quota: QuotaInfo, tier: Option<i64>) -> AppResult<()> {
    let mut meta = load_meta()?;
    if let Some(m) = meta.accounts.get_mut(user_id) {
        m.quota = Some(quota);
        if tier.is_some() {
            m.tier = tier;
        }
        save_meta(&meta)?;
    }
    Ok(())
}

pub fn get_access_token(user_id: &str) -> AppResult<String> {
    let auth = load_account_snapshot(user_id)?;
    let (_, entry) = primary_entry(&auth)?;
    Ok(entry.key.clone())
}
