import { invoke } from "@tauri-apps/api/core";
import type {
  AppSnapshot,
  ChromeLink,
  Discovered,
  HistoryPoint,
  LoginStart,
  OpenChromeStatus,
  Session,
  Settings,
  SwitchOutcome,
} from "./types";

export const api = {
  getSnapshot: () => invoke<AppSnapshot>("get_snapshot"),

  getChromeLink: () => invoke<ChromeLink>("get_chrome_link"),

  getOpenChrome: () => invoke<OpenChromeStatus>("get_open_chrome"),

  getSettings: () => invoke<Settings>("get_settings"),

  setSettings: (settings: Settings) => invoke<Settings>("set_settings", { settings }),

  getHistory: (accountId: string, hours: number) =>
    invoke<HistoryPoint[]>("get_history", { accountId, hours }),

  listSessions: () => invoke<Session[]>("list_sessions"),

  testNotification: () => invoke<void>("test_notification"),

  getAutostart: () => invoke<boolean>("get_autostart"),

  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),

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
