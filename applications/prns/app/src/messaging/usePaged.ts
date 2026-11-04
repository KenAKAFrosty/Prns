import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { errorMessage } from "./presentation";

// A navigation window, not a catalog or domain limit. First always remains reachable.
const CURSOR_WINDOW = 64;
/** Presentation owns one page and a fixed cursor window, never a copied catalog. */
export function usePaged<T, C>({ owner, scope, active, revision, read }: {
  owner: object; scope: string; active: boolean; revision: number;
  read: (after: C | undefined, signal: AbortSignal) => Promise<{ items: T[]; next: C | undefined }>;
}) {
  const [navigation, setNavigation] = useState({ owner, scope, cursors: [undefined] as (C | undefined)[], page: 1 });
  const { cursors } = navigation;
  const [rows, setRows] = useState<T[]>([]);
  const [nextCursor, setNextCursor] = useState<C>();
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const epoch = useRef(0);
  const pending = useRef<AbortController | undefined>(undefined);
  const moving = useRef(false);
  const source = useRef({ owner, scope });
  const same = source.current.owner === owner && source.current.scope === scope && navigation.owner === owner && navigation.scope === scope;
  useLayoutEffect(() => {
    source.current = { owner, scope };
    epoch.current++;
    pending.current?.abort(); pending.current = undefined; moving.current = false;
    setNavigation({ owner, scope, cursors: [undefined], page: 1 }); setRows([]); setNextCursor(undefined);
    setLoaded(false); setBusy(false); setError(undefined);
    return () => { epoch.current++; pending.current?.abort(); };
  }, [owner, scope]);
  const after = cursors[cursors.length - 1];
  useEffect(() => {
    // The layout reset schedules a render. Do not use the previous owner/scope's
    // cursor during the passive effects of this transitional render.
    if (navigation.owner !== owner || navigation.scope !== scope) return;
    if (!active) { moving.current = false; setBusy(false); return; }
    const ticket = ++epoch.current;
    const abort = new AbortController(); pending.current = abort;
    setBusy(true);
    const current = () => !abort.signal.aborted && ticket === epoch.current;
    void read(after, abort.signal).then((page) => {
      if (!current()) return;
      setRows(page.items); setNextCursor(page.next); setLoaded(true); setError(undefined);
    }).catch((cause: unknown) => { if (current()) setError(errorMessage(cause)); })
      .finally(() => {
        if (pending.current === abort) {
          pending.current = undefined; moving.current = false;
          if (current()) setBusy(false);
        }
      });
    return () => { abort.abort(); };
  }, [owner, scope, navigation.owner, navigation.scope, active, after, revision, read, refresh]);
  const move = (direction: "next" | "previous" | "first") => {
    if (!active || moving.current || !same || (direction !== "first" && (!loaded || error !== undefined))) return;
    if (direction === "next" && nextCursor === undefined) return;
    if (direction === "previous" && cursors.length <= 1) return;
    if (direction === "first" && navigation.page === 1) return;
    moving.current = true; epoch.current++; pending.current?.abort(); pending.current = undefined; setBusy(true);
    setRows([]); setLoaded(false); setNextCursor(undefined);
    setNavigation((old) => direction === "first" ? { owner, scope, cursors: [undefined], page: 1 }
      : direction === "next" ? { owner, scope, cursors: [...old.cursors, nextCursor].slice(-CURSOR_WINDOW), page: old.page + 1 }
        : { owner, scope, cursors: old.cursors.slice(0, -1), page: old.page - 1 });
  };
  const retry = useCallback(() => {
    if (!active || source.current.owner !== owner || source.current.scope !== scope) return;
    epoch.current++; pending.current?.abort(); moving.current = false;
    setError(undefined); setRefresh((value) => value + 1);
  }, [owner, scope, active]);
  return { rows: same ? rows : [], loaded: same && loaded, busy: !same || busy,
    error: same ? error : undefined, page: same ? navigation.page : 1,
    hasNext: same && nextCursor !== undefined, hasPrevious: same && cursors.length > 1,
    next: () => move("next"), previous: () => move("previous"), first: () => move("first"), retry };
}
