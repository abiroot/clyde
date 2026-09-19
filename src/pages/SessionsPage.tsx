import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { Session } from "../lib/types";
import { Group, GroupRow } from "../ui/kit";

/** Running `claude` sessions, refreshed every few seconds while visible. */
export function SessionsPage() {
  const [sessions, setSessions] = useState<Session[] | null>(null);

  useEffect(() => {
    let live = true;
    const load = () => api.listSessions().then((s) => live && setSessions(s));
    load();
    const t = setInterval(load, 4000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, []);

  const home = sessions?.find((s) => s.cwd)?.cwd?.match(/^\/Users\/[^/]+/)?.[0];
  const tidy = (p: string) => (home && p.startsWith(home) ? "~" + p.slice(home.length) : p);

  return (
    <div className="flex max-w-[640px] flex-col gap-6">
      <Group
        title={sessions ? `${sessions.length} running` : "Running"}
        footer="Sessions on the default config follow the active account within ~30 seconds of a switch. Sessions started with their own CLAUDE_CONFIG_DIR keep their own login."
      >
        {!sessions ? (
          <GroupRow>
            <span className="n-dim">Looking…</span>
          </GroupRow>
        ) : sessions.length === 0 ? (
          <GroupRow>
            <span className="n-dim">No claude sessions are running.</span>
          </GroupRow>
        ) : (
          sessions.map((s) => {
            const folder = s.cwd ? s.cwd.split("/").filter(Boolean).pop() ?? "/" : "Unknown folder";
            return (
              <GroupRow key={s.pid}>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px]">{folder}</div>
                  <div className="n-dim truncate text-[11px]">{s.cwd ? tidy(s.cwd) : `pid ${s.pid}`}</div>
                </div>
                {s.config_dir && (
                  <span className="rounded-md bg-[var(--n-track)] px-1.5 py-0.5 text-[11px] n-dim" title={s.config_dir}>
                    own login
                  </span>
                )}
                <span className="n-dim w-20 text-right text-[12px] tabular-nums">{uptime(s.running_secs)}</span>
              </GroupRow>
            );
          })
        )}
      </Group>
    </div>
  );
}

function uptime(secs: number | null): string {
  if (secs == null) return "";
  if (secs < 60) return "just now";
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h ${m % 60}m` : `${Math.floor(h / 24)}d ${h % 24}h`;
}
