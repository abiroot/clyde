# Architecture

Clyde is a Tauri 2 app: a Rust core (the proxy + engine) with a React/TypeScript
UI. The Rust side is the interesting part; the UI is a thin client over it.

## The problem it solves

Claude Code caches its OAuth token in memory at startup and only re-reads the
keychain when that token expires. So you cannot switch the *active* account of a
running session by swapping the keychain entry — the change won't be picked up.

The only per-request seams Claude Code exposes are `apiKeyHelper` /
`proxyAuthHelper`, but `apiKeyHelper` output is sent as `x-api-key`, which can't
carry a subscription OAuth bearer (those must go in `Authorization: Bearer` with
the `anthropic-beta: oauth-…` header). Therefore the only way to switch accounts
mid-session is to intercept the HTTP request and rewrite the auth — a proxy.

A useful side effect: `apiKeyHelper` is also the one auth source that bypasses
Claude Code's "managed host" guard, which otherwise refuses to send subscription
credentials to a non-Anthropic host. So Clyde sets a throwaway `apiKeyHelper`
purely to unlock pointing `ANTHROPIC_BASE_URL` at localhost.

## Rust modules (`src-tauri/src`)

| Module | Responsibility |
|---|---|
| `model.rs` | Shared types: `Account`, `Credential`, `UsageSnapshot`, `Mode`, view DTOs. |
| `vault.rs` | Keychain-backed persistence of accounts (`keyring` crate). |
| `oauth.rs` | PKCE login + token refresh against the Claude OAuth endpoints. |
| `ratelimit.rs` | Parse `anthropic-ratelimit-unified-*` headers → `UsageSnapshot`. |
| `engine.rs` | The `Core`: in-memory state, account routing, token-refresh machinery, UI event emission. |
| `proxy.rs` | The axum server: per-request account selection, header injection, streaming passthrough, 429 failover. |
| `claude_config.rs` | Enable/disable the `~/.claude/settings.json` integration (with backup/restore). |
| `commands.rs` | Tauri commands exposed to the UI. |
| `lib.rs` | App wiring: plugins, tray, window behavior, proxy startup. |

## Routing

`engine::Core::resolve_active` chooses the account for the next request:

- **Pinned mode** → always that account, no failover.
- **Auto mode** → prefer accounts that aren't hard-limited, keep the current one
  while its utilization is below `SWITCH_THRESHOLD` (stickiness, avoids
  flapping), otherwise pick the least-pressured account. On a `429`, the proxy
  marks the account limited and retries on the next one within the same request.

## Why streaming matters

Claude responses are SSE token streams. The proxy uses
`axum::body::Body::from_stream(reqwest_response.bytes_stream())` to pass bytes
through without buffering, so token latency is unaffected. Rate limits are
enforced by Anthropic at request *admission* (a `429` before the stream starts),
so failover is always clean — never mid-stream.

## Token lifecycle

Each account's `refresh_token` lives in the keychain. `Core::valid_bearer`
refreshes an access token when it's within `REFRESH_SKEW_MS` of expiry, serializes
concurrent refreshes behind a lock, and persists the rotated credential. The UI
never sees tokens — only `AccountView` DTOs.
