# Security

## Threat model & guarantees

- **Credentials at rest:** stored only in the OS-native secret store (macOS
  Keychain / Windows Credential Manager / Linux Secret Service) via the
  `keyring` crate. No tokens are written to plaintext files or logs.
- **Network surface:** Clyde opens no listening socket. Its outbound requests go
  only to Anthropic: `api.anthropic.com` (usage + profile; overridable via
  `CLYDE_UPSTREAM` for testing) and `platform.claude.com` (OAuth token exchange /
  refresh). Browser sign-in opens `claude.com` in *your* default browser — Clyde
  never handles your password. There is no Clyde backend and no telemetry.
- **Claude Code's store:** switching an account rewrites the `Claude Code-credentials`
  keychain entry and `~/.claude/.claude.json` in place, preserving all other keys.
  Clyde also self-heals by removing any dead proxy integration an older version left
  in `~/.claude/settings.json`.

## Reporting a vulnerability

Please open a private [security advisory](https://github.com/Abiroot/clyde/security/advisories/new)
on GitHub. Do not file public issues for security problems.

Include: affected version, reproduction steps, and impact. We aim to acknowledge
within a few days.

## Notes

The OAuth client id / endpoints used are public values mirroring Claude Code's
flow; they are not secrets but are not a stable API either.
