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
[Playwright MCP](https://github.com/microsoft/playwright-mcp) with Microsoft's
Playwright Extension, which reaches Chrome over a local connection with no
account attached, so browser tools keep working through every switch. You
install the extension from the Chrome Web Store; Clyde reads its connection
token from Chrome and registers the server with Claude Code. See
[Browser tools](#browser-tools) below.

**Auto-switch (optional).** Clyde can move Claude Code to another account by
itself when the active one reaches a limit — and back once your preferred
account resets. See [Auto-switch](#auto-switch) below.

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
   same account — Clyde flags it when they drift apart — or set up the
   account-independent browser tools below.) Or let [Auto-switch](#auto-switch)
   do the switching.

## Auto-switch

Off by default; turn it on in the **Auto-switch** page.

- **Rules.** A rule says which limit to watch (any limit, the 5-hour session,
  the weekly limit, or a per-model cap such as Fable's weekly one) and at what
  percentage to move on; 100% means "only when it's full". One default rule
  applies to every account, and any account can have its own instead.
- **Priority.** Drag accounts into the order you want them used. Clyde always
  uses the highest account that still has room: when the active account trips
  its rule, it moves to the next one; when a preferred account resets, it moves
  back (switch-back can be turned off). An account needs 5 points of room under
  its threshold before Clyde moves to it, so a limit hovering at the line can't
  make it flap.
- **Never switch to.** Excluded accounts are skipped. You can still pick one by
  hand; Clyde then stays on it until it trips.
- **Your choice wins.** After you switch by hand (in Clyde, or `claude /login`),
  Clyde won't switch back from that account until it runs out.
- A switch is announced with a notification. When no account has room, Clyde
  says so once and waits. Usage is checked every 2 minutes; running `claude`
  sessions follow a switch within ~30 seconds.

## Browser tools

Optional. Open **Browser tools** in the sidebar. Requires Node.js (`npx`).

1. **Pick a Chrome profile** — one that isn't signed into banking, crypto or
   payment sites: the agent can use whatever that profile is logged into.
2. **Install the Playwright Extension** — Clyde opens its Web Store page in that
   profile.
3. **Connect** — Clyde reads the extension's connection token from the profile's
   storage (or takes a pasted one) and registers a user-scope MCP server named
   `playwright` (`npx @playwright/mcp@<pinned> --extension --profile-dir-name …`).

New `claude` sessions then have `mcp__playwright__*` tools, whichever account is
active. **Test connection** starts the registered server and has it list
Chrome's tabs. Each session gets its own Chrome tab group and only sees the tabs
in it; Chrome shows a "started debugging this browser" bar while one is
connected. Clyde 1.0's Open Claude in Chrome is cleaned up automatically on
setup (remove its unpacked extension from `chrome://extensions` yourself).

## Updates

Clyde checks GitHub for a new release at launch and every 6 hours, notifies
you once per version, and shows **Install & restart** in the window and the
menubar popover. Updates are signed; Clyde verifies the signature against the
public key it ships with before replacing itself. Turn checks off in Settings.

## Security

- Browser tools (optional) give agents the Chrome profile you connect, with
  every site it's signed into — see [Browser tools](#browser-tools).

- Credentials live only in the OS-native secret store (macOS Keychain / Windows
  Credential Manager / Linux Secret Service) via the `keyring` crate.
- Clyde's only outbound calls are to Anthropic: `api.anthropic.com` (usage +
  profile) and `platform.claude.com` (OAuth token exchange / refresh). Browser
  sign-in opens `claude.com` in *your* browser, not from Clyde. Update checks
  read `latest.json` from this repo's GitHub releases. **Test connection** runs
  `npx @playwright/mcp`, which npm fetches on first use. There is no telemetry,
  no Clyde server, and no listening socket.

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
