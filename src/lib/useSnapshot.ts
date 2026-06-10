import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { AppSnapshot } from "./types";

const UPDATE_EVENT = "clyde://update";

/**
 * Live engine state: fetched once on mount, then kept in sync via the
 * `clyde://update` events the Rust core pushes on every change.
 */
export function useSnapshot() {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    api.getSnapshot().then(setSnapshot).catch(console.error);

    listen<AppSnapshot>(UPDATE_EVENT, (event) => {
      setSnapshot(event.payload);
    }).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, []);

  return { snapshot, setSnapshot };
}
