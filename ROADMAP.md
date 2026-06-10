# Roadmap

### v0.1 — macOS MVP (current)
- [x] One-click account switch by writing Claude Code's own keychain + `.claude.json`
- [x] Per-account OAuth token storage (Keychain) + refresh
- [x] Live `5h` / `7d` utilization from `GET /api/oauth/usage` (no quota cost)
- [x] Add accounts four ways: detected import, browser sign-in, terminal `/login`, token paste
- [x] Browser/token accounts resolve email + plan from `GET /api/oauth/profile`
- [x] Self-heal: strip stale legacy proxy integration from `settings.json`
- [x] Tray app + dashboard UI (add / rename / remove / select accounts)
- [ ] First-run health check + clear error states
- [ ] Desktop notifications on limit
- [ ] "Launch at login" toggle in the UI
- [ ] Signed + notarized `.dmg`

### v0.2 — hardening
- [ ] Per-account spend / request counters
- [ ] Optional usage-aware switch suggestions
- [ ] Robust handling of token-refresh / revoked-account states
- [ ] Auto-updater (GitHub Releases)

### v0.3 — Windows + Linux
- [ ] Windows build (Credential Manager, `%USERPROFILE%\.claude`, WebView2)
- [ ] Linux build (Secret Service)
- [ ] CI matrix producing signed installers for all three

### Known risks / open questions
- The OAuth `client_id` and endpoints mirror Claude Code's public flow and are
  not a stable API; they can change. Make them update-able without a release.
- The keychain item name and `.claude.json` shape are Claude Code internals that
  could change between versions. Track them.
- A switch only takes effect on the next `claude` run, since Claude Code caches its
  token in memory. Consider surfacing this clearly in the UI.
- ToS posture (see README). Consider an explicit in-app acknowledgement.
