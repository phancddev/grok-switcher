import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { LoginStatusEvent } from "./api";
import type { AccountSummary, Settings } from "./types";
import { UpdateChecker } from "./components/UpdateChecker";
import "./App.css";

function displayName(a: AccountSummary): string {
  if (a.label?.trim()) return a.label.trim();
  const parts = [a.firstName, a.lastName].filter(Boolean).join(" ").trim();
  return parts || a.email;
}

function formatWhen(iso?: string | null): string {
  if (!iso) return "—";
  try {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

/** Short date for release strings (YYYY-MM-DD or ISO) → e.g. "Jul 14, 2026" */
function formatReleaseDate(raw?: string | null): string | null {
  if (!raw) return null;
  try {
    // Prefer date-only parse so UTC midnight does not shift the day in local TZ.
    const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(raw.trim());
    const d = m
      ? new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]))
      : new Date(raw);
    if (Number.isNaN(d.getTime())) return raw;
    return d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  } catch {
    return raw;
  }
}

const LAST_SEEN_VERSION_KEY = "gs-last-seen-version";
const PENDING_UPDATE_NOTES_KEY = "gs-pending-update-notes";

function summarizeUpdateNotes(notes?: string): string | null {
  if (!notes) return null;
  const firstMeaningfulLine = notes
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line.length > 0 && !line.startsWith("#"));
  if (!firstMeaningfulLine) return null;
  const cleaned = firstMeaningfulLine.replace(/^[-*]\s+/, "");
  return cleaned.length > 140 ? `${cleaned.slice(0, 137)}…` : cleaned;
}

/** Relative reset like codex-switcher: "2h 15m", "now" */
function formatResetRelative(iso?: string | null): string {
  if (!iso) return "";
  const end = new Date(iso).getTime();
  if (Number.isNaN(end)) return "";
  const diff = Math.floor((end - Date.now()) / 1000);
  if (diff <= 0) return "now";
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) {
    const h = Math.floor(diff / 3600);
    const m = Math.floor((diff % 3600) / 60);
    return m > 0 ? `${h}h ${m}m` : `${h}h`;
  }
  const d = Math.floor(diff / 86400);
  return `${d}d`;
}

/** Color by remaining % (green = plenty left) — codex style */
function toneForRemaining(remainingPercent: number): "ok" | "warn" | "danger" {
  if (remainingPercent <= 10) return "danger";
  if (remainingPercent <= 30) return "warn";
  return "ok";
}

function PeriodBar({
  title,
  used,
  limit,
  percent,
  resetsAt,
  percentOnly,
}: {
  title: string;
  used: number;
  limit: number;
  percent: number;
  resetsAt?: string;
  percentOnly?: boolean;
}) {
  const usedP = Math.min(100, Math.max(0, percent));
  const remainingP = Math.max(0, 100 - usedP);
  const tone = toneForRemaining(remainingP);
  const resetRel = formatResetRelative(resetsAt);
  const exact = formatWhen(resetsAt);

  return (
    <div className="usage-bar">
      <div className="usage-bar-row">
        <span className="usage-bar-label">{title}</span>
        <span className="usage-bar-meta">
          <strong className={`usage-left ${tone}`}>{remainingP.toFixed(0)}% left</strong>
          {resetRel ? (
            <span className="muted">
              {" "}
              · resets {resetRel}
              {exact !== "—" ? ` (${exact})` : ""}
            </span>
          ) : null}
        </span>
      </div>
      <div
        className="usage-track"
        role="progressbar"
        aria-valuenow={remainingP}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div className={`usage-fill ${tone}`} style={{ width: `${remainingP}%` }} />
      </div>
      {!percentOnly && limit > 0 && limit !== 100 && (
        <div className="usage-sub">
          {Math.round(used).toLocaleString()} / {Math.round(limit).toLocaleString()} credits used
        </div>
      )}
    </div>
  );
}

