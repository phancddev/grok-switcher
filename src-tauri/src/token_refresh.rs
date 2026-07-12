//! OIDC access-token refresh for stored Grok accounts.
//!
//! - On app start and periodically: refresh accounts whose access token is expired or
//!   within EARLY_REFRESH_SECS of expiry.

use crate::auth::{
    extract_user_id, jwt_claim, primary_entry, read_auth_file, write_auth_file_atomic,
};
use crate::error::{AppError, AppResult};
use crate::login::AUTH_LOCK;
use crate::paths::auth_json_path;
use crate::settings::{self, Settings};
use crate::store::{load_account_snapshot, load_meta, save_account_snapshot};
use crate::types::{AuthEntry, AuthFile};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde::Serialize;
use std::sync::Mutex;
use std::thread;
use std::time::Duration as StdDuration;

/// Refresh access tokens this many seconds before `expires_at` / JWT `exp`.
const EARLY_REFRESH_SECS: i64 = 10 * 60; // 10 minutes
/// How often the background loop scans for soon-to-expire tokens.
const PERIODIC_INTERVAL: StdDuration = StdDuration::from_secs(5 * 60); // 5 minutes

static REFRESH_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOneResult {
    pub user_id: String,
    pub ok: bool,
    pub message: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshAllReport {
    pub results: Vec<RefreshOneResult>,
    pub refreshed: u32,
    pub skipped: u32,
    pub failed: u32,
}

fn token_url(issuer: &str) -> AppResult<String> {
    let base = issuer.trim_end_matches('/');
    if base != "https://auth.x.ai" {
        return Err(AppError::msg(format!(
            "Refusing untrusted OIDC issuer: {base}"
        )));
    }
    Ok(format!("{base}/oauth2/token"))
}

fn access_expiry(entry: &AuthEntry) -> Option<DateTime<Utc>> {
    if let Some(ref s) = entry.expires_at {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    if let Some(exp) = jwt_claim(&entry.key, "exp").and_then(|s| s.parse::<i64>().ok()) {
        return DateTime::from_timestamp(exp, 0);
    }
    None
}

/// True if access is already expired or expires within EARLY_REFRESH_SECS.
pub fn needs_refresh(entry: &AuthEntry) -> bool {
    let Some(exp) = access_expiry(entry) else {
        // Unknown expiry → treat as needs refresh when we have refresh_token
        return entry.refresh_token.as_ref().is_some_and(|t| !t.is_empty());
    };
    let deadline = Utc::now() + Duration::seconds(EARLY_REFRESH_SECS);
    exp <= deadline
}

fn apply_token_response(entry: &mut AuthEntry, access_token: &str, resp: &TokenResponse) {
    entry.key = access_token.to_string();
    if let Some(ref rt) = resp.refresh_token {
        if !rt.is_empty() {
            entry.refresh_token = Some(rt.clone());
        }
    }
    let expires_in = resp.expires_in.unwrap_or(6 * 3600);
    let exp = Utc::now() + Duration::seconds(expires_in as i64);
    entry.expires_at = Some(exp.to_rfc3339());
}

/// Exchange refresh_token for a new access token (OIDC public client).
pub fn refresh_entry(entry: &mut AuthEntry) -> AppResult<()> {
    let refresh = entry
        .refresh_token
        .as_ref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::msg("No refresh_token for this account"))?
        .clone();

    let issuer = entry
        .oidc_issuer
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("https://auth.x.ai");
    let client_id = entry
        .oidc_client_id
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::msg("Missing oidc_client_id"))?
        .clone();

    let url = token_url(issuer)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .user_agent(concat!("grok-switcher/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let resp = client
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()?;

    let status = resp.status();
    let body = resp.text()?;
    let parsed: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        AppError::msg(format!(
            "Token refresh returned invalid JSON ({status}): {e}"
        ))
    })?;

    if !status.is_success() || parsed.error.is_some() {
        let err = parsed.error.as_deref().unwrap_or("request_failed");
        let desc = parsed.error_description.as_deref().unwrap_or_default();
        return Err(AppError::msg(format!(
            "Token refresh failed ({status}): {err} {desc}"
        )));
    }
    let access_token = parsed
        .access_token
        .as_deref()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| AppError::msg("Token refresh returned empty access_token"))?;

    apply_token_response(entry, access_token, &parsed);
    Ok(())
}

fn persist_auth_file(user_id: &str, auth: &AuthFile, settings: &Settings) -> AppResult<()> {
    save_account_snapshot(user_id, auth)?;

    // If this account is currently active in ~/.grok/auth.json, keep CLI in sync
    let active_path = auth_json_path(settings)?;
    if let Some(active) = read_auth_file(&active_path)? {
        if let Ok((_, active_entry)) = primary_entry(&active) {
            if let Ok(active_uid) = extract_user_id(active_entry) {
                if active_uid == user_id {
                    write_auth_file_atomic(&active_path, auth)?;
                }
            }
        }
    }
    Ok(())
}

