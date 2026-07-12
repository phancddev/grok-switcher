# Grok Switcher — Code Review

Review date: 2026-07-12  
Scope: security of auth tokens, login backup/restore races, Tauri IPC, release workflow, cross-platform paths, and add/switch/quota correctness.  
Sources: `SPEC.md`, `src-tauri/src/**`, `src/**`, `.github/workflows/release.yml`, `tauri.conf.json`.

---

### Issue 1 -- Severity: bug
- **File**: src/api.ts:13-28
- **Description**: Tauri 2 renames command argument keys from Rust `snake_case` to JavaScript **camelCase** by default (`#[tauri::command]` without `rename_all = "snake_case"`). The frontend invokes with snake_case keys:

  ```ts
  invoke("switch_account", { user_id: userId });
  invoke("remove_account", { user_id: userId });
  invoke("refresh_quota", { user_id: userId ?? null });
  invoke("save_settings", { new_settings: newSettings });
  ```

  Official Tauri 2 docs require camelCase (`userId`, `newSettings`). Effects:
  - **Switch / Remove / Save settings**: required `String` / `Settings` args fail deserialization → commands error; switch/remove effectively broken.
  - **Refresh quota (per account)**: `user_id: Option<String>` silently deserializes as `None` when only `user_id` is sent, so the backend falls back to the *active* account. Clicking **Quota** on a non-active row updates the wrong account’s cached quota.
  - `add_account` / `import_current_account` / `list_accounts` / `refresh_all_quotas` have no (or only optional-missing) args and still work, which can hide the bug during smoke testing.

  The comment at the bottom of `api.ts` claims camelCase is passed; the code does the opposite.
- **Suggestion**: Change invoke payloads to `{ userId }`, `{ newSettings: newSettings }`, or mark every command with `#[tauri::command(rename_all = "snake_case")]` and keep snake_case keys. Align the comment with the chosen convention. Add a quick integration test or manual checklist for switch/remove/per-account quota after the fix.
- **Status**: open

---

### Issue 2 -- Severity: bug
- **File**: src-tauri/src/login.rs:103-111
- **Description**: On the success path, the previous-session backup is deleted **before** `finalize_import` runs:

  ```rust
  if let Some(ref bak) = backup {
      let _ = fs::remove_file(bak);
  }
  finalize_import(&auth)
  ```

  If `finalize_import` fails (cannot extract `user_id`, disk full while writing snapshot/meta, JSON issues, etc.), the new `auth.json` may remain but the prior account backup is already gone. The user loses the previous session and has no automatic restore.
- **Suggestion**: Only remove the backup after `finalize_import` succeeds. On finalize failure, either restore the backup or leave the `.bak-switcher` file and surface a clear recovery message.
- **Status**: open

---

### Issue 3 -- Severity: bug
- **File**: src-tauri/src/login.rs:35-47
- **Description**: `run_add_account` resolves `auth_json_path` from settings (`grokHome` / `GROK_HOME` / `~/.grok`), but spawns `grok logout` / `grok login` without setting `GROK_HOME` (or equivalent) in the child environment. If the user sets a custom Grok home in Settings, the CLI may still write to the default `~/.grok/auth.json` while the app polls the overridden path. Login then times out or never sees the new session; failure restore may write the backup to the override path while the CLI mutated the default path.
- **Suggestion**: When spawning `grok`, set `.env("GROK_HOME", grok_home(settings))` (and ensure the binary path resolution stays consistent). Document that custom home requires this env for the CLI.
- **Status**: open

---

### Issue 4 -- Severity: bug
- **File**: src-tauri/src/login.rs:16-125
- **Description**: Login backup/restore has several race and lifecycle issues:
  1. **No global lock** — concurrent `add_account` (or add + switch) share one fixed backup path (`auth.json.bak-switcher`) and the same `auth.json`. Second login can overwrite the first backup; restore can put the wrong session back.
  2. **Orphan backup** — if the app is killed mid-login, `.bak-switcher` is left behind and `restore_backup_if_any` is never called on startup (dead code at login.rs:172-178).
  3. **Weak change detection** — `fingerprint` uses `user_id|email|create_time` only (not token/`key`). Re-login of the same account with an unchanged create_time may not look like a change; combined with the `status.success()` branch that imports current auth even when fingerprint is unchanged, a no-op login can “succeed” without a new session.
  4. **Stability check is too loose** (login.rs:58-59): `fingerprint(&stable) == fingerprint(&auth) || fingerprint(&stable) != before_fp` accepts any post-change read, including mid-write contents.
- **Suggestion**: Serialize account mutations with a process-wide mutex (login/switch/remove/meta). On startup, if `.bak-switcher` exists, offer restore or auto-restore + notify. Include a hash of `key` (or full file hash) in the fingerprint. Require stable mtime/size or identical fingerprints across 2–3 polls before import.
- **Status**: open

---

