// Mirrors the Rust types in `src-tauri/src/model.rs`.

export interface UsageSnapshot {
  five_hour_utilization: number | null;
  seven_day_utilization: number | null;
  status: string | null;
  resets_at: number | null;
  five_hour_resets_at: number | null;
  seven_day_resets_at: number | null;
  updated_at: number;
  /** Per-model / per-product caps, e.g. "7-day · Fable". */
  scoped_limits: UsageLimit[];
}

export interface UsageLimit {
  label: string;
  percent: number;
  severity: string | null;
  resets_at: number | null;
}

export interface AccountView {
  id: string;
  label: string;
  email: string | null;
  subscription_type: string | null;
  usage: UsageSnapshot;
  is_active: boolean;
  /** Why the last usage read failed, when it did. */
  usage_error: string | null;
  forecasts: Forecast[];
}

/** When a limit runs out at the current pace. */
export interface Forecast {
  label: string;
  per_hour: number;
  minutes_to_full: number | null;
}

/** One history sample: each limit's percentage at time `t` (ms). */
export interface HistoryPoint {
  t: number;
  account: string;
  limits: Record<string, number>;
}

export interface Session {
  pid: number;
  cwd: string | null;
  running_secs: number | null;
  config_dir: string | null;
}

export interface Settings {
  alerts_enabled: boolean;
  alert_thresholds: number[];
  notify_on_reset: boolean;
  alert_all_accounts: boolean;
  menubar_readout: boolean;
  /** Empty string = no shortcut. */
  shortcut: string;
  auto_switch: AutoSwitchSettings;
  check_updates: boolean;
}

export interface AppSnapshot {
  accounts: AccountView[];
  /** The account Clyde has made active in Claude Code's credential store. */
  active_id: string | null;
  /** Email of the active account, for the title bar. */
  active_email: string | null;
  auto_switch_enabled: boolean;
  /** The most recent automatic switch. */
  last_switch: SwitchEvent | null;
  /** A newer Clyde release, when one is out. */
  update: UpdateInfo | null;
}

export interface SwitchOutcome {
  snapshot: AppSnapshot;
  /**
   * Running `claude` processes at switch time. They adopt the new account
   * within ~30 s; Claude Code's own Chrome bridge drops on the account change
   * (the Playwright browser tools don't).
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

/** One Chrome profile, as the browser-tools setup lists it. */
export interface ChromeProfile {
  /** Directory name, e.g. "Default" or "Profile 2". */
  dir: string;
  name: string;
  /** The Google account signed into it. */
  email: string | null;
  /** The Playwright Extension is installed in it. */
  has_extension: boolean;
  /** Clyde could read the extension's connection token from it. */
  token_found: boolean;
}

/**
 * Account-independent browser tools (Playwright MCP + the Playwright
 * Extension). The extension accepts a local connection authenticated by a
 * per-profile token, so switching accounts can't break it.
 */
export interface BrowserStatus {
  node_found: boolean;
  profiles: ChromeProfile[];
  /** The profile the registered server connects to. */
  profile: string | null;
  mcp_registered: boolean;
  token_set: boolean;
  /** The extension was reinstalled since setup; its token changed. */
  token_stale: boolean;
  registered_version: string | null;
  pinned_version: string;
  /** Claude Code's own account-bound Chrome integration is on. */
  builtin_enabled: boolean;
  /** Clyde 1.0's Open Claude in Chrome is still wired up. */
  legacy_present: boolean;
  ready: boolean;
}

export interface BrowserTest {
  ok: boolean;
  detail: string;
}

/** When an account counts as used up. */
export interface Rule {
  /** "any" | "session" | "weekly" | a model cap's label, e.g. "7-day · Fable". */
  limit: string;
  /** 1..100; 100 = only when full. */
  at: number;
}

export interface AccountPolicy {
  /** Never switch to this account automatically. */
  exclude: boolean;
  /** Replaces the default rule. */
  rule: Rule | null;
}

export interface AutoSwitchSettings {
  enabled: boolean;
  default_rule: Rule;
  /** Account ids, most preferred first. */
  priority: string[];
  accounts: Record<string, AccountPolicy>;
  switch_back: boolean;
  notify: boolean;
}

export interface SwitchEvent {
  from: string;
  to: string;
  reason: string;
  /** Unix ms. */
  at: number;
}

export interface UpdateInfo {
  version: string;
  current: string;
  notes: string | null;
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