function QuotaProgress({ account }: { account: AccountSummary }) {
  const q = account.quota;
  const weekly = q?.weekly;
  const monthly =
    q?.monthly ??
    (q
      ? {
          kind: "monthly",
          label: "Monthly",
          used: q.used,
          limit: q.monthlyLimit,
          percentUsed: q.percentUsed,
          periodStart: q.billingPeriodStart,
          periodEnd: q.billingPeriodEnd,
          resetsAt: q.resetsAt || q.billingPeriodEnd,
          daysUntilReset: q.daysUntilReset ?? 0,
          source: "api",
        }
      : null);

  if (!q) {
    return <div className="usage-empty muted">No rate limit data — click Refresh</div>;
  }

  return (
    <div className="usage-stack">
      {weekly && (
        <PeriodBar
          title="Weekly limit"
          used={weekly.used}
          limit={weekly.limit}
          percent={weekly.percentUsed}
          resetsAt={weekly.resetsAt || weekly.periodEnd}
          percentOnly={weekly.limit === 100 && weekly.source === "api"}
        />
      )}
      {monthly && (
        <PeriodBar
          title="Monthly limit"
          used={monthly.used}
          limit={monthly.limit}
          percent={monthly.percentUsed}
          resetsAt={monthly.resetsAt || monthly.periodEnd}
        />
      )}
    </div>
  );
}

function AccountCardView({
  account,
  busy,
  onSwitch,
  onRefresh,
  onRemove,
}: {
  account: AccountSummary;
  busy: string | null;
  onSwitch: (id: string) => void;
  onRefresh: (id: string) => void;
  onRemove: (id: string, name: string) => void;
}) {
  const name = displayName(account);
  const plan = account.subscriptionTier || (account.tier != null ? `Tier ${account.tier}` : "Unknown");
  const switching = busy === `switch-${account.userId}`;
  const refreshing = busy === `quota-${account.userId}`;

  return (
    <article className={`account-card ${account.isActive ? "is-active" : ""}`}>
      <div className="account-card-top">
        <div className="account-card-identity">
          <div className="account-card-name-row">
            {account.isActive && (
              <span className="active-dot" title="Active">
                <span className="active-dot-ping" />
                <span className="active-dot-core" />
              </span>
            )}
            <h3 className="account-card-name">{name}</h3>
          </div>
          <p className="account-card-email">{account.email}</p>
        </div>
        <div className="account-card-badges">
          <span className="plan-chip">{plan}</span>
          {account.isActive && <span className="active-chip">Active</span>}
        </div>
      </div>

      <div className="account-card-usage">
        <QuotaProgress account={account} />
      </div>

      {account.planExpiresAt && (
        <div className="account-card-submeta muted">Until {formatWhen(account.planExpiresAt)}</div>
      )}

      <div className="account-card-actions">
        {account.isActive ? (
          <button type="button" className="btn btn-active-state" disabled>
            ✓ Active
          </button>
        ) : (
          <button
            type="button"
            className="btn btn-switch"
            disabled={!!busy}
            onClick={() => onSwitch(account.userId)}
          >
            {switching ? "Switching…" : "Switch"}
          </button>
        )}
        <button
          type="button"
          className="btn btn-icon"
          disabled={!!busy}
          title="Refresh usage"
          onClick={() => onRefresh(account.userId)}
        >
          <span className={refreshing ? "spin" : undefined}>↻</span>
        </button>
        <button
          type="button"
          className="btn btn-icon btn-danger-soft"
          disabled={!!busy}
          title="Remove account"
          onClick={() => onRemove(account.userId, name)}
        >
          ✕
        </button>
      </div>
    </article>
  );
}

type ThemeMode = "light" | "dark" | "system";

