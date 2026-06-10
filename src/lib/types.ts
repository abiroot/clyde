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

export interface AppSnapshot {
  accounts: AccountView[];
  /** The account Clyde has made active in Claude Code's credential store. */
  active_id: string | null;
  /** Email of the active account, for the title bar. */
  active_email: string | null;
}

export interface LoginStart {
  flow_id: string;
  authorize_url: string;
}

export interface Discovered {
  id: string;
  config_dir: string;
  label: string;
  email: string | null;
  subscription_type: string | null;
}
