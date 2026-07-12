use crate::auth::{backup_path, fingerprint, read_auth_file, write_auth_file_atomic};
use crate::error::{AppError, AppResult};
use crate::paths::{auth_json_path, grok_home, resolve_grok_binary};
use crate::settings::Settings;
use crate::store::{import_auth_as_account, upsert_meta_account};
use crate::types::{AccountMeta, AuthFile};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Serialize mutations that touch auth.json / backups.
static AUTH_LOCK: Mutex<()> = Mutex::new(());

pub fn run_add_account(settings: &Settings) -> AppResult<(String, AccountMeta)> {
    let _guard = AUTH_LOCK
        .lock()
        .map_err(|_| AppError::msg("Auth lock poisoned"))?;

    let grok = resolve_grok_binary(settings)?;
    let home = grok_home(settings)?;
    let auth_path = auth_json_path(settings)?;

    let backup = if auth_path.exists() {
        let bak = backup_path(&auth_path);
        fs::copy(&auth_path, &bak)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&bak, fs::Permissions::from_mode(0o600));
        }
        Some(bak)
    } else {
        None
    };

    let before_fp = read_auth_file(&auth_path)?
        .as_ref()
        .map(fingerprint)
        .unwrap_or_default();

    let mut logout = Command::new(&grok);
    logout
        .arg("logout")
        .env("GROK_HOME", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = logout.status();

    let mut child = Command::new(&grok)
        .arg("login")
        .env("GROK_HOME", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::msg(format!("Failed to start `grok login`: {e}")))?;

    let started = Instant::now();
    let result = loop {
        if let Some(auth) = wait_stable_auth(&auth_path)? {
            let fp = fingerprint(&auth);
            if !fp.is_empty() && fp != before_fp {
                break Ok(auth);
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                thread::sleep(Duration::from_millis(800));
                if let Some(auth) = wait_stable_auth(&auth_path)? {
                    let fp = fingerprint(&auth);
                    if !fp.is_empty() && (before_fp.is_empty() || fp != before_fp) {
                        break Ok(auth);
                    }
                    // Login of same account may refresh token only
                    if status.success() && !fp.is_empty() {
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
            let _ = child.kill();
            let _ = child.wait();
            match finalize_import(&auth) {
                Ok(ok) => {
                    if let Some(ref bak) = backup {
                        let _ = fs::remove_file(bak);
                    }
                    Ok(ok)
                }
                Err(e) => {
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
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
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

/// Require two identical fingerprints ~150ms apart so we don't import a half-written file.
fn wait_stable_auth(auth_path: &std::path::Path) -> AppResult<Option<AuthFile>> {
    let first = match read_auth_file(auth_path)? {
        Some(a) => a,
        None => return Ok(None),
    };
    let fp1 = fingerprint(&first);
    if fp1.is_empty() {
        return Ok(None);
    }
    thread::sleep(Duration::from_millis(150));
    let second = match read_auth_file(auth_path)? {
        Some(a) => a,
        None => return Ok(None),
    };
    if fingerprint(&second) == fp1 {
        Ok(Some(second))
    } else {
        Ok(None)
    }
}

fn finalize_import(auth: &AuthFile) -> AppResult<(String, AccountMeta)> {
    let (user_id, mut meta) = import_auth_as_account(auth)?;
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
    let _guard = AUTH_LOCK
        .lock()
        .map_err(|_| AppError::msg("Auth lock poisoned"))?;
    let auth_path = auth_json_path(settings)?;
    let auth = read_auth_file(&auth_path)?.ok_or_else(|| {
        AppError::msg(
            "No active Grok session in auth.json. Run Add Account or grok login first.",
        )
    })?;
    finalize_import(&auth)
}

pub fn switch_to(settings: &Settings, user_id: &str) -> AppResult<()> {
    let _guard = AUTH_LOCK
        .lock()
        .map_err(|_| AppError::msg("Auth lock poisoned"))?;
    use crate::store::{load_account_snapshot, set_active};
    let auth = load_account_snapshot(user_id)?;
    let path = auth_json_path(settings)?;
    write_auth_file_atomic(&path, &auth)?;
    set_active(user_id)?;
    Ok(())
}
