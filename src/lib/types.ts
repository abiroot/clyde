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

export interface SwitchOutcome {
  snapshot: AppSnapshot;
  /**
   * Running `claude` processes at switch time. They adopt the new account
   * within ~30 s; a connected Chrome browser bridge drops on the account
   * change and needs `/chrome` → Reconnect extension (or a restart).
   */
  running_sessions: number;
}

/** One browser profile carrying the Claude in Chrome extension. */
export interface ChromeBrowser {
  /** Display name, e.g. `"Chrome"` or `"Chrome — Profile 2"`. */
  name: string;
  /** The account uuid the extension last authenticated as, if readable. */
  account_uuid: string | null;
  /** Email, when that uuid matches an account Clyde manages. */
  email: string | null;
  /** Clyde's account id, when it manages this account. */
  account_id: string | null;
  matches_active: boolean;
}

/**
 * Whether Claude Code's browser tools can reach Chrome. The extension and Claude
 * Code find each other through a rendezvous keyed on account uuid, so they must
 * be on the *same* account — switching in Clyde leaves the browser behind.
 */
export interface ChromeLink {
  installed: boolean;
  browsers: ChromeBrowser[];
  /** True when some browser is on the active account — `/chrome` should work. */
  matched: boolean;
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
