//! OIDC access-token refresh for stored Grok accounts.
//!
//! - On app start and periodically: refresh accounts whose access token is expired or
//!   within EARLY_REFRESH_SECS of expiry.
//!
//! ## Lock order (must be consistent everywhere to avoid deadlock)
//! 1. `REFRESH_LOCK` (outer)
//! 2. `AUTH_LOCK` (inner)
//!
//! Never take AUTH_LOCK then REFRESH_LOCK. Hold AUTH_LOCK only around disk I/O on
//! auth.json / snapshots — not across network OIDC calls.

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
/// Access token is still usable for API calls if it has at least this much life left.
const MIN_USABLE_ACCESS_SECS: i64 = 30;

/// Serialize token refresh per process. Outer lock — take before `AUTH_LOCK`.
pub(crate) static REFRESH_LOCK: Mutex<()> = Mutex::new(());

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

/// Effective access-token expiry: **minimum** of parseable `expires_at` and JWT `exp`.
/// Preferring only `expires_at` can leave a dead access token marked "still valid".
fn access_expiry(entry: &AuthEntry) -> Option<DateTime<Utc>> {
    let from_expires_at = entry.expires_at.as_ref().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    });
    let from_jwt = jwt_claim(&entry.key, "exp")
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|exp| DateTime::from_timestamp(exp, 0));

    match (from_expires_at, from_jwt) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// True if access is already expired or expires within EARLY_REFRESH_SECS.
///
/// Unknown expiry: do **not** auto-refresh every scan (avoids RT churn / death spiral).
/// Callers with `force=true` still call OIDC; `ensure_fresh_token` soft-fails if access works.
pub fn needs_refresh(entry: &AuthEntry) -> bool {
    let Some(exp) = access_expiry(entry) else {
        return false;
    };
    let deadline = Utc::now() + Duration::seconds(EARLY_REFRESH_SECS);
    exp <= deadline
}

fn should_refresh_entry(entry: &AuthEntry, force: bool, refresh_unknown_expiry: bool) -> bool {
    force || needs_refresh(entry) || (refresh_unknown_expiry && access_expiry(entry).is_none())
}

fn access_token_still_usable(entry: &AuthEntry) -> bool {
    match access_expiry(entry) {
        Some(exp) => exp > Utc::now() + Duration::seconds(MIN_USABLE_ACCESS_SECS),
        None => false,
    }
}

fn apply_token_response(entry: &mut AuthEntry, access_token: &str, resp: &TokenResponse) {
    entry.key = access_token.to_string();
    if let Some(ref rt) = resp.refresh_token {
        if !rt.is_empty() {
            entry.refresh_token = Some(rt.clone());
        }
    }
    // Prefer JWT `exp` when present; else expires_in with 1h default (not 6h).
    if let Some(exp) = jwt_claim(access_token, "exp").and_then(|s| s.parse::<i64>().ok()) {
        if let Some(dt) = DateTime::from_timestamp(exp, 0) {
            entry.expires_at = Some(dt.to_rfc3339());
            return;
        }
    }
    let expires_in = resp.expires_in.unwrap_or(3600);
    let exp = Utc::now() + Duration::seconds(expires_in as i64);
    entry.expires_at = Some(exp.to_rfc3339());
}

/// If live `auth.json` is this user and credentials differ, copy live → snapshot first.
/// Returns true when the snapshot was updated from live.
///
/// Caller should hold `AUTH_LOCK` (and typically `REFRESH_LOCK`).
fn sync_live_into_snapshot_if_active(
    user_id: &str,
    auth: &mut AuthFile,
    settings: &Settings,
) -> AppResult<bool> {
    let active_path = auth_json_path(settings)?;
    let Some(live_auth) = read_auth_file(&active_path)? else {
        return Ok(false);
    };
    let Ok((_, live_entry)) = primary_entry(&live_auth) else {
        return Ok(false);
    };
    let Ok(live_uid) = extract_user_id(live_entry) else {
        return Ok(false);
    };
    if live_uid != user_id {
        return Ok(false);
    }

    let Ok((_, snap_entry)) = primary_entry(auth) else {
        return Ok(false);
    };

    let live_rt = live_entry.refresh_token.as_deref().unwrap_or("");
    let snap_rt = snap_entry.refresh_token.as_deref().unwrap_or("");
    if live_entry.key == snap_entry.key && live_rt == snap_rt {
        return Ok(false);
    }

    // Live credentials differ (CLI may have rotated RT) — adopt live into snapshot.
    *auth = live_auth;
    save_account_snapshot(user_id, auth)?;
    eprintln!("[token_refresh] synced live auth → snapshot for {user_id}");
    Ok(true)
}

