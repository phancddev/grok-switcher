use crate::auth::{backup_path, fingerprint, read_auth_file};
use crate::error::{AppError, AppResult};
use crate::paths::{auth_json_path, resolve_grok_binary};
use crate::settings::Settings;
use crate::store::{import_auth_as_account, upsert_meta_account};
use crate::types::{AccountMeta, AuthFile};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub fn run_add_account(settings: &Settings) -> AppResult<(String, AccountMeta)> {
    let grok = resolve_grok_binary(settings)?;
    let auth_path = auth_json_path(settings)?;

    // Backup current auth
    let backup = if auth_path.exists() {
        let bak = backup_path(&auth_path);
        fs::copy(&auth_path, &bak)?;
        Some(bak)
    } else {
        None
    };

    let before_fp = read_auth_file(&auth_path)?
        .as_ref()
        .map(fingerprint)
        .unwrap_or_default();

    // Best-effort logout so login produces a clean session
    let _ = Command::new(&grok)
        .arg("logout")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Start login — opens browser. Use null stdio for GUI app (no TTY).
    let mut child = Command::new(&grok)
        .arg("login")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::msg(format!("Failed to start `grok login`: {e}")))?;

    let started = Instant::now();
    let result = loop {
        // Success path: auth file changed with a valid entry
        if let Some(auth) = read_auth_file(&auth_path)? {
            let fp = fingerprint(&auth);
            if !fp.is_empty() && fp != before_fp {
                // Wait briefly for file to stabilize (token write + metadata)
                thread::sleep(Duration::from_millis(300));
                if let Some(stable) = read_auth_file(&auth_path)? {
                    if fingerprint(&stable) == fingerprint(&auth) || fingerprint(&stable) != before_fp
                    {
                        // Prefer latest
                        let final_auth = read_auth_file(&auth_path)?.unwrap_or(auth);
                        break Ok(final_auth);
                    }
                }
            }
        }

        // Process exited?
        match child.try_wait() {
            Ok(Some(status)) => {
                // Give a short grace period for final write
                thread::sleep(Duration::from_millis(800));
                if let Some(auth) = read_auth_file(&auth_path)? {
                    let fp = fingerprint(&auth);
                    if !fp.is_empty() && (before_fp.is_empty() || fp != before_fp) {
                        break Ok(auth);
                    }
                }
                // If login exited 0 but same fingerprint, still try to import current
                if status.success() {
                    if let Some(auth) = read_auth_file(&auth_path)? {
                        break Ok(auth);
                    }
                }
                break Err(AppError::msg(format!(
                    "`grok login` exited with {status}. Complete browser sign-in, or check that Grok is installed."
                )));
            }
            Ok(None) => {}
            Err(e) => break Err(AppError::msg(format!("Failed waiting for grok login: {e}"))),
        }

        if started.elapsed() > LOGIN_TIMEOUT {
            let _ = child.kill();
            break Err(AppError::msg(
                "Login timed out after 10 minutes. Try again and complete browser sign-in.",
            ));
        }

        thread::sleep(POLL_INTERVAL);
    };

    match result {
        Ok(auth) => {
            let _ = child.kill(); // ensure process gone if still running
            let _ = child.wait();
            // Clean backup on success (new account is active)
            if let Some(ref bak) = backup {
                let _ = fs::remove_file(bak);
            }
            finalize_import(&auth)
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            // Restore backup on failure
            if let Some(ref bak) = backup {
                if bak.exists() {
                    let _ = fs::copy(bak, &auth_path);
                    let _ = fs::remove_file(bak);
                }
            }
            Err(e)
        }
    }
}

fn finalize_import(auth: &AuthFile) -> AppResult<(String, AccountMeta)> {
    let (user_id, mut meta) = import_auth_as_account(auth)?;
    // Enrich from billing/user if possible
    if let Ok((_, entry)) = crate::auth::primary_entry(auth) {
        if let Ok(quota) = crate::billing::fetch_quota_for_token(&entry.key) {
            meta.quota = Some(quota);
        }
        if let Ok(user) = crate::billing::fetch_user_for_token(&entry.key) {
            if let Some(email) = user.email {
                meta.email = email;
            }
            if user.first_name.is_some() {
                meta.first_name = user.first_name;
            }
            if user.last_name.is_some() {
                meta.last_name = user.last_name;
            }
        }
        meta.tier = crate::auth::jwt_tier(&entry.key);
    }
    upsert_meta_account(&user_id, meta.clone())?;
    Ok((user_id, meta))
}

/// Import whatever is currently in auth.json without running login.
pub fn import_current(settings: &Settings) -> AppResult<(String, AccountMeta)> {
    let auth_path = auth_json_path(settings)?;
    let auth = read_auth_file(&auth_path)?
        .ok_or_else(|| AppError::msg("No active Grok session in auth.json. Run Add Account or grok login first."))?;
    finalize_import(&auth)
}

pub fn switch_to(settings: &Settings, user_id: &str) -> AppResult<()> {
    use crate::store::{load_account_snapshot, set_active};
    let auth = load_account_snapshot(user_id)?;
    let path = auth_json_path(settings)?;
    crate::auth::write_auth_file_atomic(&path, &auth)?;
    set_active(user_id)?;
    // Touch mtime for hot-reload consumers
    let _ = path;
    Ok(())
}

#[allow(dead_code)]
pub fn restore_backup_if_any(auth_path: &Path) {
    let bak = backup_path(auth_path);
    if bak.exists() {
        let _ = fs::copy(&bak, auth_path);
        let _ = fs::remove_file(bak);
    }
}
