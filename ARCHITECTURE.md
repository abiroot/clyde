# Architecture

Clyde is a Tauri 2 app: a Rust core (the engine) with a React/TypeScript UI. The
Rust side is the interesting part; the UI is a thin client over it.

## The problem it solves

If you have more than one Claude subscription, switching the *active* account that
plain `claude` uses normally means juggling separate config dirs or re-logging in,
and the configs drift apart.

Claude Code reads its subscription token from one place: the OS keychain item for
its config dir (for the default `~/.claude`, that's the macOS Keychain entry
`Claude Code-credentials`), plus the displayed identity in `~/.claude/.claude.json`
→ `oauthAccount`. So to make `claude` run as a different account, you rewrite those
two things to hold that account's OAuth — in place, preserving everything else.

That is exactly what Clyde does. There is **no proxy** and **no `settings.json`
edit**: Clyde writes the chosen account's credential straight into Claude Code's own
store, and plain `claude` then talks directly to `api.anthropic.com` as that
account — whether or not Clyde is running.

> **Trade-off:** Claude Code reads the keychain credential through a ~30-second
> cache (verified against Claude Code 2.1.222's secure-storage layer), so a
> switch reaches **running sessions within ~30 s** and new runs immediately.
> Two consequences: a long-running session silently continues *as the new
> account* (its quota, its billing), and the session's Claude-in-Chrome browser
> bridge — which re-derives the account identity on every (re)connect — drops
> when the identity changes under it and does not re-pair on its own. The user
> recovers it with `/chrome` → "Reconnect extension", or by restarting the
> session; `set_active_account` reports how many `claude` processes were running
> so the UI can say exactly that. An earlier design used a localhost proxy to
> switch per-request; it was removed in favor of this simpler, proxy-free
> approach.

## Rust modules (`src-tauri/src`)

| Module | Responsibility |
|---|---|
| `model.rs` | Shared types: `Account`, `Credential`, `UsageSnapshot`, and the secret-free view DTOs (`AccountView`, `AppSnapshot`). |
| `vault.rs` | Keychain-backed persistence of Clyde's *own* account list (`keyring` crate). |
| `oauth.rs` | PKCE browser login, profile lookup, and access-token refresh against the Claude OAuth endpoints. |
| `usage.rs` | Parse the `GET /api/oauth/usage` JSON → `UsageSnapshot`. |
| `engine.rs` | The `Core`: in-memory state, the active selection, usage polling, token-refresh machinery, and UI event emission. |
| `claude_sync.rs` | Make a Clyde account the active Claude Code account by rewriting its keychain entry + `.claude.json`. Also self-heals stale legacy proxy integrations. |
| `import_claude.rs` | Discover and import existing logins from Claude Code's own config dirs / keychain entries. |
| `chrome_link.rs` | Read which claude.ai account the Claude-in-Chrome extension is signed into, so the UI can explain why `/chrome` stopped working after a switch. |
| `commands.rs` | Tauri commands exposed to the UI (the only Rust↔JS surface). |
| `lib.rs` | App wiring: plugins, tray, window behavior, the usage-poll loop. |

## Switching an account

`claude_sync::activate` is the heart of it. Given an account, it:

1. reads the existing `Claude Code-credentials` keychain blob and replaces the
   `claudeAiOauth` object with this account's `accessToken` / `refreshToken` /
   `expiresAt` / `scopes`, writing *this* account's own plan metadata
   (`subscriptionType`, `rateLimitTier`, `isMax`, captured at import) and leaving
   any other top-level keys (e.g. `mcpOAuth`) untouched;
2. updates Claude Code's global config → `oauthAccount` so Claude Code shows the
   right identity.

For (2), Clyde resolves the config file the way Claude Code does (verified
against 2.1.227): `~/.claude/.config.json` when that exists, otherwise
`~/.claude.json`. Earlier versions wrote `~/.claude/.claude.json`, which Claude
Code does not read unless `CLAUDE_CONFIG_DIR` points at `~/.claude` — so the
identity silently went stale while the keychain (the part that actually
authenticates) switched correctly. That file is still updated when it already
exists, so the two can't disagree. The block written includes `accountUuid`,
not just the email: Claude Code compares it against the account its live token
resolves to, and a stale one is what surfaces as "belongs to a different
claude.ai account" on the Chrome bridge.

## Chrome and account switching

Claude Code's browser tools reach the Claude-in-Chrome extension through
`wss://bridge.claudeusercontent.com`, a rendezvous **keyed on account uuid**:
Claude Code joins the room its live token resolves to, the extension joins the
room its own stored `accountUuid` names. Switching accounts moves Claude Code to
a different room and leaves the browser behind, so `/chrome` reports the
extension as not connected.

Clyde cannot move the browser for you. The extension keeps its OAuth tokens in
`chrome.storage.session` (memory-only) and accepts a sign-in only from a
claude.ai page. What it can do is name the problem: `chrome_link.rs` reads the
`accountUuid` the extension persists in its `chrome.storage.local` LevelDB and
compares it to the active account. When Clyde also manages the account the
browser is on, the fix costs one click and no browser interaction — switch Clyde
to it.

`engine::Core::set_active` wraps this: it first refreshes the account's token if
stale (so Claude Code gets a non-expired bearer), calls `activate`, then records
the new active id and pushes a fresh snapshot to the UI. There is no automatic
routing or failover — the user picks the active account; the gauges exist to inform
that choice.

Clyde always targets the user's *default* `claude` (the `~/.claude` config dir /
unsuffixed keychain service), regardless of any `CLAUDE_CONFIG_DIR` the Clyde
process itself may have inherited.

