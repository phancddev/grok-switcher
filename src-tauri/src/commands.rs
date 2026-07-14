use crate::billing;
use crate::error::AppResult;
use crate::login::{self, LoginStatusEvent};
use crate::paths;
use crate::settings::{self, Settings};
use crate::store;
use crate::token_refresh::{self, RefreshAllReport};
use crate::types::{AccountSummary, QuotaInfo};
use crate::update::{self, GithubUpdateInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn list_accounts() -> AppResult<Vec<AccountSummary>> {
    let s = settings::load_settings()?;
    store::list_summaries(&s)
}

#[tauri::command]
pub fn get_active() -> AppResult<Option<AccountSummary>> {
    let list = list_accounts()?;
    Ok(list.into_iter().find(|a| a.is_active))
}

fn summary_from_import(user_id: String, meta: crate::types::AccountMeta) -> AccountSummary {
    AccountSummary {
        user_id,
        email: meta.email,
        first_name: meta.first_name,
        last_name: meta.last_name,
        label: meta.label,
        is_active: true,
        last_used: meta.last_used,
        created_at: meta.created_at,
        quota: meta.quota,
        tier: meta.tier,
        subscription_tier: meta.subscription_tier,
        plan_expires_at: meta.plan_expires_at,
    }
}

/// Runs device-auth login; streams URL/code via event `login-status`.
#[tauri::command]
pub async fn add_account(app: AppHandle, label: Option<String>) -> AppResult<AccountSummary> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let s = settings::load_settings()?;
        let on_status: Arc<dyn Fn(LoginStatusEvent) + Send + Sync> = Arc::new(move |ev| {
            let _ = app2.emit("login-status", &ev);
        });
        let (user_id, meta) = login::run_add_account_arc(&s, label, on_status)?;
        Ok(summary_from_import(user_id, meta))
    })
    .await
    .map_err(|e| crate::error::AppError::msg(format!("Login task failed: {e}")))?
}

#[tauri::command]
pub async fn import_current_account(label: Option<String>) -> AppResult<AccountSummary> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = settings::load_settings()?;
        let (user_id, meta) = login::import_current(&s, label)?;
        Ok(summary_from_import(user_id, meta))
    })
    .await
    .map_err(|e| crate::error::AppError::msg(format!("Import task failed: {e}")))?
}

#[tauri::command]
pub async fn refresh_all_quotas() -> AppResult<HashMap<String, QuotaOrError>> {
    tauri::async_runtime::spawn_blocking(|| {
        let meta = store::load_meta()?;
        let mut map = HashMap::new();
        for user_id in meta.accounts.keys() {
            match billing::refresh_quota(user_id) {
                Ok(q) => {
                    map.insert(user_id.clone(), QuotaOrError::Ok(q));
                }
                Err(e) => {
                    map.insert(
                        user_id.clone(),
                        QuotaOrError::Err {
                            error: e.to_string(),
                        },
                    );
                }
            }
        }
        Ok(map)
    })
    .await
    .map_err(|e| crate::error::AppError::msg(format!("Quota refresh task failed: {e}")))?
}

#[tauri::command]
pub fn switch_account(user_id: String) -> AppResult<AccountSummary> {
    let s = settings::load_settings()?;
    login::switch_to(&s, &user_id)?;
    let list = store::list_summaries(&s)?;
    list.into_iter()
        .find(|a| a.user_id == user_id)
        .ok_or_else(|| crate::error::AppError::msg("Account switched but not found in list"))
}

