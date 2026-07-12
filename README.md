# Grok Switcher

Desktop app for **multi-account Grok Build** switching. Manage several Grok accounts, switch the active `auth.json` credentials, and check usage quotas — all from a simple Tauri UI.

Built with **Tauri 2**, **React**, and **TypeScript**.

## Features

- Add accounts via the official `grok login` flow
- Switch the active account (writes `~/.grok/auth.json`)
- Refresh quota / billing usage per account
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
