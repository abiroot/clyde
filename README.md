<div align="center">

# Clyde

**Account switcher for Claude Code.**

Run plain `claude` across all your Claude accounts — Clyde switches the active one
in a click, so you can move off an account *before* you hit a usage limit.

[![CI](https://github.com/Abiroot/clyde/actions/workflows/ci.yml/badge.svg)](https://github.com/Abiroot/clyde/actions/workflows/ci.yml)
&nbsp;[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
&nbsp;![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)

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
- **Switching is one click — no proxy, no restart of Clyde.** Clyde writes the
  chosen account straight into Claude Code's own credential store; plain `claude`
  then runs as that account, whether or not Clyde is open.
- **It shows you the headroom.** Clyde reads each account's live utilization so you
  can switch to the freshest one *before* a hard limit lands.

## How it works

Claude Code reads its subscription token from one place — the OS keychain item for
its config dir (`Claude Code-credentials` for the default `~/.claude`) — plus the
displayed identity in `~/.claude/.claude.json`. To make `claude` run as a different
account, Clyde rewrites those two things, in place, with the chosen account's OAuth:

1. it keeps each account's OAuth (refresh token) in your OS Keychain,
2. on switch, it refreshes that account's token if needed and writes it into Claude
   Code's `Claude Code-credentials` keychain entry, updating `.claude.json`'s
   identity to match,
3. plain `claude` then talks straight to `api.anthropic.com` as that account,
4. in the background Clyde reads each account's `GET /api/oauth/usage` — the same
   endpoint Claude Code's own status line uses — to keep the 5h / 7d gauges current
   (no messages are sent, so it costs no quota).

```
   you ──click──▶ Clyde ──writes──▶ Claude Code keychain + .claude.json
                    │
                    ├─ each account's OAuth in your OS Keychain, refreshed
                    └─ live 5h / 7d utilization per account (GET /api/oauth/usage)

   claude ──HTTPS──▶ api.anthropic.com   (directly, as the chosen account)
```

Because Clyde supplies the real credential to Claude Code's own store, a single
config dir can drive any number of accounts. A switch applies to new `claude`
runs immediately, and running sessions follow within ~30 seconds (Claude Code
re-reads the keychain on a short cache). Clyde tells you when a switch happened
while sessions were running.

**Chrome is the exception.** Claude Code and the Claude-in-Chrome extension find
each other through a rendezvous keyed on account, so `/chrome` only works while
the browser is signed into the *same* claude.ai account that's active. Switching
leaves the browser behind, and Clyde can't move it — the extension holds its
tokens in memory and only accepts a sign-in from a claude.ai page. Instead Clyde
detects it and says which account Chrome is on; when that's an account Clyde
already manages, one click switches to it and browser tools work again without
touching Chrome at all.

**Browser tools for every account (optional).** Clyde can instead set up
[Open Claude in Chrome](https://github.com/noemica-io/open-claude-in-chrome), an
open-source extension that reaches Chrome over a local socket with no account
attached, so browser tools keep working through every switch. Clyde downloads it
from upstream (pinned to a reviewed commit), installs its helper, and registers it
with Claude Code; you load the extension once from `chrome://extensions`. See
[Browser tools](#browser-tools-open-claude-in-chrome) below.

> Full design notes in [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Status

🚧 **Early / work in progress.** macOS first; the architecture is cross-platform
(Windows/Linux to follow). See [`ROADMAP.md`](ROADMAP.md).

## Install (from source)

Requires [Rust](https://rustup.rs) and [Node 20+](https://nodejs.org).

```bash
git clone https://github.com/Abiroot/clyde
cd clyde
npm install
npm run tauri dev      # run in dev
npm run tauri build    # produce a .app / installer
```

## Usage

1. **Add your accounts** — four ways, whichever fits:
   - **Detected** — import logins Claude Code already has on this machine (no
     re-auth; reuses their tokens).
   - **Browser** — sign in to any account in your browser and paste the code back.
   - **Terminal** — let Claude Code's own `/login` run in an isolated profile.
   - **Token** — paste an existing OAuth token JSON.

   Tokens are stored in your OS Keychain, never in plaintext.
2. **Pick the active account** — one click makes it the account Claude Code uses.
   Clyde writes it into Claude Code's own credential store; nothing else to wire up.
3. **Use `claude` as normal.** Watch the gauges and switch whenever an account is
   running hot. (New `claude` runs switch immediately; running sessions follow
   within ~30 seconds. If you use `/chrome`, keep the browser signed into the
   same account — Clyde flags it when they drift apart, or set up the
   account-independent browser tools below.)

## Browser tools (Open Claude in Chrome)

Optional. Open it from **Make browser tools work for every account** on the
dashboard. Requires Node.js.

1. **Set up** — Clyde clones the project to `~/Projects/Vendor/open-claude-in-chrome`
   at the commit it was reviewed against, installs the helper's dependencies
   (`npm ci --ignore-scripts`), registers the native messaging host for Chrome,
   Edge and Brave, and adds a user-scope MCP server named `open-claude-in-chrome`.
2. **Load it in Chrome** — `chrome://extensions` → Developer mode → Load unpacked →
   the `extension` folder. Clyde predicts the id Chrome assigns and confirms it.
3. **Restart Chrome.** The checklist ticks itself off.

New `claude` sessions then have `mcp__open-claude-in-chrome__*` tools, whichever
account is active. Things to know before you turn it on:

- **Licence.** Open Claude in Chrome is licensed **PolyForm Noncommercial 1.0.0**
  (its README says MIT; the LICENSE file governs). Clyde never ships its code — it
  downloads it onto your machine — and whether your use is noncommercial is yours
  to judge.
- **No site blocklist.** The official extension refuses banking, crypto, payment
  and some other sites; this one doesn't. Load it in a Chrome profile that isn't
  signed into anything you wouldn't hand to an agent, and in one profile only.
- **Unreviewed updates.** Clyde flags the copy if it's moved off the reviewed
  commit (e.g. after a manual `git pull`).
- Mouse and keyboard actions use Chrome's debugger API, which Chrome blocks on
  pages where another extension has injected a frame; reading and navigating
  still work there.
- **Remove** in the same dialog unregisters the MCP server and the native host.

## Security

- Browser tools (optional) run a third-party extension with access to every site
  in the Chrome profile it's loaded into — see
  [Browser tools](#browser-tools-open-claude-in-chrome) for the precautions.

- Credentials live only in the OS-native secret store (macOS Keychain / Windows
  Credential Manager / Linux Secret Service) via the `keyring` crate.
- Clyde's only outbound calls are to Anthropic: `api.anthropic.com` (usage +
  profile) and `platform.claude.com` (OAuth token exchange / refresh). Browser
  sign-in opens `claude.com` in *your* browser, not from Clyde. There is no
  telemetry, no Clyde server, and no listening socket.

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