#[tauri::command]
pub fn remove_account(user_id: String) -> AppResult<()> {
    // Lock order: REFRESH_LOCK outer, AUTH_LOCK inner (same as token_refresh) to avoid deadlock.
    let _refresh_guard = token_refresh::REFRESH_LOCK
        .lock()
        .map_err(|_| crate::error::AppError::msg("Token refresh lock poisoned"))?;
    let _auth_guard = login::AUTH_LOCK
        .lock()
        .map_err(|_| crate::error::AppError::msg("Auth lock poisoned"))?;

    let s = settings::load_settings()?;
    let original_meta = store::load_meta()?;
    let mut meta = original_meta.clone();
    if !meta.accounts.contains_key(&user_id) {
        return Err(crate::error::AppError::msg(format!(
            "Unknown account: {user_id}"
        )));
    }

    // Capture the exact live file so a failed multi-file mutation can restore it.
    let live_path = paths::auth_json_path(&s)?;
    let original_live_auth = crate::auth::read_auth_file(&live_path)?;
    let live_user_id = match original_live_auth.as_ref() {
        Some(auth) => {
            let (_, entry) = crate::auth::primary_entry(auth)?;
            Some(crate::auth::extract_user_id(entry)?)
        }
        None => None,
    };
    let live_is_this = live_user_id.as_deref() == Some(user_id.as_str());

    meta.accounts.remove(&user_id);

    // Prefer most recently used remaining account when live session must switch.
    let next_user_id = if live_is_this && !meta.accounts.is_empty() {
        let mut ids: Vec<(String, Option<String>)> = meta
            .accounts
            .iter()
            .map(|(id, m)| (id.clone(), m.last_used.clone()))
            .collect();
        // RFC3339 strings sort chronologically; None last_used sorts first (older).
        ids.sort_by(|a, b| b.1.cmp(&a.1));
        ids.into_iter().next().map(|(id, _)| id)
    } else {
        None
    };

    // Validate the next snapshot before mutating live auth or metadata.
    let next_auth = next_user_id
        .as_deref()
        .map(store::load_account_snapshot)
        .transpose()?;

    if live_is_this {
        login::replace_live_auth(&s, next_auth.as_ref())?;
        meta.active_user_id = next_user_id.clone();
        if let Some(next) = next_user_id.as_deref() {
            if let Some(account) = meta.accounts.get_mut(next) {
                account.last_used = Some(chrono::Utc::now().to_rfc3339());
            }
        }
    } else if let Some(ref live) = live_user_id {
        // Keep meta aligned with the real CLI session, but never mark unmanaged live auth active.
        meta.active_user_id = meta.accounts.contains_key(live).then(|| live.clone());
    } else if meta.active_user_id.as_deref() == Some(user_id.as_str()) {
        meta.active_user_id = None;
    }

    if let Err(save_error) = store::save_meta(&meta) {
        if live_is_this {
            if let Err(rollback_error) = login::replace_live_auth(&s, original_live_auth.as_ref()) {
                return Err(crate::error::AppError::msg(format!(
                    "Failed to save account removal: {save_error}; live-auth rollback also failed: {rollback_error}"
                )));
            }
        }
        return Err(save_error);
    }

    if let Err(remove_error) = store::remove_account_snapshot(&user_id) {
        let meta_rollback = store::save_meta(&original_meta).err();
        let live_rollback = if live_is_this {
            login::replace_live_auth(&s, original_live_auth.as_ref()).err()
        } else {
            None
        };

        if meta_rollback.is_some() || live_rollback.is_some() {
            return Err(crate::error::AppError::msg(format!(
                "Failed to remove account snapshot: {remove_error}; rollback failed (meta: {}, live auth: {})",
                meta_rollback
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "ok".into()),
                live_rollback
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "ok".into())
            )));
        }
        return Err(remove_error);
    }

    Ok(())
}

#[tauri::command]
pub fn set_account_label(user_id: String, label: Option<String>) -> AppResult<AccountSummary> {
    login::set_label(&user_id, label)?;
    let s = settings::load_settings()?;
    let list = store::list_summaries(&s)?;
    list.into_iter()
        .find(|a| a.user_id == user_id)
        .ok_or_else(|| crate::error::AppError::msg("Account not found after rename"))
}

#[tauri::command]
pub async fn refresh_quota(user_id: Option<String>) -> AppResult<QuotaInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = settings::load_settings()?;
        let meta = store::load_meta()?;
        let uid = match user_id {
            Some(id) => {
                if !meta.accounts.contains_key(&id) {
                    return Err(crate::error::AppError::msg(format!(
                        "Unknown account: {id}"
                    )));
                }
                id
            }
            None => {
                // A present unmanaged live session must not fall back to a saved account.
                match store::detect_active_user_id(&s)? {
                    Some(live) if meta.accounts.contains_key(&live) => live,
                    Some(_) => {
                        return Err(crate::error::AppError::msg(
                            "The active Grok CLI session is not managed by Grok Switcher",
                        ));
                    }
                    None => meta
                        .active_user_id
                        .clone()
                        .filter(|id| meta.accounts.contains_key(id))
                        .ok_or_else(|| crate::error::AppError::msg("No active account"))?,
                }
            }
        };
        billing::refresh_quota(&uid)
    })
    .await
    .map_err(|e| crate::error::AppError::msg(format!("Quota task failed: {e}")))?
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum QuotaOrError {
    Ok(QuotaInfo),
    Err { error: String },
}

#[tauri::command]
pub fn get_settings() -> AppResult<Settings> {
    settings::load_settings()
}

#[tauri::command]
pub fn save_settings(new_settings: Settings) -> AppResult<Settings> {
    settings::save_settings(&new_settings)
}

#[tauri::command]
pub fn resolve_grok_binary() -> AppResult<Option<String>> {
    let s = settings::load_settings()?;
    match paths::resolve_grok_binary(&s) {
        Ok(p) => Ok(Some(p.display().to_string())),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub fn get_app_version() -> String {
    update::app_version()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub release_date: Option<String>,
}

#[tauri::command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        version: update::app_version(),
        release_date: update::app_release_date(),
    }
}

/// Always-available GitHub Releases check (no signing required).
#[tauri::command]
pub async fn check_github_update() -> AppResult<GithubUpdateInfo> {
    tauri::async_runtime::spawn_blocking(update::check_github_latest)
        .await
        .map_err(|e| crate::error::AppError::msg(format!("Update check task failed: {e}")))?
}

/// Manually refresh OAuth access tokens for all stored accounts (force).
#[tauri::command]
pub async fn refresh_all_tokens() -> AppResult<RefreshAllReport> {
    tauri::async_runtime::spawn_blocking(|| token_refresh::refresh_accounts(true))
        .await
        .map_err(|e| crate::error::AppError::msg(format!("Token refresh task failed: {e}")))
}
