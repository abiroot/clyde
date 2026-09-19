import { invoke } from "@tauri-apps/api/core";
import type {
  AppSnapshot,
  ChromeLink,
  Discovered,
  LoginStart,
  OpenChromeStatus,
  SwitchOutcome,
} from "./types";

export const api = {
  getSnapshot: () => invoke<AppSnapshot>("get_snapshot"),

  getChromeLink: () => invoke<ChromeLink>("get_chrome_link"),

  getOpenChrome: () => invoke<OpenChromeStatus>("get_open_chrome"),

  setupOpenChrome: () => invoke<OpenChromeStatus>("setup_open_chrome"),

  removeOpenChrome: () => invoke<OpenChromeStatus>("remove_open_chrome"),

  setBuiltinChrome: (enabled: boolean) =>
    invoke<OpenChromeStatus>("set_builtin_chrome", { enabled }),

  revealOpenChromeExtension: () => invoke<void>("reveal_open_chrome_extension"),

  openChromeExtensionsPage: () => invoke<void>("open_chrome_extensions_page"),

  discoverClaudeAccounts: () =>
    invoke<Discovered[]>("discover_claude_accounts"),

  importClaudeAccounts: (configDirs: string[]) =>
    invoke<AppSnapshot>("import_claude_accounts", { configDirs }),

  startClaudeLogin: () => invoke<string>("start_claude_login"),

  setActiveAccount: (id: string) =>
    invoke<SwitchOutcome>("set_active_account", { id }),

  renameAccount: (id: string, label: string) =>
    invoke<AppSnapshot>("rename_account", { id, label }),

  removeAccount: (id: string) => invoke<AppSnapshot>("remove_account", { id }),

  beginLogin: () => invoke<LoginStart>("begin_login"),

  completeLogin: (flowId: string, code: string, label: string) =>
    invoke<AppSnapshot>("complete_login", { flowId, code, label }),

  importToken: (label: string, tokenJson: string) =>
    invoke<AppSnapshot>("import_token", { label, tokenJson }),
};
