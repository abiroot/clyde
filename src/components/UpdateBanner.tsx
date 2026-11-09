import { useState } from "react";
import { ChevronDown, RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import type { UpdateInfo } from "../lib/types";

/**
 * "Clyde 1.2.0 is available" with an Install button, and the release notes
 * behind "What's new". Installing downloads the signed release, verifies it,
 * swaps the app and relaunches — so this is the last thing the user sees of
 * the old version.
 */
export function UpdateBanner({ update, compact = false }: { update: UpdateInfo; compact?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const notes = update.notes?.trim() || null;

  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.installUpdate();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div
      className={`flex flex-col gap-1.5 rounded-lg ${compact ? "px-2.5 py-1.5" : "px-3 py-2"}`}
      style={{ background: "color-mix(in srgb, var(--n-accent) 12%, transparent)" }}
    >
      <div className="flex items-center gap-2.5">
        <span className="min-w-0 flex-1 text-[12px]">
          <span className="font-medium">Clyde {update.version} is available</span>
          {!compact && <span className="n-dim"> · you have {update.current}</span>}
          {notes && (
            <button
              className="ml-1.5 inline-flex items-center gap-0.5 text-[12px] font-medium"
              style={{ color: "var(--n-accent)" }}
              aria-expanded={open}
              onClick={() => setOpen((o) => !o)}
            >
              What's new
              <ChevronDown size={12} style={{ transform: open ? "rotate(180deg)" : undefined }} />
            </button>
          )}
          {error && (
            <span className="block" style={{ color: "var(--n-danger)" }}>
              {error}
            </span>
          )}
        </span>
        <button className="n-button flex items-center gap-1.5" disabled={busy} onClick={install}>
          {busy && <RefreshCw size={11} className="animate-spin" />}
          {busy ? "Installing…" : "Install & restart"}
        </button>
      </div>
      {open && notes && <ReleaseNotes text={notes} compact={compact} />}
    </div>
  );
}

/** The release notes (light Markdown: headings, `- ` bullets, **bold**, `code`) as text. */
function ReleaseNotes({ text, compact }: { text: string; compact: boolean }) {
  const inline = (s: string) => s.replace(/\*\*(.+?)\*\*/g, "$1").replace(/`([^`]+)`/g, "$1");
  return (
    <div className={`flex flex-col gap-1 overflow-y-auto text-[12px] leading-snug ${compact ? "max-h-40" : "max-h-64"}`}>
      {text.split("\n").map((raw, i) => {
        const line = raw.trimEnd();
        if (!line.trim()) return null;
        const heading = line.match(/^#{1,6}\s+(.*)$/);
        if (heading) return <p key={i} className="pt-1 font-semibold">{inline(heading[1])}</p>;
        const bullet = line.match(/^\s*[-*]\s+(.*)$/);
        if (bullet)
          return (
            <p key={i} className="flex gap-1.5">
              <span className="n-dim">•</span>
              <span>{inline(bullet[1])}</span>
            </p>
          );
        return <p key={i}>{inline(line)}</p>;
      })}
    </div>
  );
}