function resolveTheme(mode: ThemeMode): "light" | "dark" {
  if (mode === "light" || mode === "dark") return mode;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(mode: ThemeMode) {
  const resolved = resolveTheme(mode);
  document.documentElement.setAttribute("data-theme", resolved);
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
  const [theme, setTheme] = useState<ThemeMode>(() => {
    const saved = localStorage.getItem("gs-theme") as ThemeMode | null;
    return saved === "light" || saved === "dark" || saved === "system" ? saved : "system";
  });
  const [appInfo, setAppInfo] = useState<api.AppInfo | null>(null);

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

  // Load version / release date; one-shot "Updated to…" toast when version changes.
  useEffect(() => {
    let cancelled = false;
    // Capture this before the theme effect creates the key during a first launch.
    const hadExistingInstallState = localStorage.getItem("gs-theme") !== null;
    void (async () => {
      try {
        const info = await api.getAppInfo();
        if (cancelled) return;
        setAppInfo(info);
        const lastSeen = localStorage.getItem(LAST_SEEN_VERSION_KEY);
        const pendingRaw = localStorage.getItem(PENDING_UPDATE_NOTES_KEY);
        let pending: { version?: string; notes?: string } | null = null;
        if (pendingRaw) {
          try {
            pending = JSON.parse(pendingRaw) as { version?: string; notes?: string };
          } catch {
            /* stale/invalid data is cleared below */
          }
        }

        const updatedFromKnownVersion = !!lastSeen && lastSeen !== info.version;
        const updatedByInAppUpdater = pending?.version === info.version;
        // v0.1.1 predates LAST_SEEN_VERSION_KEY, but already persisted gs-theme.
        const upgradedFromPreMarkerVersion = !lastSeen && hadExistingInstallState;
        if (updatedFromKnownVersion || updatedByInAppUpdater || upgradedFromPreMarkerVersion) {
          const dateLabel = formatReleaseDate(info.releaseDate);
          const base = dateLabel
            ? `Updated to v${info.version} · ${dateLabel}`
            : `Updated to v${info.version}`;
          const notes = updatedByInAppUpdater ? summarizeUpdateNotes(pending?.notes) : null;
          setInfo(notes ? `${base} — ${notes}` : base);
        }
        localStorage.setItem(LAST_SEEN_VERSION_KEY, info.version);
        if (pendingRaw) localStorage.removeItem(PENDING_UPDATE_NOTES_KEY);
      } catch {
        /* non-critical (e.g. browser preview without Tauri) */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    applyTheme(theme);
    localStorage.setItem("gs-theme", theme);
    if (theme !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme("system");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [theme]);

  const cycleTheme = () => {
    setTheme((t) => (t === "system" ? "light" : t === "light" ? "dark" : "system"));
  };

  const resolvedTheme = resolveTheme(theme);
  const themeTitle =
    theme === "dark" ? "Dark" : theme === "light" ? "Light" : `System (${resolvedTheme})`;

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
    const isActive = accounts.some((a) => a.userId === userId && a.isActive);
    const others = accounts.filter((a) => a.userId !== userId).length;
    let msg = `Remove account “${name}” from Grok Switcher?\n(Does not delete the Grok cloud account.)`;
    if (isActive) {
      msg +=
        others > 0
          ? "\n\nThis is the active CLI session — Grok will switch to another saved account."
          : "\n\nThis is the only account — the live Grok CLI session will be cleared.";
    }
    if (!confirm(msg)) {
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

  const activeAccounts = accounts.filter((a) => a.isActive);
  const otherAccounts = accounts.filter((a) => !a.isActive);
  const releaseLabel = formatReleaseDate(appInfo?.releaseDate);

  return (
    <div className="app">
      <UpdateChecker />
      <header className="header">
        <div className="header-inner">
          <div className="brand">
            <img className="brand-logo" src="/logo.png" alt="Grok Switcher" width={40} height={40} />
            <div>
              <h1>Grok Switcher</h1>
              <p className="subtitle">Switch accounts · monitor weekly &amp; monthly quota</p>
            </div>
          </div>
          <div className="header-actions">
            <button
              type="button"
              className="btn theme-toggle"
              onClick={cycleTheme}
              title={`Theme: ${themeTitle} (click to cycle)`}
              aria-label={`Theme ${themeTitle}`}
            >
              <svg
                className="theme-icon"
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="none"
                xmlns="http://www.w3.org/2000/svg"
                aria-hidden="true"
              >
                <circle cx="12" cy="12" r="4" fill="currentColor" />
                <g stroke="currentColor" strokeWidth="1.75" strokeLinecap="round">
                  <line x1="12" y1="2" x2="12" y2="4.5" />
                  <line x1="12" y1="19.5" x2="12" y2="22" />
                  <line x1="2" y1="12" x2="4.5" y2="12" />
                  <line x1="19.5" y1="12" x2="22" y2="12" />
                  <line x1="4.93" y1="4.93" x2="6.7" y2="6.7" />
                  <line x1="17.3" y1="17.3" x2="19.07" y2="19.07" />
                  <line x1="4.93" y1="19.07" x2="6.7" y2="17.3" />
                  <line x1="17.3" y1="6.7" x2="19.07" y2="4.93" />
                </g>
              </svg>
            </button>
            <button
              type="button"
              className="btn btn-icon"
              onClick={() => void onRefreshQuota()}
              disabled={!!busy || accounts.length === 0}
              title="Refresh all quotas"
            >
              <span className={busy === "quota-all" ? "spin" : undefined}>↻</span>
            </button>
            <button
              type="button"
              className="btn btn-icon"
              onClick={() => void openSettings()}
              disabled={!!busy}
              title="Settings"
            >
              ⚙
            </button>
            <button type="button" className="btn secondary" onClick={() => void onImport()} disabled={!!busy}>
              Import
            </button>
            <button type="button" className="btn primary" onClick={openAddModal} disabled={!!busy}>
              + Add account
            </button>
          </div>
        </div>
      </header>

      {(error || info) && (
        <div className={`toast ${error ? "error" : "ok"}`}>
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
          <div className="empty-state">
            <div className="spinner" />
            <p className="muted">Loading accounts…</p>
          </div>
        ) : accounts.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">👤</div>
            <h2>No accounts yet</h2>
            <p className="muted">
              Add a Grok Build account with device login, or import the session in{" "}
              <code>~/.grok/auth.json</code>.
            </p>
            {!grokPath && (
              <p className="warn">Grok CLI not found — set the binary path in Settings.</p>
            )}
            <button type="button" className="btn primary" onClick={openAddModal}>
              + Add account
            </button>
          </div>
        ) : (
          <div className="sections">
            {activeAccounts.length > 0 && (
              <section className="section">
                <h2 className="section-title">Active account</h2>
                <div className="card-grid card-grid-one">
                  {activeAccounts.map((a) => (
                    <AccountCardView
                      key={a.userId}
                      account={a}
                      busy={busy}
                      onSwitch={(id) => void onSwitch(id)}
                      onRefresh={(id) => void onRefreshQuota(id)}
                      onRemove={onRemove}
                    />
                  ))}
                </div>
              </section>
            )}
            {otherAccounts.length > 0 && (
              <section className="section">
                <h2 className="section-title">Other accounts</h2>
                <div className="card-grid">
                  {otherAccounts.map((a) => (
                    <AccountCardView
                      key={a.userId}
                      account={a}
                      busy={busy}
                      onSwitch={(id) => void onSwitch(id)}
                      onRefresh={(id) => void onRefreshQuota(id)}
                      onRemove={onRemove}
                    />
                  ))}
                </div>
              </section>
            )}
          </div>
        )}
      </main>

      <footer className="footer">
        {appInfo && (
          <span className="footer-version" title={appInfo.releaseDate ?? undefined}>
            <code>
              {releaseLabel
                ? `v${appInfo.version} · ${releaseLabel}`
                : `v${appInfo.version}`}
            </code>
          </span>
        )}
        <span>
          Store <code>~/.grok-switcher</code>
        </span>
        <span>
          Auth <code>~/.grok/auth.json</code>
        </span>
        {grokPath && (
          <span title={grokPath}>
            CLI <code>{grokPath}</code>
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

            <div className="about-block">
              <h3 className="about-title">About</h3>
              <div className="about-row">
                <span className="muted">Version</span>
                <code>{appInfo ? `v${appInfo.version}` : "—"}</code>
              </div>
              <div className="about-row">
                <span className="muted">Released</span>
                <code>{releaseLabel ?? appInfo?.releaseDate ?? "—"}</code>
              </div>
            </div>

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