/// After OIDC failure: if live auth is this user and RT/AT changed vs snapshot, heal snapshot.
fn try_heal_from_live(user_id: &str, settings: &Settings) -> AppResult<Option<RefreshOneResult>> {
    let active_path = auth_json_path(settings)?;
    let Some(live_auth) = read_auth_file(&active_path)? else {
        return Ok(None);
    };
    let Ok((_, live_entry)) = primary_entry(&live_auth) else {
        return Ok(None);
    };
    let Ok(live_uid) = extract_user_id(live_entry) else {
        return Ok(None);
    };
    if live_uid != user_id {
        return Ok(None);
    }

    let snap = match load_account_snapshot(user_id) {
        Ok(s) => s,
        Err(_) => {
            // Snapshot missing (removed?) — nothing to heal into.
            return Ok(None);
        }
    };
    let Ok((_, snap_entry)) = primary_entry(&snap) else {
        return Ok(None);
    };

    let live_rt = live_entry.refresh_token.as_deref().unwrap_or("");
    let snap_rt = snap_entry.refresh_token.as_deref().unwrap_or("");
    if live_entry.key == snap_entry.key && live_rt == snap_rt {
        return Ok(None);
    }

    // Account may have been removed while OIDC was in flight.
    let meta = load_meta()?;
    if !meta.accounts.contains_key(user_id) {
        return Ok(None);
    }

    save_account_snapshot(user_id, &live_auth)?;
    eprintln!("[token_refresh] healed snapshot from live auth after OIDC failure for {user_id}");
    Ok(Some(RefreshOneResult {
        user_id: user_id.into(),
        ok: true,
        message: "Refresh skipped: healed from live auth".into(),
        expires_at: live_entry.expires_at.clone(),
    }))
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
    // Abort if account was removed while refresh was in flight (prevents resurrection).
    let meta = load_meta()?;
    if !meta.accounts.contains_key(user_id) {
        return Err(AppError::msg(format!(
            "Account {user_id} was removed; aborting token persist"
        )));
    }

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
/// - `!force`: only if `needs_refresh` after optional live→snapshot sync
fn refresh_account_with_policy(
    user_id: &str,
    force: bool,
    refresh_unknown_expiry: bool,
) -> AppResult<RefreshOneResult> {
    let _guard = REFRESH_LOCK
        .lock()
        .map_err(|_| AppError::msg("Token refresh lock poisoned"))?;

    let settings = settings::load_settings()?;
    let mut auth = load_account_snapshot(user_id)?;

    // Prefer live CLI tokens for the active account before deciding needs_refresh / OIDC.
    let live_synced = {
        let _auth_guard = AUTH_LOCK
            .lock()
            .map_err(|_| AppError::msg("Auth lock poisoned"))?;
        sync_live_into_snapshot_if_active(user_id, &mut auth, &settings)?
    };

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

    // Background scans skip unknown expiry to avoid churn. API paths opt in to one refresh
    // attempt, which gives opaque/legacy tokens a parseable expiry for subsequent calls.
    let refreshing_unknown_expiry =
        !force && !needs_refresh(entry) && access_expiry(entry).is_none();
    if !should_refresh_entry(entry, force, refresh_unknown_expiry) {
        return Ok(RefreshOneResult {
            user_id: user_id.into(),
            ok: true,
            message: if live_synced {
                "Synced from live auth (access token still valid)".into()
            } else {
                "Access token still valid".into()
            },
            expires_at: entry.expires_at.clone(),
        });
    }

    match refresh_entry(entry) {
        Ok(()) => {
            let exp = entry.expires_at.clone();
            let refreshed_entry = entry.clone();

            // Synchronize the live auth-file read/check/write with login and switch operations.
            let _auth_guard = AUTH_LOCK
                .lock()
                .map_err(|_| AppError::msg("Auth lock poisoned"))?;

            // Account removed during OIDC network call — do not resurrect snapshot.
            let meta = load_meta()?;
            if !meta.accounts.contains_key(user_id) {
                return Ok(RefreshOneResult {
                    user_id: user_id.into(),
                    ok: true,
                    message: "Refresh skipped: account was removed".into(),
                    expires_at: None,
                });
            }

            let mut latest_auth = load_account_snapshot(user_id)?;
            // Key may have changed if another path rewrote the snapshot.
            let latest_key = latest_auth
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| AppError::msg("Empty auth snapshot after refresh"))?;
            let latest_entry = latest_auth
                .get_mut(&latest_key)
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

            match persist_auth_file(user_id, &latest_auth, &settings) {
                Ok(()) => Ok(RefreshOneResult {
                    user_id: user_id.into(),
                    ok: true,
                    message: if force {
                        "Refreshed".into()
                    } else if refreshing_unknown_expiry {
                        "Refreshed (expiry was unknown)".into()
                    } else {
                        "Refreshed (near expiry)".into()
                    },
                    expires_at: exp,
                }),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("was removed") {
                        Ok(RefreshOneResult {
                            user_id: user_id.into(),
                            ok: true,
                            message: "Refresh skipped: account was removed".into(),
                            expires_at: None,
                        })
                    } else {
                        Ok(RefreshOneResult {
                            user_id: user_id.into(),
                            ok: false,
                            message: msg,
                            expires_at: exp,
                        })
                    }
                }
            }
        }
        Err(e) => {
            // OIDC invalid_grant / failure: try healing from live CLI tokens once.
            let heal = {
                let _auth_guard = AUTH_LOCK
                    .lock()
                    .map_err(|_| AppError::msg("Auth lock poisoned"))?;
                try_heal_from_live(user_id, &settings)?
            };
            if let Some(healed) = heal {
                return Ok(healed);
            }
            Ok(RefreshOneResult {
                user_id: user_id.into(),
                ok: false,
                message: e.to_string(),
                expires_at: entry.expires_at.clone(),
            })
        }
    }
}

