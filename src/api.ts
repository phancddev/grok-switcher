import { invoke as tauriInvoke, isTauri } from "@tauri-apps/api/core";
import type { AccountSummary, QuotaInfo, Settings } from "./types";

/**
 * Safe wrapper around Tauri invoke.
 * When the Rust backend failed to compile (disk full, etc.) or the UI is
 * opened in a plain browser, `window.__TAURI_INTERNALS__` is missing and
 * raw `invoke` throws: Cannot read properties of undefined (reading 'invoke').
 */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: unknown } })
    .__TAURI_INTERNALS__;

  if (!isTauri() || !internals?.invoke) {
    throw new Error(
      "Tauri backend is not available. Run with `npm run tauri dev` (not `npm run dev` alone). " +
        "If you already did, the Rust side may have failed to build — check the terminal for " +
        "`No space left on device` or other compile errors, free disk space, then restart.",
    );
  }

  return tauriInvoke<T>(cmd, args);
}

export const listAccounts = () => invoke<AccountSummary[]>("list_accounts");

export const getActive = () => invoke<AccountSummary | null>("get_active");

export const addAccount = () => invoke<AccountSummary>("add_account");

export const importCurrentAccount = () =>
  invoke<AccountSummary>("import_current_account");

/** Tauri 2 default rename_all for command args is camelCase */
export const switchAccount = (userId: string) =>
  invoke<AccountSummary>("switch_account", { userId });

export const removeAccount = (userId: string) =>
  invoke<void>("remove_account", { userId });

export const refreshQuota = (userId?: string) =>
  invoke<QuotaInfo>("refresh_quota", { userId: userId ?? null });

export const refreshAllQuotas = () =>
  invoke<Record<string, QuotaInfo | { error: string }>>("refresh_all_quotas");

export const getSettings = () => invoke<Settings>("get_settings");

export const saveSettings = (newSettings: Settings) =>
  invoke<Settings>("save_settings", { newSettings });

export const resolveGrokBinary = () =>
  invoke<string | null>("resolve_grok_binary");
