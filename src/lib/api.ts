import { invoke } from "@tauri-apps/api/core";
import type {
  AppSnapshot,
  ChromeLink,
  Discovered,
  HistoryPoint,
  LoginStart,
  BrowserStatus,
  BrowserTest,
  UpdateInfo,
  Session,
  Settings,
  SwitchOutcome,
} from "./types";

export const api = {
  getSnapshot: () => invoke<AppSnapshot>("get_snapshot"),

  getChromeLink: () => invoke<ChromeLink>("get_chrome_link"),

  getBrowser: () => invoke<BrowserStatus>("get_browser"),

  browserOpenStore: (profile: string) => invoke<void>("browser_open_store", { profile }),

  browserSetup: (profile: string, token: string | null) =>
    invoke<BrowserStatus>("browser_setup", { profile, token }),

  browserRemove: () => invoke<BrowserStatus>("browser_remove"),

  browserRemoveLegacy: () => invoke<BrowserStatus>("browser_remove_legacy"),

  browserTest: () => invoke<BrowserTest>("browser_test"),

  setBuiltinChrome: (enabled: boolean) =>
    invoke<BrowserStatus>("set_builtin_chrome", { enabled }),

  openChromeExtensionsPage: (profile: string) =>
    invoke<void>("open_chrome_extensions_page", { profile }),

  checkForUpdate: () => invoke<UpdateInfo | null>("check_for_update"),

  installUpdate: () => invoke<void>("install_update"),

  copyText: (text: string) => invoke<void>("copy_text", { text }),

  getSettings: () => invoke<Settings>("get_settings"),

  setSettings: (settings: Settings) => invoke<Settings>("set_settings", { settings }),

  getHistory: (accountId: string, hours: number) =>
    invoke<HistoryPoint[]>("get_history", { accountId, hours }),

  listSessions: () => invoke<Session[]>("list_sessions"),

  testNotification: () => invoke<void>("test_notification"),

  getAutostart: () => invoke<boolean>("get_autostart"),

  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),

  discoverClaudeAccounts: () =>
    invoke<Discovered[]>("discover_claude_accounts"),

  importClaudeAccounts: (configDirs: string[]) =>
    invoke<AppSnapshot>("import_claude_accounts", { configDirs }),

  startClaudeLogin: () => invoke<string>("start_claude_login"),

  setActiveAccount: (id: string) =>
    invoke<SwitchOutcome>("set_active_account", { id }),

  connectKimi: (key: string) => invoke<AppSnapshot>("connect_kimi", { key }),

  renameAccount: (id: string, label: string) =>
    invoke<AppSnapshot>("rename_account", { id, label }),

  removeAccount: (id: string) => invoke<AppSnapshot>("remove_account", { id }),

  beginLogin: () => invoke<LoginStart>("begin_login"),

  completeLogin: (flowId: string, code: string, label: string) =>
    invoke<AppSnapshot>("complete_login", { flowId, code, label }),

  importToken: (label: string, tokenJson: string) =>
    invoke<AppSnapshot>("import_token", { label, tokenJson }),
};
