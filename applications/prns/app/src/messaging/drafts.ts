import AsyncStorage from "@react-native-async-storage/async-storage";
import { useCallback, useEffect, useRef, useState } from "react";
import { presentationCurrent, presentationEpoch, requirePresentationEpoch } from "../runtime/presentation-storage";
import { ConversationDraftStore, draftKey, type DraftSnapshot } from "./draft-store";

const drafts = new ConversationDraftStore(AsyncStorage);
export function resetConversationDrafts(): Promise<void> { return drafts.reset(); }
const empty: DraftSnapshot = { text: "", ready: false, pending: false };

export function useConversationDraft(identityScope: string, selected: string | undefined) {
  const epoch = useRef(presentationEpoch());
  const key = selected === undefined ? undefined : draftKey(identityScope, selected);
  const [state, setState] = useState<{ key: string | undefined; snapshot: DraftSnapshot }>({ key, snapshot: empty });
  const [actionError, setActionError] = useState<{ identity: string; group: string; text: string; message: string }>();
  const current = useRef<{ key: string; watch: ReturnType<ConversationDraftStore["watch"]> } | undefined>(undefined);
  const [attempt, setAttempt] = useState(0);
  const identity = useRef(identityScope);
  identity.current = identityScope;
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    if (key === undefined) return;
    let active = true;
    try {
      const watch = drafts.watch(key, (snapshot) => { if (active) setState({ key, snapshot }); });
      current.current = { key, watch };
      return () => { active = false; current.current = undefined; watch.close(); };
    } catch (cause) {
      setState({ key, snapshot: { ...empty, error: cause instanceof Error ? cause.message : String(cause) } });
    }
  }, [key, attempt]);
  const setText = useCallback((text: string) => {
    const selected = current.current;
    if (selected && selected.key === key) selected.watch.setText(text);
  }, [key]);
  const retry = useCallback(() => {
    if (!presentationCurrent(epoch.current)) return;
    if (actionError?.identity === identityScope) {
      const failed = actionError;
      const key = draftKey(failed.identity, failed.group);
      void drafts.clearMatching(key, failed.text).then(async (cleared) => { requirePresentationEpoch(epoch.current); if (!cleared) await drafts.retry(key); })
        .then(() => { if (mounted.current && identity.current === failed.identity) setActionError((current) => current === failed ? undefined : current); })
        .catch((cause: unknown) => { if (mounted.current && identity.current === failed.identity) setActionError((current) => current === failed ? { ...failed, message: cause instanceof Error ? cause.message : String(cause) } : current); });
    }
    const selected = current.current;
    if (selected && selected.key === key) selected.watch.retry();
    else setAttempt((previous) => previous + 1);
  }, [key, identityScope, actionError]);
  const clearMatching = useCallback(async (group: string, text: string) => {
    try { requirePresentationEpoch(epoch.current); return await drafts.clearMatching(draftKey(identityScope, group), text); }
    catch (cause) {
      if (mounted.current && identity.current === identityScope) setActionError({ identity: identityScope, group, text, message: cause instanceof Error ? cause.message : String(cause) });
      throw cause;
    }
  }, [identityScope]);
  const snapshot = state.key === key && key !== undefined ? state.snapshot : empty;
  return { ...snapshot, error: snapshot.error ?? (actionError?.identity === identityScope ? actionError.message : undefined), setText, retry, clearMatching };
}
