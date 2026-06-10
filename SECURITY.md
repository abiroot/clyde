# Security

## Threat model & guarantees

- **Credentials at rest:** stored only in the OS-native secret store (macOS
  Keychain / Windows Credential Manager / Linux Secret Service) via the
  `keyring` crate. No tokens are written to plaintext files or logs.
- **Network surface:** the proxy binds to `127.0.0.1` only. Clyde sends requests
  to exactly one external host — `api.anthropic.com` (overridable via
  `CLYDE_UPSTREAM` for testing). There is no Clyde backend and no telemetry.
- **The `settings.json` integration** adds two keys and backs up their prior
  values to `~/.claude/.clyde-integration-backup.json`, restored on disconnect.

## Reporting a vulnerability

Please open a private security advisory on GitHub, or email the maintainers
listed in the repo. Do not file public issues for security problems.

Include: affected version, reproduction steps, and impact. We aim to acknowledge
within a few days.

## Notes

The OAuth client id / endpoints used are public values mirroring Claude Code's
flow; they are not secrets but are not a stable API either.
