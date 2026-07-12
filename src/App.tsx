import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { LoginStatusEvent } from "./api";
import type { AccountSummary, Settings } from "./types";
import "./App.css";

function displayName(a: AccountSummary): string {
  if (a.label?.trim()) return a.label.trim();
  const parts = [a.firstName, a.lastName].filter(Boolean).join(" ").trim();
  return parts || a.email;
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

function QuotaProgress({ account }: { account: AccountSummary }) {
  const q = account.quota;
  const percent = Math.min(100, Math.max(0, q?.percentUsed ?? 0));
  const used = q ? Math.round(q.used) : null;
  const limit = q ? Math.round(q.monthlyLimit) : null;
  const remaining = used != null && limit != null ? Math.max(0, limit - used) : null;
  const tone = percent >= 90 ? "danger" : percent >= 70 ? "warn" : "ok";

  return (
    <div className="quota-block">
      <div className="quota-head">
        <span className="quota-title">Quota</span>
        <span className={`quota-percent ${tone}`}>
          {q ? `${percent.toFixed(1)}%` : "—"}
        </span>
      </div>
      <div className="quota-bar" role="progressbar" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
        <div className={`quota-fill ${tone}`} style={{ width: `${percent}%` }} />
      </div>
      <div className="quota-meta">
        {q ? (
          <>
            <span>
              <strong>{used?.toLocaleString()}</strong>
              {" / "}
              {limit?.toLocaleString()} credits
            </span>
            <span className="muted">
              {remaining != null ? `${remaining.toLocaleString()} left` : ""}
              {periodLabel(account) ? ` · ${periodLabel(account)}` : ""}
            </span>
          </>
        ) : (
          <span className="muted">No quota data — click Refresh</span>
        )}
      </div>
    </div>
  );
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

  // Add-account modal
  const [showAdd, setShowAdd] = useState(false);
  const [addLabel, setAddLabel] = useState("");
  const [loginUrl, setLoginUrl] = useState<string | null>(null);
  const [loginCode, setLoginCode] = useState<string | null>(null);
  const [loginMessages, setLoginMessages] = useState<string[]>([]);
  const [copied, setCopied] = useState<"url" | "code" | null>(null);
  const [addPhase, setAddPhase] = useState<"form" | "waiting" | "done">("form");

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const list = await api.listAccounts();
      setAccounts(list);
      const path = await api.resolveGrokBinary();
      setGrokPath(path);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg.replace(/^Error:\s*/, ""));
      setAccounts([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Listen for login-status events while adding
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<LoginStatusEvent>("login-status", (event) => {
      const { kind, value } = event.payload;
      if (kind === "url" && value) {
        setLoginUrl(value);
      } else if (kind === "code" && value) {
        setLoginCode(value);
      } else if (kind === "message" && value) {
        setLoginMessages((prev) => {
          const next = [...prev, value];
          return next.slice(-6);
        });
      } else if (kind === "done") {
        setAddPhase("done");
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, []);

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

  const openAddModal = () => {
    setShowAdd(true);
    setAddLabel("");
    setLoginUrl(null);
    setLoginCode(null);
    setLoginMessages([]);
    setCopied(null);
    setAddPhase("form");
    setError(null);
    setInfo(null);
  };

  const closeAddModal = () => {
    if (busy === "add") return; // don't close while login running
    setShowAdd(false);
    setAddPhase("form");
  };

  const onConfirmAdd = () =>
    withBusy("add", async () => {
      setAddPhase("waiting");
      setLoginUrl("https://auth.x.ai/device");
      setLoginCode(null);
      setLoginMessages(["Starting device login…"]);
      const label = addLabel.trim() || null;
      try {
        const acc = await api.addAccount(label);
        setAddPhase("done");
        setInfo(`Added ${acc.label || acc.email}`);
        setShowAdd(false);
      } catch (e) {
        setAddPhase("form");
        throw e;
      }
    });

  const copyText = async (text: string, which: "url" | "code") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(which);
      setTimeout(() => setCopied(null), 1500);
    } catch {
      setError("Could not copy to clipboard");
    }
  };

  const onImport = () => {
    const label = prompt("Optional nickname for this account:") ?? undefined;
    void withBusy("import", async () => {
      const acc = await api.importCurrentAccount(label?.trim() || null);
      setInfo(`Imported ${acc.label || acc.email}`);
    });
  };

  const onSwitch = (userId: string) =>
    withBusy(`switch-${userId}`, async () => {
      const acc = await api.switchAccount(userId);
      setInfo(`Switched to ${acc.label || acc.email}`);
    });

  const onRemove = (userId: string, name: string) => {
    if (!confirm(`Remove account “${name}” from Grok Switcher?\n(Does not delete the Grok account.)`)) {
      return;
    }
    void withBusy(`remove-${userId}`, async () => {
      await api.removeAccount(userId);
      setInfo(`Removed ${name}`);
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
          <button type="button" className="btn primary" onClick={openAddModal} disabled={!!busy}>
            Add account
          </button>
        </div>
      </header>

      {(error || info) && (
        <div className={`banner ${error ? "error" : "info"}`}>
          <span>{error ?? info}</span>
          <button
            type="button"
            className="banner-close"
            onClick={() => {
              setError(null);
              setInfo(null);
            }}
          >
            ×
          </button>
        </div>
      )}

      <main className="main">
        {loading ? (
          <div className="empty">Loading…</div>
        ) : accounts.length === 0 ? (
          <div className="empty">
            <h2>No accounts yet</h2>
            <p>
              Click <strong>Add account</strong> to name an account and sign in with a copyable link,
              or <strong>Import current</strong> for the session in <code>~/.grok/auth.json</code>.
            </p>
            {!grokPath && (
              <p className="warn">
                Grok CLI not found. Open Settings and set the binary path (e.g.{" "}
                <code>~/.grok/bin/grok</code>).
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
                  {a.label && displayName(a) !== a.email && (
                    <div className="email subtle-label">
                      {a.firstName || a.lastName
                        ? [a.firstName, a.lastName].filter(Boolean).join(" ")
                        : null}
                    </div>
                  )}
                  <QuotaProgress account={a} />
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
                    {busy === `quota-${a.userId}` ? "…" : "Refresh"}
                  </button>
                  <button
                    type="button"
                    className="btn danger small"
                    disabled={!!busy}
                    onClick={() => onRemove(a.userId, displayName(a))}
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

      {/* Add account modal */}
      {showAdd && (
        <div className="modal-backdrop" onClick={closeAddModal}>
          <div className="modal modal-wide" onClick={(e) => e.stopPropagation()}>
            <h2>Add account</h2>

            {addPhase === "form" && (
              <>
                <label className="field">
                  <span>Nickname (gợi nhớ)</span>
                  <input
                    type="text"
                    autoFocus
                    placeholder="e.g. Work, Personal, Acc 2…"
                    value={addLabel}
                    onChange={(e) => setAddLabel(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void onConfirmAdd();
                    }}
                  />
                </label>
                <p className="hint">
                  After you click <strong>Add</strong>, a login link and device code will appear so you can
                  copy them and finish sign-in in the browser.
                </p>
                <div className="modal-actions">
                  <button type="button" className="btn ghost" onClick={closeAddModal}>
                    Cancel
                  </button>
                  <button
                    type="button"
                    className="btn primary"
                    onClick={() => void onConfirmAdd()}
                    disabled={!!busy}
                  >
                    Add
                  </button>
                </div>
              </>
            )}

            {(addPhase === "waiting" || addPhase === "done") && (
              <>
                {addLabel.trim() && (
                  <p className="hint">
                    Saving as nickname: <strong>{addLabel.trim()}</strong>
                  </p>
                )}

                <div className="copy-box">
                  <div className="copy-label">Login link</div>
                  <div className="copy-row">
                    <code className="copy-value">{loginUrl ?? "Waiting for link…"}</code>
                    <button
                      type="button"
                      className="btn secondary small"
                      disabled={!loginUrl}
                      onClick={() => loginUrl && void copyText(loginUrl, "url")}
                    >
                      {copied === "url" ? "Copied!" : "Copy"}
                    </button>
                  </div>
                </div>

                <div className="copy-box">
                  <div className="copy-label">Device code</div>
                  <div className="copy-row">
                    <code className={`copy-value code ${loginCode ? "" : "muted"}`}>
                      {loginCode ?? "Waiting for code from CLI…"}
                    </code>
                    <button
                      type="button"
                      className="btn secondary small"
                      disabled={!loginCode}
                      onClick={() => loginCode && void copyText(loginCode, "code")}
                    >
                      {copied === "code" ? "Copied!" : "Copy"}
                    </button>
                  </div>
                </div>

                <ol className="steps">
                  <li>Copy the link and open it in a browser</li>
                  <li>Paste / enter the device code</li>
                  <li>Sign in with the Grok account you want to add</li>
                  <li>Return here — the app will capture the session automatically</li>
                </ol>

                {loginMessages.length > 0 && (
                  <div className="login-log">
                    {loginMessages.map((m, i) => (
                      <div key={`${i}-${m.slice(0, 24)}`}>{m}</div>
                    ))}
                  </div>
                )}

                <div className="modal-actions">
                  <button
                    type="button"
                    className="btn ghost"
                    onClick={closeAddModal}
                    disabled={busy === "add"}
                  >
                    {busy === "add" ? "Waiting…" : "Close"}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}

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
              <button
                type="button"
                className="btn primary"
                onClick={() => void saveSettings()}
                disabled={!!busy}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