### Issue 5 -- Severity: bug
- **File**: src-tauri/src/store.rs:90-97
- **Description**: Active account UI prefers `meta.active_user_id` over the live `auth.json` contents:

  ```rust
  let active = meta
      .active_user_id
      .clone()
      .or(active_from_file.clone());
  ```

  If the user runs `grok login` / switches outside the app, or a failed operation leaves meta and file out of sync, the UI can mark the wrong account **Active** while Grok CLI uses a different token. Switch/quota mental model breaks (“I switched but CLI still uses X”).
- **Suggestion**: Treat `auth.json` as source of truth for `is_active` when a matching managed account exists; use meta only as fallback when the file is missing/unreadable. Optionally reconcile `meta.active_user_id` on `list_accounts`.
- **Status**: open

---

### Issue 6 -- Severity: bug
- **File**: src-tauri/src/paths.rs:52-59
- **Description**: Settings paths for `grokBinaryPath` / `grokHome` are passed through `PathBuf::from` with no expansion of `~` (or `%USERPROFILE%`). The Settings UI placeholders show `~/.grok/bin/grok`, so users will commonly save a path that `is_file()` rejects → “Configured grok binary not found” and **Add account** fails.
- **Suggestion**: Expand a leading `~/` via `home_dir()`, and on Windows accept `%USERPROFILE%` / `%USERPROFILE%\...`. Validate and show the resolved path in Settings.
- **Status**: open

---

### Issue 7 -- Severity: suggestion
- **File**: src-tauri/src/paths.rs:46-48, src-tauri/src/auth.rs:23-42, src-tauri/src/store.rs:24-34
- **Description**: Token storage security is mostly reasonable for a local desktop tool, with gaps vs SPEC “mode 0600”:
  - Snapshots use atomic write + `0o600` on Unix (`write_auth_file_atomic`).
  - `meta.json` / `settings.json` also set `0o600` after write (meta may contain cached account metadata; settings less sensitive).
  - **`ensure_app_dirs` does not set directory mode `0o700`** on `~/.grok-switcher` / `accounts/`. With a permissive umask, directories may be `755`, allowing other local users to list account IDs even if files are `600`.
  - **Windows**: no explicit ACL tightening; reliance on default profile ACLs is typical but weaker than Unix `0600`.
  - Login backup is plain `fs::copy` to `auth.json.bak-switcher` beside the live auth file (same directory as Grok credentials); permissions follow the source file and are not re-asserted as `0600`.
  - Tokens remain plaintext JSON by design (acceptable for this app class; worth stating in README).
- **Suggestion**: After `create_dir_all`, `chmod 0o700` app dirs on Unix. Re-apply `0o600` on backup files. Optionally document Windows profile ACL expectations. Consider storing backups under `~/.grok-switcher/` rather than next to `auth.json`.
- **Status**: open

---

### Issue 8 -- Severity: suggestion
- **File**: src-tauri/src/commands.rs:88-120, src-tauri/src/billing.rs:43-48
- **Description**: `add_account`, `import_current_account`, and `refresh_all_quotas` correctly use `spawn_blocking`. `refresh_quota` is a **sync** command that performs blocking HTTP (up to 20s timeout) via `reqwest::blocking`. Tauri docs prefer async/blocking-pool patterns for heavy work to avoid UI jank. `switch_account` / `list_accounts` are short disk I/O (lower risk). Concurrent meta RMW (`load_meta` → mutate → `save_meta`) has no lock, so overlapping quota refresh + switch can lose updates.
- **Suggestion**: Make `refresh_quota` (and optionally other I/O commands) `async` + `spawn_blocking`, or use an async reqwest client. Share one mutex for meta/auth mutations with login/switch.
- **Status**: open

---

### Issue 9 -- Severity: suggestion
- **File**: src-tauri/src/store.rs:24-34
- **Description**: `save_meta` writes `meta.json` with a single `fs::write`. A crash mid-write can corrupt the account index (all labels, active id, cached quotas). Account snapshots already use temp+rename.
- **Suggestion**: Reuse the same atomic write helper as auth snapshots for `meta.json` and `settings.json`.
- **Status**: open

---

### Issue 10 -- Severity: suggestion
- **File**: src-tauri/src/auth.rs:45-49, src-tauri/src/types.rs:55-87
- **Description**:
  - `primary_entry` uses `HashMap::iter().next()`, which is unordered. If `auth.json` ever contains multiple entries, import/switch/fingerprint may pick an arbitrary one.
  - `AuthEntry` fields are snake_case with no `#[serde(alias = "...")]` for camelCase. If the Grok CLI ever emits camelCase keys (or a mix), `user_id` / `email` / tokens may fail to parse or fall back poorly (`unknown@local`, JWT `sub` only).
- **Suggestion**: Prefer a known map key if the CLI uses a stable key; otherwise choose entry by non-empty `key` + `user_id`/`email`. Add serde aliases for common camelCase variants and/or `#[serde(deny_unknown_fields)]` in tests with a fixture captured from a real `auth.json`.
- **Status**: open

