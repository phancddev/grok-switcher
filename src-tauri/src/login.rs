use crate::auth::{backup_path, fingerprint, read_auth_file, write_auth_file_atomic};
use crate::error::{AppError, AppResult};
use crate::paths::{auth_json_path, grok_home, resolve_grok_binary};
use crate::settings::Settings;
use crate::store::{import_auth_as_account, upsert_meta_account};
use crate::types::{AccountMeta, AuthFile};
use serde::Serialize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Serialize mutations that touch auth.json / backups.
static AUTH_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStatusEvent {
    pub kind: String,
    pub value: String,
}

pub fn run_add_account_arc(
    settings: &Settings,
    label: Option<String>,
    on_status: std::sync::Arc<dyn Fn(LoginStatusEvent) + Send + Sync>,
) -> AppResult<(String, AccountMeta)> {
    let emit = |kind: &str, value: String| {
        on_status(LoginStatusEvent {
            kind: kind.into(),
            value,
        });
    };

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

    emit(
        "message",
        "Starting device login… copy the link, open it, then enter the code.".into(),
    );

    let mut child = Command::new(&grok)
        .arg("login")
        .arg("--device-auth")
        .env("GROK_HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::msg(format!("Failed to start `grok login --device-auth`: {e}")))?;

    if let Some(stdout) = child.stdout.take() {
        let cb = on_status.clone();
        thread::spawn(move || pipe_status(stdout, cb));
    }
    if let Some(stderr) = child.stderr.take() {
        let cb = on_status.clone();
        thread::spawn(move || pipe_status(stderr, cb));
    }

    // Always provide a default device page link to copy immediately
    emit("url", "https://auth.x.ai/device".into());

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
                    if status.success() && !fp.is_empty() {
                        break Ok(auth);
                    }
                }
                break Err(AppError::msg(format!(
                    "`grok login` exited with {status}. Open the link, enter the code, and finish sign-in."
                )));
            }
            Ok(None) => {}
            Err(e) => break Err(AppError::msg(format!("Failed waiting for grok login: {e}"))),
        }

        if started.elapsed() > LOGIN_TIMEOUT {
            let _ = child.kill();
            break Err(AppError::msg(
                "Login timed out after 10 minutes. Try again and complete device sign-in.",
            ));
        }

        thread::sleep(POLL_INTERVAL);
    };

    match result {
        Ok(auth) => {
            let _ = child.kill();
            let _ = child.wait();
            match finalize_import(&auth, label) {
                Ok(ok) => {
                    if let Some(ref bak) = backup {
                        let _ = fs::remove_file(bak);
                    }
                    emit("done", "Login complete".into());
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

fn pipe_status<R: std::io::Read + Send + 'static>(
    reader: R,
    on_status: std::sync::Arc<dyn Fn(LoginStatusEvent) + Send + Sync>,
) {
    let buf = BufReader::new(reader);
    for line in buf.lines().flatten() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Emit raw line as message for debugging
        for url in extract_urls(trimmed) {
            on_status(LoginStatusEvent {
                kind: "url".into(),
                value: url,
            });
        }
        if let Some(code) = extract_device_code(trimmed) {
            on_status(LoginStatusEvent {
                kind: "code".into(),
                value: code,
            });
        }
        on_status(LoginStatusEvent {
            kind: "message".into(),
            value: trimmed.to_string(),
        });
    }
}

fn extract_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 8 < bytes.len() {
        if text[i..].starts_with("https://") || text[i..].starts_with("http://") {
            let start = i;
            i += if text[i..].starts_with("https://") {
                8
            } else {
                7
            };
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.is_whitespace() || c == ')' || c == ']' || c == '"' || c == '\'' || c == '<' {
                    break;
                }
                i += 1;
            }
            let mut url = text[start..i].to_string();
            // strip trailing punctuation
            while url.ends_with('.') || url.ends_with(',') || url.ends_with(';') {
                url.pop();
            }
            if !out.contains(&url) {
                out.push(url);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn extract_device_code(text: &str) -> Option<String> {
    // Patterns: ABCD-1234, ABCD1234, code: XXXX-XXXX
    let upper = text.to_uppercase();
    // Find token matching [A-Z0-9]{4}-[A-Z0-9]{4}
    let chars: Vec<char> = upper.chars().collect();
    let n = chars.len();
    for i in 0..n {
        if i + 9 <= n
            && chars[i].is_ascii_alphanumeric()
            && chars[i + 1].is_ascii_alphanumeric()
            && chars[i + 2].is_ascii_alphanumeric()
            && chars[i + 3].is_ascii_alphanumeric()
            && chars[i + 4] == '-'
            && chars[i + 5].is_ascii_alphanumeric()
            && chars[i + 6].is_ascii_alphanumeric()
            && chars[i + 7].is_ascii_alphanumeric()
            && chars[i + 8].is_ascii_alphanumeric()
        {
            let before_ok = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
            let after_ok = i + 9 >= n || !chars[i + 9].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return Some(chars[i..i + 9].iter().collect());
            }
        }
    }
    None
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

fn finalize_import(auth: &AuthFile, label: Option<String>) -> AppResult<(String, AccountMeta)> {
    let (user_id, mut meta) = import_auth_as_account(auth)?;
    if let Some(l) = label {
        let t = l.trim();
        if !t.is_empty() {
            meta.label = Some(t.to_string());
        }
    }
    if let Ok((_, entry)) = crate::auth::primary_entry(auth) {
        // Prefer subscription-enriched user payload for plan name
        if let Ok(user) = crate::billing::fetch_subscription_for_token(&entry.key) {
            if let Some(email) = user.email.clone() {
                meta.email = email;
            }
            if user.first_name.is_some() {
                meta.first_name = user.first_name.clone();
            }
            if user.last_name.is_some() {
                meta.last_name = user.last_name.clone();
            }
            meta.subscription_tier = user.subscription_tiers.clone();
            meta.plan_expires_at = crate::billing::plan_expires_from_user(&user);
        } else if let Ok(user) = crate::billing::fetch_user_for_token(&entry.key) {
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
    if let Ok((_, entry)) = crate::auth::primary_entry(auth) {
        if let Ok(q) = crate::billing::fetch_quota_for_token(&entry.key) {
            meta.quota = Some(q.clone());
            let _ = crate::store::update_quota(&user_id, q, meta.tier);
        }
    }
    if let Ok(m) = crate::store::load_meta() {
        if let Some(acc) = m.accounts.get(&user_id) {
            meta = acc.clone();
        }
    }
    Ok((user_id, meta))
}

/// Import whatever is currently in auth.json without running login.
pub fn import_current(
    settings: &Settings,
    label: Option<String>,
) -> AppResult<(String, AccountMeta)> {
    let _guard = AUTH_LOCK
        .lock()
        .map_err(|_| AppError::msg("Auth lock poisoned"))?;
    let auth_path = auth_json_path(settings)?;
    let auth = read_auth_file(&auth_path)?.ok_or_else(|| {
        AppError::msg(
            "No active Grok session in auth.json. Run Add Account or grok login first.",
        )
    })?;
    finalize_import(&auth, label)
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

pub fn set_label(user_id: &str, label: Option<String>) -> AppResult<()> {
    use crate::store::{load_meta, save_meta};
    let mut meta = load_meta()?;
    let entry = meta
        .accounts
        .get_mut(user_id)
        .ok_or_else(|| AppError::msg(format!("Unknown account: {user_id}")))?;
    entry.label = label.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    save_meta(&meta)
}
