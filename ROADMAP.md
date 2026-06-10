# Roadmap

### v0.1 — macOS MVP (current)
- [x] Local streaming auth proxy (header injection + SSE passthrough)
- [x] Per-account OAuth token storage (Keychain) + refresh
- [x] Live `5h` / `7d` utilization from unified rate-limit headers
- [x] Auto routing with stickiness + pre-emptive switching
- [x] 429 transparent failover within a request
- [x] One-click `~/.claude/settings.json` integration (with restore)
- [x] Tray app + dashboard UI (add / pin / rename / remove accounts)
- [ ] First-run proxy health check + clear error states
- [ ] Desktop notifications on switch / limit
- [ ] "Launch at login" toggle in the UI
- [ ] Signed + notarized `.dmg`

### v0.2 — hardening
- [ ] Import existing logins directly from the Claude Code keychain entry
- [ ] Per-account spend / request counters
- [ ] Configurable switch threshold and routing strategy
- [ ] Robust handling of token-refresh / revoked-account states
- [ ] Auto-updater (GitHub Releases)

### v0.3 — Windows + Linux
- [ ] Windows build (Credential Manager, `%USERPROFILE%\.claude`, WebView2)
- [ ] Linux build (Secret Service)
- [ ] CI matrix producing signed installers for all three

### Known risks / open questions
- The OAuth `client_id` and endpoints mirror Claude Code's public flow and are
  not a stable API; they can change. Make them update-able without a release.
- The `apiKeyHelper`-bypasses-host-guard behavior is an implementation detail of
  Claude Code that could change between versions. Track it.
- ToS posture (see README). Consider an explicit in-app acknowledgement.