---

### Issue 11 -- Severity: suggestion
- **File**: src-tauri/src/store.rs:71-88, src-tauri/src/login.rs:128-149
- **Description**: Re-import / re-add of an existing `user_id` overwrites meta via `upsert_meta_account` with a fresh `AccountMeta` that always has `label: None` and a new `created_at` fallback. User-defined labels are lost; created timestamp resets when missing from the entry.
- **Suggestion**: When upserting, merge with existing meta (preserve `label`, `created_at`).
- **Status**: open

---

### Issue 12 -- Severity: suggestion
- **File**: .github/workflows/release.yml:17-19, 108-116, 155-184
- **Description**: Release workflow is largely correct (matrix platforms, version sync into package/tauri/Cargo, draft releases, NSIS/DMG/AppImage/deb aligned with SPEC). Remaining issues:
  1. **`targets: ${{ matrix.rust_target }}`** for Linux/Windows is an empty string. Depending on `dtolnay/rust-toolchain` behavior, an empty `targets` input may be harmless or may warn/misconfigure; safer to omit the input when unset (conditional step or separate matrix fields).
  2. **`concurrency: cancel-in-progress: true`** on a multi-OS matrix can cancel after some artifacts uploaded, leaving a half-populated draft release for that tag/ref.
  3. **Every push to `main`** creates a new draft prerelease (`v${PKG_VERSION}-main.${SHORT}`). This is intentional for CI artifacts but can clutter Releases; tags for real versions are separate and OK.
  4. Version job uses `node -p` without `actions/setup-node` (runner image Node is usually fine).
  5. Dual macOS jobs on `macos-latest` with explicit `--target` for arm64/x64 is the right pattern for universal coverage without a universal binary.
- **Suggestion**: Gate `targets` on non-empty values; consider `cancel-in-progress: false` for release tags; optionally only build main drafts nightly or on workflow_dispatch to reduce draft spam.
- **Status**: open

---

### Issue 13 -- Severity: nit
- **File**: src/api.ts:33-35
- **Description**: Comment contradicts both Tauri’s default (camelCase JS keys) and the actual snake_case payloads. Also claims serde `rename_all` applies to command args; command arg renaming is controlled by `#[tauri::command(rename_all = ...)]`, not struct serde.
- **Suggestion**: Rewrite the comment after fixing Issue 1.
- **Status**: open

---

### Issue 14 -- Severity: nit
- **File**: src-tauri/src/commands.rs:88-89
- **Description**: Comment says “Accept both snake_case and camelCase via dual registration below if needed” but nothing implements dual registration.
- **Suggestion**: Remove the comment or implement explicit dual-arg support (not usually needed if Issue 1 is fixed properly).
- **Status**: open

---

### Issue 15 -- Severity: nit
- **File**: src-tauri/src/login.rs:41-46 vs SPEC.md:65
- **Description**: SPEC says login should inherit stdio so the browser opens; implementation uses `Stdio::null()` for GUI. Browser launch usually does not need inherited stdio, and null is appropriate for a windowed app, but CLI errors are discarded, making failures harder to diagnose.
- **Suggestion**: Capture stderr to a temp log or string and include the tail in timeout/exit error messages. Update SPEC to match GUI behavior.
- **Status**: open

---

### Issue 16 -- Severity: nit
- **File**: src-tauri/src/store.rs:133-144, src-tauri/src/commands.rs:98-108
- **Description**: Removing the active account clears `meta.active_user_id` but leaves `~/.grok/auth.json` as that account. Footer/CLI still show a live session that is no longer “managed.” Not necessarily wrong, but easy to confuse with “logged out.”
- **Suggestion**: Document behavior; optionally prompt to leave credentials, clear auth, or switch to another managed account on remove.
- **Status**: open

---

## Summary

Not clean — several issues would break or mis-handle core flows:

| Area | Verdict |
|------|---------|
| **IPC (switch/remove/settings/per-account quota)** | **Critical** — snake_case invoke keys vs Tauri 2 camelCase default |
| **Login backup/restore** | Backup deleted before finalize; no lock; orphan backup not restored |
| **Custom `GROK_HOME`** | CLI not given env → add-account can miss auth file |
| **Token storage** | Unix file `0600` OK for snapshots; dirs not `0700`; Windows default ACLs only |
| **Active account** | Meta preferred over live `auth.json` → can show wrong Active |
| **Paths** | No `~` expansion in settings; otherwise `PathBuf` / Windows `grok.exe` handling is fine |
| **release.yml** | Structurally sound; empty rust target input + cancel-in-progress are polish risks |
| **Quota** | Logic fine when user id is received; per-row refresh wrong until IPC fixed; blocking HTTP on sync command |

Highest priority fixes: **Issue 1 (IPC keys)**, **Issue 2 (backup lifecycle)**, **Issue 3 (GROK_HOME on spawn)**. No application code was modified in this review; findings only in this file.
