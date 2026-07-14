# Grok Switcher

Desktop app for **multi-account Grok Build** switching. Manage several Grok accounts, switch the active `auth.json` credentials, and check usage quotas — all from a simple Tauri UI.

Built with **Tauri 2**, **React**, and **TypeScript**.

## Features

- Add accounts via the official `grok login` flow
- Switch the active account (writes `~/.grok/auth.json`)
- Refresh quota / billing usage per account
- Hide account names and emails individually or all at once
- Remove saved accounts with a cross-platform confirmation dialog
- Portable installers: macOS DMG, Windows NSIS (current user), Linux AppImage + deb

## Prerequisites

- [Node.js](https://nodejs.org/) 20+ (22 recommended)
- [Rust](https://rustup.rs/) stable
- Platform deps for Tauri: see [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Grok Build CLI](https://grok.com) (`grok` on your `PATH`) for **Add Account**

## Development

```bash
npm install
npm run tauri dev
```

Frontend only (Vite, no Rust shell):

```bash
npm run dev
```

## Build

```bash
npm run tauri build
```

Artifacts land under `src-tauri/target/release/bundle/` (platform-specific: `dmg`, `nsis`, `appimage`, `deb`, etc.).

## Install from Releases

GitHub Actions builds on every push to `main` and on tags matching `v*` (e.g. `v0.1.0`). Releases are created as **drafts** — publish them under **Releases** when ready.

| OS | What to download | How to install |
|----|------------------|----------------|
| **macOS** | `.dmg` (arm64 or x64) | Open the DMG → drag **Grok Switcher** into **Applications** |
| **Windows** | NSIS `.exe` | Run the installer (per-user / current user) |
| **Linux** | `.AppImage` | `chmod +x Grok*.AppImage && ./Grok*.AppImage` |
| **Linux** | `.deb` | `sudo dpkg -i grok-switcher_*.deb` |

Unsigned macOS builds may need **System Settings → Privacy & Security** → “Open Anyway” the first time.

## Data storage

| Path | Purpose |
|------|---------|
| `~/.grok-switcher/` | App data (accounts, meta, settings) |
| `~/.grok-switcher/accounts/` | Snapshots of each account’s auth |
| `~/.grok-switcher/meta.json` | Account list, labels, active user, cached quota |
| `~/.grok-switcher/settings.json` | Optional `grok` binary path / `GROK_HOME` override |
| `~/.grok/auth.json` | **Active** Grok Build credentials (what CLI uses) |

On Windows, `~` is `%USERPROFILE%`. You can override the Grok home with the `GROK_HOME` environment variable or in app settings.

## Project layout

```
├── src/                 # React + TypeScript UI
├── src-tauri/           # Rust backend (Tauri commands, paths, billing)
├── .github/workflows/   # Release CI
└── package.json
```

## License

Private / use as you like unless otherwise stated.

## Auto Updates

Grok Switcher checks for updates on launch (same idea as [Codex Switcher](https://github.com/Lampese/codex-switcher)):

1. **In-app install** via `tauri-plugin-updater` when GitHub Releases contain a signed `latest.json`.
2. **Fallback** via the GitHub Releases API: shows “Update available” and opens the release page to download.

### One-time signing key setup (for install-from-app)

```bash
# Generate keypair (private key must stay secret)
npx tauri signer generate -w ~/.tauri/grok-switcher.key

# Put the *public* key into src-tauri/tauri.conf.json → plugins.updater.pubkey
# Put the *private* key into GitHub Actions secrets:
#   TAURI_SIGNING_PRIVATE_KEY          (file contents)
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD (if you set one)
```

Back up the private key (and its password, if any) outside the repository. Existing installs can only accept updates signed by the matching key. The release workflow validates the signing credentials before starting the platform build matrix.

Publish a versioned release (tag `v0.x.y`). CI builds installers and attaches `latest.json`. The app endpoint is:

`https://github.com/phancddev/grok-switcher/releases/latest/download/latest.json`

Without a valid private key secret, the release workflow stops at the preflight job instead of failing after the platform bundles have already been built.
