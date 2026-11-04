import { useCallback, useEffect, useRef, useState } from "react";
import { prns_chat_app as app } from "@prns/app-bindings";
import * as platform from "@prns/app-platform";
import { clearAppPresentation, markPresentationResetPending } from "./reset-presentation";

export type ResetState = {
  busy: boolean; reviewed: boolean; pending: boolean; localDeleted: boolean; error?: string | undefined;
};
/** The opaque native review stays with the installation, across route changes.
 * After reload, only nondestructive cleanup may resume without a fresh review. */
export function useInstallationReset(pending: boolean, block: () => void, finished: () => void) {
  const [state, setState] = useState<ResetState>({ busy: false, reviewed: false, pending: false, localDeleted: false });
  const review = useRef<app.AppResetReviewLike | undefined>(undefined);
  const flight = useRef(false);
  const deleted = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const publish = (value: ResetState) => { if (mounted.current) setState(value); };
  const cleanup = useCallback(async () => {
    await platform.resetPreferences();
    await clearAppPresentation();
    deleted.current = false;
    if (mounted.current) { setState({ busy: false, reviewed: false, pending: false, localDeleted: false }); finished(); }
  }, [finished]);
  const prepare = useCallback(async () => {
    if (flight.current) return;
    flight.current = true;
    setState((previous) => ({ ...previous, busy: true, error: undefined }));
    try {
      await platform.prepareReset();
      review.current = app.appPrepareReset();
      setState({ busy: false, reviewed: true, pending, localDeleted: false });
    } catch (cause) { setState({ busy: false, reviewed: false, pending, localDeleted: false, error: String(cause) }); }
    finally { flight.current = false; }
  }, [pending]);
  const confirm = useCallback(async () => {
    if (flight.current || review.current === undefined) return;
    const exact = review.current;
    flight.current = true;
    publish({ busy: true, reviewed: true, pending: true, localDeleted: false });
    try {
      block();
      await markPresentationResetPending();
      await platform.stop();
      await app.appReset(exact);
      deleted.current = true; review.current = undefined;
      publish({ busy: true, reviewed: false, pending: true, localDeleted: true });
      await cleanup();
    } catch (cause) {
      publish({ busy: false, reviewed: review.current !== undefined, pending: true, localDeleted: deleted.current, error: String(cause) });
    } finally { flight.current = false; }
  }, [block, cleanup]);
  const resumeCleanup = useCallback(async () => {
    if (flight.current) return;
    flight.current = true;
    publish({ busy: true, reviewed: false, pending: true, localDeleted: true });
    try {
      // This is also the reload check: never infer deletion from a stale UI flag.
      if (await platform.installationPresent()) throw new Error("Saved native data is still present. Review and confirm reset again before deleting it.");
      deleted.current = true;
      await platform.stop();
      await cleanup();
    } catch (cause) {
      publish({ busy: false, reviewed: false, pending: true, localDeleted: deleted.current, error: String(cause) });
    } finally { flight.current = false; }
  }, [cleanup]);
  useEffect(() => {
    if (!pending || flight.current) return;
    let cancelled = false;
    void platform.installationPresent().then((present) => {
      if (cancelled || flight.current) return;
      if (present) setState({ busy: false, reviewed: false, pending: true, localDeleted: false });
      else void resumeCleanup();
    }).catch((cause) => { if (!cancelled) setState({ busy: false, reviewed: false, pending: true, localDeleted: false, error: String(cause) }); });
    return () => { cancelled = true; };
  }, [pending, resumeCleanup]);
  return { state, prepare, confirm, resumeCleanup,
    cancel: () => { if (!flight.current && !pending) { review.current = undefined; setState({ busy: false, reviewed: false, pending: false, localDeleted: false }); } } };
}