On startup, `detect_active` reconciles Clyde's idea of the active account with
whichever one Claude Code's keychain actually holds (matched by token lineage,
falling back to the identity in `~/.claude/.claude.json`; if neither matches a
stored account, none is claimed active), and
`cleanup_legacy_integration` strips any dead proxy keys (a throwaway `apiKeyHelper`
and a localhost `ANTHROPIC_BASE_URL`) that an older Clyde version may have left in
`settings.json`.

## Live usage

So the gauges fill even when no traffic is flowing, `Core::poll_usage` runs on a
120-second loop (`lib.rs`). For each account it calls `GET /api/oauth/usage` — the
same endpoint Claude Code's own status line uses — and parses the JSON
(`{ five_hour, seven_day, ... }`, each `{ utilization 0-100, resets_at }`) into a
`UsageSnapshot`. This sends no messages, so it costs no quota; an earlier version
read rate-limit headers off a throwaway `max_tokens: 1` request, which did.

Before polling, it first reconciles the keychain slot with the vault
(`adopt_active_slot`) — because Claude Code rotates that token on its own, and
Clyde must not fight it over the refresh token. The reconcile is bidirectional
and identity-checked: whichever side holds the newer token generation wins, and
a credential is only ever adopted into the account it provably belongs to.

## Adding accounts

`import_claude` covers the on-machine paths (discover + import from existing config
dirs / keychain entries; or spawn Claude Code's own `/login` in an isolated
profile). `oauth` covers the browser path: a PKCE flow that mirrors Claude Code's
subscription login exactly — these details are load-bearing and were reverse-
engineered from the binary:

- authorize at **`https://claude.com/cai/oauth/authorize`** (not `claude.ai`),
  redirect `https://platform.claude.com/oauth/code/callback`, token exchange at
  `https://platform.claude.com/v1/oauth/token`;
- the query is **form-encoded** (`reqwest::Url::parse_with_params`), the full
  6-scope set is requested, and the PKCE **`state` is 32 bytes** — a short state,
  raw-colon encoding, the wrong host, or a scope subset each makes the grant fail
  with a generic "Invalid request format";
- since Claude's access tokens are opaque, an account's email + plan come from
  `GET /api/oauth/profile` after the token is obtained.

Browser- and token-added accounts key their id off the email, so they merge with
the same account if it's later discovered on disk rather than duplicating.

## Token lifecycle

Each account's `refresh_token` lives in Clyde's keychain vault. **Anthropic's
refresh tokens are single-use**: every refresh consumes the old token and issues a
new pair, so whoever holds a stale generation is one refresh attempt away from a
forced logout. Everything below exists to keep all copies on the latest generation:

- `Core::valid_bearer` refreshes an access token when it's within
  `REFRESH_SKEW_MS` (60s) of expiry, serializes concurrent refreshes behind a
  `tokio::Mutex`, and persists the rotated credential. When the account owns the
  keychain slot, the rotation is **pushed back into the keychain**
  (`write_active_credential`, guarded so a `claude /login` the user did in the
  meantime is never clobbered) — otherwise the next `claude` run would try the
  consumed refresh token and log itself out.
- If a refresh of the active account fails, the keychain is re-read and the
  refresh retried once with the live generation — a running `claude` may have
  rotated it after Clyde last synced.
- `resync_credential_from_source` only adopts a source dir's credential when it
  is a strictly newer generation (`Credential::is_newer_than`); source keychains
  go stale the moment Clyde refreshes the account itself.

The UI never sees tokens — only `AccountView` DTOs, pushed over the
`clyde://update` Tauri event.
