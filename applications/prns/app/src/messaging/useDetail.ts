import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { errorMessage } from "./presentation";

/** Exact owner/scope reads retain loaded facts during automatic refresh. */
export function useDetail<T>(owner: object, scope: string | undefined, active: boolean, revision: number, read: (signal: AbortSignal) => Promise<T>) {
  const [value, setValue] = useState<T>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const epoch = useRef(0);
  const source = useRef({ owner, scope });
  const same = source.current.owner === owner && source.current.scope === scope;
  useLayoutEffect(() => {
    source.current = { owner, scope }; epoch.current++;
    setValue(undefined); setError(undefined); setBusy(false); setLoaded(false);
    return () => { epoch.current++; };
  }, [owner, scope]);
  useEffect(() => {
    if (!active || scope === undefined) return;
    const ticket = ++epoch.current, abort = new AbortController();
    const current = () => !abort.signal.aborted && ticket === epoch.current;
    setBusy(true);
    void Promise.resolve().then(() => read(abort.signal)).then((result) => {
      if (current()) { setValue(() => result); setError(undefined); setLoaded(true); }
    }).catch((cause: unknown) => { if (current()) { setValue(undefined); setError(errorMessage(cause)); } })
      .finally(() => { if (current()) setBusy(false); });
    return () => abort.abort();
  }, [owner, scope, active, revision, read]);
  return { value: same ? value : undefined, error: same ? error : undefined, busy: !same || busy, loaded: same && loaded };
}