pub fn refresh_account(user_id: &str, force: bool) -> AppResult<RefreshOneResult> {
    refresh_account_with_policy(user_id, force, false)
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
                if r.ok
                    && (r.message.contains("still valid")
                        || r.message.contains("skipped")
                        || r.message.contains("Synced from live"))
                {
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
/// If OIDC refresh fails but the access JWT is still usable, proceed (soft success).
pub fn ensure_fresh_token(user_id: &str) -> AppResult<()> {
    // Unlike the periodic scan, an API path should make one OIDC attempt when expiry is
    // unknown. A successful response records expires_at, so this does not churn every call.
    let r = refresh_account_with_policy(user_id, false, true)?;
    if r.ok {
        return Ok(());
    }

    // Structured: if access token still has life, do not hard-fail the API path.
    if let Ok(auth) = load_account_snapshot(user_id) {
        if let Ok((_, entry)) = primary_entry(&auth) {
            if access_token_still_usable(entry) {
                eprintln!(
                    "[token_refresh] ensure_fresh: refresh failed but access still usable for {user_id}: {}",
                    r.message
                );
                return Ok(());
            }
            // Expiry is unknown: refresh was attempted above. Let the API validate the
            // existing access token if OIDC was temporarily unavailable, and retry refresh
            // again on the next API call if necessary.
            if access_expiry(entry).is_none() {
                eprintln!(
                    "[token_refresh] ensure_fresh: refresh failed for unknown-expiry token; trying existing access for {user_id}: {}",
                    r.message
                );
                return Ok(());
            }
            // No refresh_token: allow try with existing access (may 401 later).
            if entry.refresh_token.as_ref().is_none_or(|t| t.is_empty()) {
                return Ok(());
            }
        }
    }

    // Fallback string matching (legacy paths).
    if r.message.contains("still valid") || r.message.contains("No refresh_token") {
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
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    fn make_jwt(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{header}.{payload}.sig")
    }

    fn bare_entry(key: &str, expires_at: Option<&str>) -> AuthEntry {
        AuthEntry {
            key: key.to_string(),
            auth_mode: None,
            create_time: None,
            user_id: None,
            email: None,
            first_name: None,
            last_name: None,
            principal_type: None,
            principal_id: None,
            team_id: None,
            refresh_token: Some("rt".into()),
            expires_at: expires_at.map(String::from),
            oidc_issuer: None,
            oidc_client_id: None,
            coding_data_retention_opt_out: None,
        }
    }

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

    #[test]
    fn access_expiry_uses_min_of_expires_at_and_jwt() {
        // JWT exp earlier than expires_at → min is JWT
        let jwt_exp = 1_700_000_000i64; // 2023-11-14
        let later = "2030-01-01T00:00:00+00:00";
        let entry = bare_entry(&make_jwt(jwt_exp), Some(later));
        let exp = access_expiry(&entry).expect("expiry");
        assert_eq!(exp.timestamp(), jwt_exp);

        // expires_at earlier than JWT → min is expires_at
        let early = "2020-01-01T00:00:00+00:00";
        let far_jwt = 2_000_000_000i64;
        let entry2 = bare_entry(&make_jwt(far_jwt), Some(early));
        let exp2 = access_expiry(&entry2).expect("expiry");
        assert_eq!(
            exp2.timestamp(),
            DateTime::parse_from_rfc3339(early).unwrap().timestamp()
        );
    }

    #[test]
    fn access_expiry_jwt_only_and_expires_at_only() {
        let jwt_exp = 1_800_000_000i64;
        let entry = bare_entry(&make_jwt(jwt_exp), None);
        assert_eq!(access_expiry(&entry).unwrap().timestamp(), jwt_exp);

        let only_at = "2025-06-01T12:00:00+00:00";
        let entry2 = bare_entry("not-a-jwt", Some(only_at));
        let exp2 = access_expiry(&entry2).unwrap();
        assert_eq!(
            exp2.timestamp(),
            DateTime::parse_from_rfc3339(only_at).unwrap().timestamp()
        );
    }

    #[test]
    fn needs_refresh_unknown_expiry_is_false() {
        let entry = bare_entry("not-a-jwt", None);
        assert!(!needs_refresh(&entry));
        assert!(!should_refresh_entry(&entry, false, false));
        assert!(should_refresh_entry(&entry, false, true));
    }

    #[test]
    fn needs_refresh_expired_is_true() {
        let past = Utc::now().timestamp() - 3600;
        let entry = bare_entry(&make_jwt(past), None);
        assert!(needs_refresh(&entry));
    }

    #[test]
    fn needs_refresh_far_future_is_false() {
        let future = Utc::now().timestamp() + 24 * 3600;
        let entry = bare_entry(&make_jwt(future), None);
        assert!(!needs_refresh(&entry));
    }

    #[test]
    fn apply_token_response_prefers_jwt_exp() {
        let exp = Utc::now().timestamp() + 1800;
        let mut entry = bare_entry("old", Some("2099-01-01T00:00:00+00:00"));
        let jwt = make_jwt(exp);
        let resp = TokenResponse {
            access_token: Some(jwt.clone()),
            refresh_token: Some("new-rt".into()),
            expires_in: Some(99999),
            error: None,
            error_description: None,
        };
        apply_token_response(&mut entry, &jwt, &resp);
        assert_eq!(entry.key, jwt);
        assert_eq!(entry.refresh_token.as_deref(), Some("new-rt"));
        let got = DateTime::parse_from_rfc3339(entry.expires_at.as_ref().unwrap())
            .unwrap()
            .timestamp();
        assert_eq!(got, exp);
    }

    #[test]
    fn apply_token_response_default_expires_in_is_3600() {
        let mut entry = bare_entry("old", None);
        let resp = TokenResponse {
            access_token: Some("opaque-token".into()),
            refresh_token: None,
            expires_in: None,
            error: None,
            error_description: None,
        };
        let before = Utc::now();
        apply_token_response(&mut entry, "opaque-token", &resp);
        let got = DateTime::parse_from_rfc3339(entry.expires_at.as_ref().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let delta = (got - before).num_seconds();
        assert!((3590..=3610).contains(&delta), "delta={delta}");
    }

    #[test]
    fn access_token_still_usable_respects_min_life() {
        let soon = Utc::now().timestamp() + 10; // < 30s
        let entry = bare_entry(&make_jwt(soon), None);
        assert!(!access_token_still_usable(&entry));

        let later = Utc::now().timestamp() + 120;
        let entry2 = bare_entry(&make_jwt(later), None);
        assert!(access_token_still_usable(&entry2));
    }
}
