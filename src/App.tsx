import { useCallback, useEffect, useState } from "react";
import * as api from "./api";
import type { AccountSummary, Settings } from "./types";
import "./App.css";

function displayName(a: AccountSummary): string {
  const parts = [a.firstName, a.lastName].filter(Boolean).join(" ").trim();
  return parts || a.email;
}

function formatQuota(a: AccountSummary): string {
  const q = a.quota;
  if (!q) return "Quota: —";
  const used = Math.round(q.used).toLocaleString();
  const limit = Math.round(q.monthlyLimit).toLocaleString();
  return `${used} / ${limit} credits (${q.percentUsed.toFixed(1)}%)`;
}

function periodLabel(a: AccountSummary): string {
  const q = a.quota;
  if (!q?.billingPeriodEnd) return "";
  try {
    const end = new Date(q.billingPeriodEnd);
    return `Resets ${end.toLocaleDateString()}`;
  } catch {
    return `Period ends ${q.billingPeriodEnd}`;
  }
}

export default function App() {
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [info, setInfo] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [settings, setSettings] = useState<Settings>({});
  const [grokPath, setGrokPath] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const list = await api.listAccounts();
      setAccounts(list);
      const path = await api.resolveGrokBinary();
      setGrokPath(path);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const withBusy = async (key: string, fn: () => Promise<void>) => {
    setBusy(key);
    setError(null);
    setInfo(null);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const onAdd = () =>
    withBusy("add", async () => {
      setInfo("Complete sign-in in your browser…");
      const acc = await api.addAccount();
      setInfo(`Added ${acc.email}`);
    });

  const onImport = () =>
    withBusy("import", async () => {
      const acc = await api.importCurrentAccount();
      setInfo(`Imported ${acc.email}`);
    });

  const onSwitch = (userId: string) =>
    withBusy(`switch-${userId}`, async () => {
      const acc = await api.switchAccount(userId);
      setInfo(`Switched to ${acc.email}`);
    });

  const onRemove = (userId: string, email: string) => {
    if (!confirm(`Remove account ${email} from Grok Switcher?\n(Does not delete the Grok account.)`)) {
      return;
    }
    void withBusy(`remove-${userId}`, async () => {
      await api.removeAccount(userId);
      setInfo(`Removed ${email}`);
    });
  };

  const onRefreshQuota = (userId?: string) =>
    withBusy(userId ? `quota-${userId}` : "quota-all", async () => {
      if (userId) {
        await api.refreshQuota(userId);
        setInfo("Quota updated");
      } else {
        await api.refreshAllQuotas();
        setInfo("All quotas refreshed");
      }
    });

  const openSettings = async () => {
    setShowSettings(true);
    try {
      const s = await api.getSettings();
      setSettings(s);
    } catch (e) {
      setError(String(e));
    }
  };

  const saveSettings = () =>
    withBusy("settings", async () => {
      await api.saveSettings(settings);
      setShowSettings(false);
      setInfo("Settings saved");
    });

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <img className="brand-logo" src="/logo.png" alt="Grok Switcher" width={40} height={40} />
          <div>
            <h1>Grok Switcher</h1>
            <p className="subtitle">Manage Grok Build accounts · switch · check quota</p>
          </div>
        </div>
        <div className="header-actions">
          <button type="button" className="btn ghost" onClick={() => void openSettings()} disabled={!!busy}>
            Settings
          </button>
          <button
            type="button"
            className="btn ghost"
            onClick={() => void onRefreshQuota()}
            disabled={!!busy || accounts.length === 0}
          >
            Refresh quotas
          </button>
          <button type="button" className="btn secondary" onClick={() => void onImport()} disabled={!!busy}>
            Import current
          </button>
          <button type="button" className="btn primary" onClick={() => void onAdd()} disabled={!!busy}>
            {busy === "add" ? "Waiting for login…" : "Add account"}
          </button>
        </div>
      </header>

      {(error || info) && (
        <div className={`banner ${error ? "error" : "info"}`}>
          <span>{error ?? info}</span>
          <button type="button" className="banner-close" onClick={() => { setError(null); setInfo(null); }}>
            ×
          </button>
        </div>
      )}

      {busy === "add" && (
        <div className="banner info">
          Browser login started. Sign in with the new account, then return here.
        </div>
      )}

      <main className="main">
        {loading ? (
          <div className="empty">Loading…</div>
        ) : accounts.length === 0 ? (
          <div className="empty">
            <h2>No accounts yet</h2>
            <p>
              Click <strong>Add account</strong> to run <code>grok login</code>, or{" "}
              <strong>Import current</strong> to capture the session already in{" "}
              <code>~/.grok/auth.json</code>.
            </p>
            {!grokPath && (
              <p className="warn">
                Grok CLI not found on PATH. Open Settings and set the binary path
                (e.g. <code>~/.grok/bin/grok</code>).
              </p>
            )}
          </div>
        ) : (
          <ul className="account-list">
            {accounts.map((a) => (
              <li key={a.userId} className={`card ${a.isActive ? "active" : ""}`}>
                <div className="card-main">
                  <div className="card-title">
                    <span className="name">{displayName(a)}</span>
                    {a.isActive && <span className="badge">Active</span>}
                    {a.tier != null && <span className="badge muted">tier {a.tier}</span>}
                  </div>
                  <div className="email">{a.email}</div>
                  <div className="quota-row">
                    <div className="quota-bar">
                      <div
                        className="quota-fill"
                        style={{
                          width: `${Math.min(100, a.quota?.percentUsed ?? 0)}%`,
                        }}
                      />
                    </div>
                    <div className="quota-meta">
                      <span>{formatQuota(a)}</span>
                      <span className="muted">{periodLabel(a)}</span>
                    </div>
                  </div>
                </div>
                <div className="card-actions">
                  {!a.isActive && (
                    <button
                      type="button"
                      className="btn primary small"
                      disabled={!!busy}
                      onClick={() => void onSwitch(a.userId)}
                    >
                      {busy === `switch-${a.userId}` ? "…" : "Switch"}
                    </button>
                  )}
                  <button
                    type="button"
                    className="btn ghost small"
                    disabled={!!busy}
                    onClick={() => void onRefreshQuota(a.userId)}
                  >
                    Quota
                  </button>
                  <button
                    type="button"
                    className="btn danger small"
                    disabled={!!busy}
                    onClick={() => onRemove(a.userId, a.email)}
                  >
                    Remove
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </main>

      <footer className="footer">
        <span>
          Store: <code>~/.grok-switcher</code>
        </span>
        <span>
          Active auth: <code>~/.grok/auth.json</code>
        </span>
        {grokPath && (
          <span title={grokPath}>
            CLI: <code>{grokPath}</code>
          </span>
        )}
      </footer>

      {showSettings && (
        <div className="modal-backdrop" onClick={() => setShowSettings(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>Settings</h2>
            <label className="field">
              <span>Grok binary path</span>
              <input
                type="text"
                placeholder="~/.grok/bin/grok"
                value={settings.grokBinaryPath ?? ""}
                onChange={(e) =>
                  setSettings((s) => ({
                    ...s,
                    grokBinaryPath: e.target.value || null,
                  }))
                }
              />
            </label>
            <label className="field">
              <span>GROK_HOME override (optional)</span>
              <input
                type="text"
                placeholder="Leave empty for ~/.grok"
                value={settings.grokHome ?? ""}
                onChange={(e) =>
                  setSettings((s) => ({
                    ...s,
                    grokHome: e.target.value || null,
                  }))
                }
              />
            </label>
            <div className="modal-actions">
              <button type="button" className="btn ghost" onClick={() => setShowSettings(false)}>
                Cancel
              </button>
              <button type="button" className="btn primary" onClick={() => void saveSettings()} disabled={!!busy}>
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
