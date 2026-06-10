<div align="center">

# Clyde

**Account switcher & auto-failover for Claude Code.**

Run plain `claude` across all your Claude accounts — Clyde switches between them
automatically, *before* you ever hit a usage limit.

</div>

---

## Why

If you have more than one Claude subscription, you've probably done the dance:
hit your 5-hour limit on one account, quit, switch to a second account in another
config dir, and lose your flow. Worse, the two config dirs drift apart and you're
forever re-syncing settings.

Clyde fixes both:

- **One `claude`, one set of settings.** You keep using the default `~/.claude`
  config. Clyde never asks you to juggle config directories.
- **Switching happens underneath, mid-session.** A tiny local proxy injects the
  right account's credentials per request, so you never restart.
- **It pre-empts limits.** Clyde reads each account's live utilization and routes
  to the freshest one *before* a hard limit lands — and fails over instantly if
  one does.

## How it works

Claude Code lets you point it at a custom endpoint with `ANTHROPIC_BASE_URL`.
Clyde runs a small proxy on `localhost` and wires Claude Code to it with two keys
in `~/.claude/settings.json`. For every request the proxy:

1. picks the active account (balancing on live usage, honoring a pin),
2. mints a fresh `Authorization: Bearer` + `anthropic-beta: oauth-…`,
3. forwards to `api.anthropic.com`, streaming the response straight back,
4. reads the `anthropic-ratelimit-unified-*` headers to update each account's gauges,
5. on a `429` / hard limit, transparently retries on the next account.

```
   claude ──HTTP──▶ 127.0.0.1:8787 (Clyde) ──HTTPS──▶ api.anthropic.com
                         │
                         ├─ tokens in your OS Keychain, refreshed per account
                         ├─ live 5h / 7d utilization per account
                         └─ pre-emptive routing + 429 failover
```

Because the proxy supplies the real OAuth headers itself, the credential Claude
Code sends is irrelevant — which is exactly what lets a single config dir drive
any number of accounts.

> Full design notes in [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Status

🚧 **Early / work in progress.** macOS first; the architecture is cross-platform
(Windows/Linux to follow). See [`ROADMAP.md`](ROADMAP.md).

## Install (from source)

Requires [Rust](https://rustup.rs) and [Node 20+](https://nodejs.org).

```bash
git clone https://github.com/your-org/clyde
cd clyde
npm install
npm run tauri dev      # run in dev
npm run tauri build    # produce a .app / installer
```

## Usage

1. **Add your accounts** — sign in through the browser, or paste an existing
   token. Tokens are stored in your OS Keychain, never in plaintext.
2. **Connect Claude Code** — one click wires `~/.claude/settings.json` to the
   proxy. (One click to disconnect restores it exactly.)
3. **Use `claude` as normal.** Watch the gauges; Clyde handles the rest. Pin a
   specific account any time, or leave it on Auto.

## Security

- Credentials live only in the OS-native secret store (macOS Keychain / Windows
  Credential Manager / Linux Secret Service) via the `keyring` crate.
- The proxy binds to `127.0.0.1` only.
- Clyde talks to nothing but `api.anthropic.com` (the upstream) — there is no
  telemetry and no Clyde server.

See [`SECURITY.md`](SECURITY.md) to report issues.

## ⚠️ Responsible use

Clyde orchestrates **your own** Claude accounts on your own machine. That said:
automatically rotating between accounts to extend usage limits may run against
Anthropic's Terms of Service, and could put your accounts at risk. You are
responsible for how you use it. Clyde is an independent, unofficial project and
is not affiliated with or endorsed by Anthropic. "Claude" is a trademark of
Anthropic.

The OAuth client id / endpoints Clyde uses mirror Claude Code's public flow and
are not a stable API; they can change without notice.

## License

[MIT](LICENSE)
