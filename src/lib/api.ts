import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshot, LoginStart, Mode } from "./types";

export const api = {
  getSnapshot: () => invoke<AppSnapshot>("get_snapshot"),

  setMode: (mode: Mode) => invoke<AppSnapshot>("set_mode", { mode }),

  renameAccount: (id: string, label: string) =>
    invoke<AppSnapshot>("rename_account", { id, label }),

  removeAccount: (id: string) => invoke<AppSnapshot>("remove_account", { id }),

  beginLogin: () => invoke<LoginStart>("begin_login"),

  completeLogin: (flowId: string, code: string, label: string) =>
    invoke<AppSnapshot>("complete_login", { flowId, code, label }),

  importToken: (label: string, tokenJson: string) =>
    invoke<AppSnapshot>("import_token", { label, tokenJson }),

  enableIntegration: () => invoke<AppSnapshot>("enable_integration"),

  disableIntegration: () => invoke<AppSnapshot>("disable_integration"),
};
