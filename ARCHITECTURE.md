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

> **Trade-off:** Claude Code caches its OAuth token in memory at startup and only
> re-reads the keychain when that token expires. So switching takes effect on the
> **next `claude` run** (or once a running session's token refreshes), not
> instantly mid-session. An earlier design used a localhost proxy to switch
> per-request; it was removed in favor of this simpler, proxy-free approach.

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
| `commands.rs` | Tauri commands exposed to the UI (the only Rust↔JS surface). |
| `lib.rs` | App wiring: plugins, tray, window behavior, the usage-poll loop. |

## Switching an account

`claude_sync::activate` is the heart of it. Given an account, it:

1. reads the existing `Claude Code-credentials` keychain blob and replaces the
   `claudeAiOauth` object with this account's `accessToken` / `refreshToken` /
   `expiresAt` / `scopes`, writing *this* account's own plan metadata
   (`subscriptionType`, `rateLimitTier`, `isMax`, captured at import) and leaving
   any other top-level keys (e.g. `mcpOAuth`) untouched;
2. updates `~/.claude/.claude.json` → `oauthAccount` so Claude Code shows the right
   identity.

`engine::Core::set_active` wraps this: it first refreshes the account's token if
stale (so Claude Code gets a non-expired bearer), calls `activate`, then records
the new active id and pushes a fresh snapshot to the UI. There is no automatic
routing or failover — the user picks the active account; the gauges exist to inform
that choice.

Clyde always targets the user's *default* `claude` (the `~/.claude` config dir /
unsuffixed keychain service), regardless of any `CLAUDE_CONFIG_DIR` the Clyde
process itself may have inherited.

On startup, `detect_active` reconciles Clyde's idea of the active account with
whichever one Claude Code's keychain actually holds (matched by token), and
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

Before polling, it first pulls the active account's current credential back out of
Claude Code's keychain (`sync_active_credential_from_keychain`) — because Claude
Code rotates that token on its own, and Clyde must not fight it over the refresh
token.

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

Each account's `refresh_token` lives in Clyde's keychain vault. `Core::valid_bearer`
refreshes an access token when it's within `REFRESH_SKEW_MS` (60s) of expiry,
serializes concurrent refreshes behind a `tokio::Mutex`, and persists the rotated
credential. The UI never sees tokens — only `AccountView` DTOs, pushed over the
`clyde://update` Tauri event.
