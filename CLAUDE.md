# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Clyde is

A Tauri 2 desktop app (menubar/tray) that lets you run plain `claude` across multiple Claude
accounts and switches between them to dodge usage limits. Rust core + a thin React/TypeScript UI.

## Commands

```bash
npm install                 # install JS deps (also pulls Tauri CLI)
npm run tauri dev           # run the app (Vite on :1420 + Rust, hot reload)
npm run tauri build         # produce a .app / installer
npm run build               # frontend only: tsc typecheck + vite bundle

# Rust core (run from repo root with the manifest path; there is no test runner for the UI)
cargo test    --manifest-path src-tauri/Cargo.toml                      # all Rust tests
cargo test    --manifest-path src-tauri/Cargo.toml strips_clyde_proxy   # single test by name
cargo fmt     --manifest-path src-tauri/Cargo.toml
cargo clippy  --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo check   --manifest-path src-tauri/Cargo.toml
```

Run the full Rust gate (`fmt` + `clippy -D warnings` + `test`) plus `npm run build` before opening a PR.
There are no JS/UI tests; the Rust tests live inline in `claude_sync.rs`, `usage.rs`, and `oauth.rs`.

## Architecture — read this first

See `ARCHITECTURE.md` for the full design. The short version:

**Mechanism (no proxy):** switching an account means rewriting Claude Code's *own*
credential store in place, then plain `claude` talks to `api.anthropic.com` as that account —
whether or not Clyde is running. Specifically, `claude_sync::activate` writes:

- the macOS Keychain item **`Claude Code-credentials`** (the OAuth secret), and
- `~/.claude/.claude.json` → `oauthAccount` (the displayed identity).

Clyde always targets the *default* `~/.claude` config dir / unsuffixed keychain service, ignoring
any `CLAUDE_CONFIG_DIR` the Clyde process itself inherited. On startup it self-heals by stripping
any stale proxy integration a previous version left in `settings.json`
(`cleanup_legacy_integration`).

### Rust modules (`src-tauri/src`)

| Module | Responsibility |
|---|---|
| `model.rs` | Shared types: `Account`, `Credential`, `UsageSnapshot`, view DTOs (`AccountView`, `AppSnapshot`). |
| `vault.rs` | Keychain-backed persistence of Clyde's *own* account list (`keyring` crate). |
| `oauth.rs` | PKCE browser login (authorize at `claude.com/cai`; 32-byte state; form-encoded), `/api/oauth/profile` lookup, and access-token refresh. |
| `usage.rs` | Parse the `GET /api/oauth/usage` JSON → `UsageSnapshot`. |
| `engine.rs` | `Core`: in-memory source of truth (accounts, active id, usage), token refresh, usage polling, UI event emission. Switching calls into `claude_sync`. |
| `claude_sync.rs` | Make a Clyde account the active Claude Code account by rewriting its keychain + `.claude.json`. |
| `import_claude.rs` | Discover & import existing logins from Claude Code config dirs / keychain entries. |
| `commands.rs` | Tauri commands exposed to the UI (the only Rust↔JS surface). |
| `lib.rs` | App wiring: plugins, tray menu, window hide-on-close, usage poll loop, command registration. |

`main.rs` just calls `clyde_lib::run()`.

### Engine model

- `Core` is an `Arc` held as Tauri managed state, shared across all commands. State lives behind
  an `RwLock`; concurrent token refreshes serialize behind a `tokio::Mutex` (`refresh_lock`).
- `Core::valid_bearer` refreshes an access token within `REFRESH_SKEW_MS` (60s) of expiry and
  persists the rotated credential.
- `poll_usage` runs on a 120s loop (see `lib.rs` `setup`), reading `GET /api/oauth/usage` per
  account (the same endpoint Claude Code's status line uses — no quota cost) so gauges fill even
  with no traffic.
- After any state change, `Core::emit` pushes an `AppSnapshot` to the frontend over the
  `clyde://update` Tauri event (`UPDATE_EVENT`). The UI never sees raw tokens — only DTOs.

### Frontend (`src/`)

Thin client. `lib/api.ts` is the single typed bridge — every backend call is an `invoke<…>` here,
mirroring the commands registered in `lib.rs`. `lib/useSnapshot.ts` subscribes to `clyde://update`
and holds the current `AppSnapshot`. `App.tsx` switches between two views: `views/Onboarding.tsx`
(no accounts yet) and `views/Dashboard.tsx`. Tailwind v4 via the Vite plugin (no config file).

**When you add a Tauri command:** register it in `lib.rs`'s `generate_handler!`, add the typed
wrapper in `src/lib/api.ts`, and add any new DTO to both `model.rs` and `src/lib/types.ts`.

## Conventions

- Tokens live only in the OS keychain — never log or write them to plaintext files.
- Keep the UI calm and jargon-free on the happy path.
