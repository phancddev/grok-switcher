import { invoke } from "@tauri-apps/api/core";
import type { AccountSummary, QuotaInfo, Settings } from "./types";

export const listAccounts = () => invoke<AccountSummary[]>("list_accounts");

export const getActive = () => invoke<AccountSummary | null>("get_active");

export const addAccount = () => invoke<AccountSummary>("add_account");

export const importCurrentAccount = () =>
  invoke<AccountSummary>("import_current_account");

export const switchAccount = (userId: string) =>
  invoke<AccountSummary>("switch_account", { user_id: userId });

export const removeAccount = (userId: string) =>
  invoke<void>("remove_account", { user_id: userId });

export const refreshQuota = (userId?: string) =>
  invoke<QuotaInfo>("refresh_quota", { user_id: userId ?? null });

export const refreshAllQuotas = () =>
  invoke<Record<string, QuotaInfo | { error: string }>>("refresh_all_quotas");

export const getSettings = () => invoke<Settings>("get_settings");

export const saveSettings = (newSettings: Settings) =>
  invoke<Settings>("save_settings", { new_settings: newSettings });

export const resolveGrokBinary = () =>
  invoke<string | null>("resolve_grok_binary");

// Note: Tauri 2 converts camelCase JS keys ↔ snake_case Rust args via serde rename
// when using #[serde(rename_all = "camelCase")] is not automatic for command args.
// We pass camelCase and rename parameters in Rust commands to camelCase aliases.