/// Refresh one account's tokens.
/// - `force`: always call OIDC refresh (explicit manual refresh)
/// - `!force`: only if `needs_refresh`
pub fn refresh_account(user_id: &str, force: bool) -> AppResult<RefreshOneResult> {
    let _guard = REFRESH_LOCK
        .lock()
        .map_err(|_| AppError::msg("Token refresh lock poisoned"))?;

    let mut auth = load_account_snapshot(user_id)?;
    let key = auth
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| AppError::msg("Empty auth snapshot"))?;
    let entry = auth
        .get_mut(&key)
        .ok_or_else(|| AppError::msg("Missing auth entry"))?;
    let original_access_token = entry.key.clone();
    let original_refresh_token = entry.refresh_token.clone();

    if entry.refresh_token.as_ref().is_none_or(|t| t.is_empty()) {
        return Ok(RefreshOneResult {
            user_id: user_id.into(),
            ok: false,
            message: "No refresh_token — re-login required".into(),
            expires_at: entry.expires_at.clone(),
        });
    }

    if !force && !needs_refresh(entry) {
        return Ok(RefreshOneResult {
            user_id: user_id.into(),
            ok: true,
            message: "Access token still valid".into(),
            expires_at: entry.expires_at.clone(),
        });
    }

    match refresh_entry(entry) {
        Ok(()) => {
            let exp = entry.expires_at.clone();
            let refreshed_entry = entry.clone();
            let settings = settings::load_settings()?;

            // Synchronize the live auth-file read/check/write with login and switch operations.
            let _auth_guard = AUTH_LOCK
                .lock()
                .map_err(|_| AppError::msg("Auth lock poisoned"))?;
            let mut latest_auth = load_account_snapshot(user_id)?;
            let latest_entry = latest_auth
                .get_mut(&key)
                .ok_or_else(|| AppError::msg("Missing auth entry after refresh"))?;

            // Do not overwrite a re-login or another credential update completed in-flight.
            if latest_entry.key != original_access_token
                || latest_entry.refresh_token != original_refresh_token
            {
                return Ok(RefreshOneResult {
                    user_id: user_id.into(),
                    ok: true,
                    message: "Refresh skipped: credentials changed concurrently".into(),
                    expires_at: latest_entry.expires_at.clone(),
                });
            }

            latest_entry.key = refreshed_entry.key;
            latest_entry.refresh_token = refreshed_entry.refresh_token;
            latest_entry.expires_at = refreshed_entry.expires_at;
            persist_auth_file(user_id, &latest_auth, &settings)?;
            Ok(RefreshOneResult {
                user_id: user_id.into(),
                ok: true,
                message: if force {
                    "Refreshed".into()
                } else {
                    "Refreshed (near expiry)".into()
                },
                expires_at: exp,
            })
        }
        Err(e) => Ok(RefreshOneResult {
            user_id: user_id.into(),
            ok: false,
            message: e.to_string(),
            expires_at: entry.expires_at.clone(),
        }),
    }
}

/// Refresh every stored account.
/// `force_all`: true for an explicit manual refresh; false for background checks.
pub fn refresh_accounts(force_all: bool) -> RefreshAllReport {
    let meta = match load_meta() {
        Ok(m) => m,
        Err(e) => {
            return RefreshAllReport {
                results: vec![RefreshOneResult {
                    user_id: String::new(),
                    ok: false,
                    message: e.to_string(),
                    expires_at: None,
                }],
                refreshed: 0,
                skipped: 0,
                failed: 1,
            };
        }
    };

    let mut results = Vec::new();
    let mut refreshed = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    let ids: Vec<String> = meta.accounts.keys().cloned().collect();
    for user_id in ids {
        match refresh_account(&user_id, force_all) {
            Ok(r) => {
                if r.ok && (r.message.contains("still valid") || r.message.contains("skipped")) {
                    skipped += 1;
                } else if r.ok {
                    refreshed += 1;
                } else {
                    failed += 1;
                }
                results.push(r);
            }
            Err(e) => {
                failed += 1;
                results.push(RefreshOneResult {
                    user_id,
                    ok: false,
                    message: e.to_string(),
                    expires_at: None,
                });
            }
        }
    }

    RefreshAllReport {
        results,
        refreshed,
        skipped,
        failed,
    }
}

/// Ensure access token is fresh before API calls (lazy refresh).
pub fn ensure_fresh_token(user_id: &str) -> AppResult<()> {
    let r = refresh_account(user_id, false)?;
    if r.ok {
        return Ok(());
    }
    // If still valid message path already ok; failed near-expiry is an error for callers
    if r.message.contains("still valid") {
        return Ok(());
    }
    // No refresh_token: still allow try with existing access (may 401)
    if r.message.contains("No refresh_token") {
        return Ok(());
    }
    Err(AppError::msg(format!(
        "Token refresh failed for {user_id}: {}",
        r.message
    )))
}

/// Background: refresh near-expiry tokens on startup and every PERIODIC_INTERVAL.
pub fn spawn_background_refresh(app: tauri::AppHandle) {
    thread::spawn(move || {
        // Small delay so UI can finish first paint
        thread::sleep(StdDuration::from_secs(2));

        eprintln!("[token_refresh] startup: checking near-expiry tokens…");
        let report = refresh_accounts(false);
        eprintln!(
            "[token_refresh] startup done: refreshed={} skipped={} failed={}",
            report.refreshed, report.skipped, report.failed
        );
        let _ = app.emit("tokens-refreshed", &report);

        loop {
            thread::sleep(PERIODIC_INTERVAL);
            eprintln!("[token_refresh] periodic: checking near-expiry…");
            let report = refresh_accounts(false);
            if report.refreshed > 0 || report.failed > 0 {
                eprintln!(
                    "[token_refresh] periodic: refreshed={} skipped={} failed={}",
                    report.refreshed, report.skipped, report.failed
                );
                let _ = app.emit("tokens-refreshed", &report);
            }
        }
    });
}

// Bring Emitter into scope for app.emit
use tauri::Emitter;

#[cfg(test)]
mod tests {
    use super::token_url;

    #[test]
    fn accepts_xai_oidc_issuer() {
        assert_eq!(
            token_url("https://auth.x.ai/").unwrap(),
            "https://auth.x.ai/oauth2/token"
        );
    }

    #[test]
    fn rejects_untrusted_oidc_issuer() {
        assert!(token_url("http://auth.x.ai").is_err());
        assert!(token_url("https://example.com").is_err());
    }
}
