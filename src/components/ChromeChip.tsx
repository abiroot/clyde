import { useEffect, useState } from "react";
import { Unlink } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../lib/api";
import type { AppSnapshot, ChromeLink } from "../lib/types";

interface Props {
  snapshot: AppSnapshot;
  /** Switch Clyde to the account the browser is already on. */
  onActivate: (id: string) => void;
  /** Open the setup for browser tools that work with every account. */
  onOpenBrowserTools: () => void;
  busyId: string | null;
}

/**
 * Tells the user whether Claude Code's browser tools can reach Chrome.
 *
 * The extension and Claude Code find each other through a rendezvous keyed on
 * account uuid, so they have to be on the same claude.ai account — switching in
 * Clyde silently leaves the browser behind and `/chrome` reports the extension
 * as not connected. Clyde can't move the browser itself (the extension holds its
 * tokens in memory and only accepts a sign-in from a claude.ai page), so this
 * offers the two fixes that do work — and leads with the one that needs no
 * browser interaction at all: switch Clyde to whatever account Chrome is on.
 *
 * Silent when everything already agrees, and when the extension isn't installed.
 */
export function ChromeChip({ snapshot, onActivate, onOpenBrowserTools, busyId }: Props) {
  const [link, setLink] = useState<ChromeLink | null>(null);

  // Re-read whenever the active account changes: that's exactly the event that
  // breaks the pairing.
  useEffect(() => {
    let live = true;
    api
      .getChromeLink()
      .then((l) => live && setLink(l))
      .catch(() => live && setLink(null));
    return () => {
      live = false;
    };
  }, [snapshot.active_id]);

  if (!link || !link.installed || link.matched) return null;

  // Any browser whose account Clyde also manages can be adopted in one click.
  const adoptable = link.browsers.find((b) => b.account_id !== null);
  const signedIn = link.browsers.filter((b) => b.account_uuid !== null);
  if (signedIn.length === 0) return null;

  const who =
    adoptable?.email ??
    signedIn[0].email ??
    `another account (${signedIn[0].account_uuid?.slice(0, 8)})`;
  const browser = (adoptable ?? signedIn[0]).name;

  return (
    <div className="mx-4 mt-1 mb-2 rounded-lg border border-[var(--color-warn)]/30 bg-[var(--color-warn)]/10 px-3 py-2 text-[11px] text-[var(--color-warn)]">
      <div className="flex items-start gap-2">
        <Unlink size={13} className="mt-[1px] shrink-0" />
        <span className="leading-snug">
          {browser} is signed into <strong>{who}</strong>, so Claude Code's
          browser tools (<code>/chrome</code>) can't reach it while a different
          account is active.
        </span>
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-3 pl-[21px]">
        {adoptable && (
          <button
            onClick={() => onActivate(adoptable.account_id!)}
            disabled={busyId !== null}
            className="no-drag font-medium underline underline-offset-2 hover:opacity-70 disabled:opacity-40"
          >
            Switch Clyde to {adoptable.email ?? "it"}
          </button>
        )}
        <button
          onClick={() => void openUrl("https://claude.ai/login")}
          className="no-drag font-medium underline underline-offset-2 hover:opacity-70"
        >
          Sign Chrome into {snapshot.active_email ?? "the active account"}
        </button>
        <button
          onClick={onOpenBrowserTools}
          className="no-drag font-medium underline underline-offset-2 hover:opacity-70"
        >
          Make browser tools work for every account
        </button>
      </div>
    </div>
  );
}
