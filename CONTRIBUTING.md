# Contributing to Clyde

Thanks for your interest! Clyde is early — issues, ideas, and PRs are welcome.

## Project layout

```
src/                 React + TypeScript UI (Vite, Tailwind v4)
  components/        UI building blocks
  views/             Dashboard / Onboarding
  lib/               typed Tauri bridge, hooks, helpers
src-tauri/src/       Rust core
  engine.rs          state + active selection + usage polling
  claude_sync.rs     writes the active account into Claude Code's store
  import_claude.rs   import existing Claude Code logins
  oauth.rs           PKCE browser login, profile lookup, token refresh
  usage.rs           parse GET /api/oauth/usage
  vault.rs           Clyde's own account list in the OS keychain
```

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design.

## Dev setup

```bash
npm install
npm run tauri dev
```

You'll need Rust (stable) and Node 20+.

## Before you open a PR

```bash
npm run build                         # typecheck + bundle the UI
cargo fmt   --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test  --manifest-path src-tauri/Cargo.toml
```

## Guidelines

- Keep the UI **calm and self-explanatory** — no jargon in the happy path.
- Never log or persist tokens outside the keychain.
- Switching rewrites Claude Code's own keychain entry + `.claude.json` in place —
  preserve any keys you don't own, and always target the default `~/.claude`.
- Match the surrounding code style; prefer small, reviewable PRs.

## Scope & conduct

Be respectful. By contributing you agree your work is licensed under the
project's [MIT license](LICENSE). Please keep discussions mindful of the
responsible-use note in the README.
