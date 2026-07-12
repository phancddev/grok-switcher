# Grok Switcher — Spec

Desktop app (Tauri 2) to manage multiple Grok Build accounts on macOS, Windows, Linux.

## Storage

- App data: `~/.grok-switcher/` (or `%USERPROFILE%\.grok-switcher` on Windows)
  - `accounts/<user_id>.json` — full snapshot of `auth.json` (mode 0600)
  - `meta.json` — `{ accounts: { [userId]: { email, firstName, lastName, label, lastUsed, createdAt, cachedQuota? } }, activeUserId?: string }`
- Active Grok auth: `~/.grok/auth.json` (or `$GROK_HOME/auth.json`)

## Tauri commands

| Command | Args | Returns |
|---------|------|---------|
| `list_accounts` | — | `AccountSummary[]` |
| `get_active` | — | `AccountSummary \| null` |
| `add_account` | — | `AccountSummary` (spawns `grok login`, watches auth.json) |
| `switch_account` | `userId: string` | `AccountSummary` |
| `remove_account` | `userId: string` | `void` |
| `refresh_quota` | `userId?: string` | `QuotaInfo` (omit = active) |
| `refresh_all_quotas` | — | `Record<userId, QuotaInfo \| { error: string }>` |
| `get_settings` | — | `Settings` |
| `save_settings` | `Settings` | `Settings` |
| `resolve_grok_binary` | — | `string \| null` |

## Types

```ts
type AccountSummary = {
  userId: string;
  email: string;
  firstName?: string;
  lastName?: string;
  label?: string;
  isActive: boolean;
  lastUsed?: string;
  createdAt?: string;
  quota?: QuotaInfo | null;
  tier?: number | null;
};

type QuotaInfo = {
  used: number;
  monthlyLimit: number;
  onDemandCap: number;
  billingPeriodStart: string;
  billingPeriodEnd: string;
  percentUsed: number;
  fetchedAt: string;
};

type Settings = {
  grokBinaryPath?: string | null;
  grokHome?: string | null; // override GROK_HOME
};
```

## Login flow (add_account)

1. Ensure store dirs exist
2. Backup current `auth.json` if present
3. Resolve `grok` binary (settings → GROK_HOME/bin → ~/.grok/bin → PATH)
4. Run `grok logout` (best effort)
5. Run `grok login` (inherit stdio so browser opens; wait up to 10 min)
6. Watch/poll `auth.json` until email/user_id present and file stable
7. Parse entry → save snapshot + meta
8. Leave new account as active (do not restore backup)
9. Fetch quota for new account

## Switch flow

1. Load snapshot for userId
2. Atomic write to `auth.json` (temp + rename)
3. Update meta.activeUserId + lastUsed
4. Return summary

## Billing

```
GET https://cli-chat-proxy.grok.com/v1/billing
GET https://cli-chat-proxy.grok.com/v1/user
Authorization: Bearer <token>
X-XAI-Token-Auth: xai-grok-cli
```

## Bundle (portable-friendly)

- macOS: `app` + `dmg` (drag to Applications)
- Windows: `nsis` (simple installer) + `msi` optional
- Linux: `appimage` + `deb`
