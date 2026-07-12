use crate::billing;
use crate::error::AppResult;
use crate::login::{self, LoginStatusEvent};
use crate::paths;
use crate::settings::{self, Settings};
use crate::store;
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
    let mut meta = store::load_meta()?;
    meta.accounts.remove(&user_id);
    if meta.active_user_id.as_deref() == Some(user_id.as_str()) {
        meta.active_user_id = None;
    }
    store::save_meta(&meta)?;
    store::remove_account_snapshot(&user_id)?;
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
        let uid = match user_id {
            Some(id) => id,
            None => store::detect_active_user_id(&s)?
                .or_else(|| store::load_meta().ok().and_then(|m| m.active_user_id))
                .ok_or_else(|| crate::error::AppError::msg("No active account"))?,
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

/// Always-available GitHub Releases check (no signing required).
#[tauri::command]
pub async fn check_github_update() -> AppResult<GithubUpdateInfo> {
    tauri::async_runtime::spawn_blocking(update::check_github_latest)
        .await
        .map_err(|e| crate::error::AppError::msg(format!("Update check task failed: {e}")))?
}
