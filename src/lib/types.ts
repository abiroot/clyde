// Mirrors the Rust types in `src-tauri/src/model.rs`.

export interface UsageSnapshot {
  five_hour_utilization: number | null;
  seven_day_utilization: number | null;
  status: string | null;
  resets_at: number | null;
  updated_at: number;
}

export interface AccountView {
  id: string;
  label: string;
  email: string | null;
  subscription_type: string | null;
  usage: UsageSnapshot;
  is_active: boolean;
}

export type Mode = { kind: "auto" } | { kind: "pinned"; accountId: string };

export interface AppSnapshot {
  accounts: AccountView[];
  mode: Mode;
  active_id: string | null;
  proxy_port: number;
  proxy_running: boolean;
  integration_enabled: boolean;
}

export interface LoginStart {
  flow_id: string;
  authorize_url: string;
}
