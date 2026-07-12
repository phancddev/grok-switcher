import { useCallback, useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Update } from "@tauri-apps/plugin-updater";
import * as api from "../api";

type Status =
  | { kind: "idle" }
  | { kind: "checking" }
  | {
      kind: "available";
      version: string;
      notes?: string | null;
      /** tauri plugin update object when in-app install is possible */
      update?: Update;
      releaseUrl?: string;
      mode: "install" | "download";
    }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "ready" }
  | { kind: "error"; message: string };

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function UpdateChecker() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [dismissed, setDismissed] = useState(false);
  const [currentVersion, setCurrentVersion] = useState("");

  const check = useCallback(async () => {
    if (!isTauri()) return;
    setStatus({ kind: "checking" });
    setDismissed(false);

    try {
      const ver = await api.getAppVersion();
      setCurrentVersion(ver);
    } catch {
      /* ignore */
    }

    // 1) Prefer official Tauri updater (signed latest.json from GitHub Releases)
    try {
      const { check: checkUpdate } = await import("@tauri-apps/plugin-updater");
      const update = await checkUpdate();
      if (update) {
        setStatus({
          kind: "available",
          version: update.version,
          notes: update.body,
          update,
          mode: "install",
        });
        return;
      }
    } catch (err) {
      console.warn("Tauri updater check failed, falling back to GitHub API:", err);
    }

    // 2) Fallback: GitHub Releases API (no signing needed — open download page)
    try {
      const info = await api.checkGithubUpdate();
      if (info.hasUpdate) {
        setStatus({
          kind: "available",
          version: info.latestVersion,
          notes: info.releaseNotes,
          releaseUrl: info.releaseUrl,
          mode: "download",
        });
        return;
      }
      setStatus({ kind: "idle" });
    } catch (err) {
      console.warn("GitHub update check failed:", err);
      setStatus({ kind: "idle" });
    }
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    void check();
  }, [check]);

  const handleInstall = async () => {
    if (status.kind !== "available" || status.mode !== "install" || !status.update) return;
    const { update } = status;
    try {
      let downloaded = 0;
      let total: number | null = null;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? null;
            setStatus({ kind: "downloading", downloaded: 0, total });
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            setStatus({ kind: "downloading", downloaded, total });
            break;
          case "Finished":
            setStatus({ kind: "ready" });
            break;
        }
      });
      setStatus({ kind: "ready" });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setStatus({ kind: "error", message });
    }
  };

  const handleOpenRelease = async () => {
    if (status.kind !== "available") return;
    const url =
      status.releaseUrl ||
      `https://github.com/phancddev/grok-switcher/releases/tag/v${status.version}`;
    try {
      await openUrl(url);
    } catch {
      window.open(url, "_blank");
    }
  };

  const handleRelaunch = async () => {
    try {
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (err) {
      console.error(err);
    }
  };

  if (!isTauri() || dismissed || status.kind === "idle" || status.kind === "checking") {
    return null;
  }

  return (
    <div className="update-banner">
      <div className="update-card">
        {status.kind === "available" && (
          <div className="update-row">
            <div className="update-text">
              <p className="update-title">
                Update available: v{status.version}
                {currentVersion ? (
                  <span className="muted"> (you have v{currentVersion})</span>
                ) : null}
              </p>
              {status.notes && <p className="update-notes">{status.notes}</p>}
            </div>
            <div className="update-actions">
              <button type="button" className="btn secondary small" onClick={() => setDismissed(true)}>
                Later
              </button>
              {status.mode === "install" ? (
                <button type="button" className="btn primary small" onClick={() => void handleInstall()}>
                  Update
                </button>
              ) : (
                <button type="button" className="btn primary small" onClick={() => void handleOpenRelease()}>
                  Download
                </button>
              )}
            </div>
          </div>
        )}

        {status.kind === "downloading" && (
          <div>
            <div className="update-row">
              <p className="update-title">Downloading update…</p>
              <p className="muted">
                {formatBytes(status.downloaded)}
                {status.total ? ` / ${formatBytes(status.total)}` : ""}
              </p>
            </div>
            <div className="usage-track" style={{ marginTop: 8 }}>
              <div
                className="usage-fill ok"
                style={{
                  width:
                    status.total && status.total > 0
                      ? `${Math.min(100, (status.downloaded / status.total) * 100)}%`
                      : "40%",
                }}
              />
            </div>
          </div>
        )}

        {status.kind === "ready" && (
          <div className="update-row">
            <p className="update-title">Update ready. Restart to apply.</p>
            <div className="update-actions">
              <button type="button" className="btn secondary small" onClick={() => setDismissed(true)}>
                Later
              </button>
              <button type="button" className="btn primary small" onClick={() => void handleRelaunch()}>
                Restart
              </button>
            </div>
          </div>
        )}

        {status.kind === "error" && (
          <div className="update-row">
            <p className="update-title">Update failed: {status.message}</p>
            <button type="button" className="btn secondary small" onClick={() => setDismissed(true)}>
              Dismiss
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
