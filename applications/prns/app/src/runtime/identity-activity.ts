import AsyncStorage from "@react-native-async-storage/async-storage";
import { useEffect, useState } from "react";
import { prns_groups as groups } from "@prns/app-bindings";
import type { prns_identities as identities, prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { hex } from "../bytes";
import { identityKey } from "../identity-reference";
import { decodeViewed, unreadStatus, viewedDefault } from "../messaging/viewed";
import type { Unread, ViewedState } from "../messaging/viewed";
import { observeViewState } from "../view-state";
import { CATALOG_PAGE } from "../messaging/useConversation";

export type IdentityActivity = { unread: number; invitations: number; partial: boolean; checking?: boolean; error?: string };
type Cached = { head: string; viewed: ViewedState; unread: Unread; seen: object };
const page = CATALOG_PAGE;
const cacheEntries = 1024;
const pause = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/** Walk all summary pages with bounded requests. Cache unchanged native heads so
 * a receipt/change hint does not decode every saved conversation again. */
export async function scanActivity(client: groups.ClientLike, viewed: ViewedState, signal: AbortSignal,
  cache: Map<string, Cached>, publish: (activity: IdentityActivity) => void): Promise<void> {
  let unread = 0, invitations = 0, incomplete = false;
  let after: ArrayBuffer | undefined;
  const traversal = {};
  do {
    const result = await client.conversations(after, page, { signal });
    for (const conversation of result.items) {
      if (signal.aborted) return;
      const key = hex(conversation.group);
      try {
        const changes = await client.changes(conversation.group, undefined, 1, { signal });
        const head = `${hex(changes.head.incarnation)}:${changes.head.position}`;
        const previous = cache.get(key);
        const status = previous?.head === head && previous.viewed === viewed ? previous.unread
          : await unreadStatus(client, conversation.group, viewed, signal);
        cache.delete(key);
        if (cache.size >= cacheEntries) cache.delete(cache.keys().next().value!);
        cache.set(key, { head, viewed, unread: status, seen: traversal });
        if (status === "unread") unread++;
        else if (status === "unchecked") incomplete = true;
      } catch { if (signal.aborted) return; incomplete = true; }
    }
    if (signal.aborted) return;
    // Publish one completed traversal below. Intermediate conversation totals
    // have not read invitations yet and must not replace known activity.
    if (result.next !== undefined && after !== undefined && hex(result.next) <= hex(after)) throw new Error("Conversation activity cursor did not advance.");
    after = result.next;
    await pause();
  } while (after !== undefined && !signal.aborted);
  for (const [key, value] of cache) if (value.seen !== traversal) cache.delete(key);
  let invitationAfter: ArrayBuffer | undefined;
  do {
    const result = await client.invitations(BigInt(Math.floor(Date.now() / 1000)), invitationAfter, page, { signal });
    invitations += result.items.filter((item) => !item.outgoing && item.state === groups.GroupInvitationState.Pending).length;
    if (result.next !== undefined && invitationAfter !== undefined && hex(result.next) <= hex(invitationAfter)) throw new Error("Invitation activity cursor did not advance.");
    invitationAfter = result.next;
    await pause();
  } while (invitationAfter !== undefined && !signal.aborted);
  if (!signal.aborted) publish({ unread, invitations, partial: incomplete });
}

export function useIdentityActivities(owner: owners.ServiceRuntimeLike | undefined, records: readonly identities.IdentityRecord[], active: boolean) {
  const [activity, setActivity] = useState<Readonly<Record<string, IdentityActivity>>>({});
  const scope = records.map((record) => `${identityKey(record)}:${record.services.join(",")}`).join(";");
  useEffect(() => {
    setActivity({});
    if (owner === undefined || !active) return;
    const abort = new AbortController();
    const cleanups: (() => void)[] = [];
    for (const record of records) {
      if (!record.services.includes(0x67727073)) continue;
      const key = identityKey(record);
      const viewedKey = `prns.ui.viewed.${key}.v1`;
      const publish = (value: IdentityActivity) => { if (!abort.signal.aborted) setActivity((previous) => ({ ...previous, [key]: value })); };
      const fail = (cause: unknown) => { if (!abort.signal.aborted) setActivity((previous) => ({ ...previous,
        [key]: { unread: previous[key]?.unread ?? 0, invitations: previous[key]?.invitations ?? 0, partial: true, error: String(cause) },
      })); };
      publish({ unread: 0, invitations: 0, partial: true, checking: true });
      void (async () => {
        let observation: groups.GroupObservationLike | undefined;
        try {
          const client = owner.groups(record);
          observation = await client.subscribe({ signal: abort.signal });
          if (abort.signal.aborted) return;
          const cache = new Map<string, Cached>();
          let viewedRaw: string | null | undefined;
          let viewed = viewedDefault;
          let requested = false, running = false;
          const refresh = async () => {
            requested = true;
            if (running) return;
            running = true;
            try {
              while (requested && !abort.signal.aborted) {
                requested = false;
                const raw = await AsyncStorage.getItem(viewedKey);
                if (raw !== viewedRaw) { viewed = raw === null ? viewedDefault : decodeViewed(JSON.parse(raw)); viewedRaw = raw; }
                await scanActivity(client, viewed, abort.signal, cache, publish);
                // Coalesce bursts of receipts before another bounded traversal.
                if (!abort.signal.aborted) await new Promise<void>((resolve) => setTimeout(resolve, 250));
              }
            } catch (cause) { fail(cause); }
            finally { running = false; }
          };
          cleanups.push(observeViewState(viewedKey, () => { void refresh(); }));
          void refresh();
          while (!abort.signal.aborted) { await observation.next({ signal: abort.signal }); if (!abort.signal.aborted) void refresh(); }
        } catch (cause) { fail(cause); }
        finally { if (observation !== undefined && "uniffiDestroy" in observation && typeof observation.uniffiDestroy === "function") observation.uniffiDestroy(); }
      })();
    }
    return () => { abort.abort(); for (const cleanup of cleanups) cleanup(); };
  }, [owner, scope, active]);
  return activity;
}

export function activityLabel(value: IdentityActivity | undefined): string | undefined {
  if (value === undefined) return undefined;
  const parts = [value.unread > 0 ? `${value.partial ? "at least " : ""}${value.unread} unread` : undefined,
    value.invitations > 0 ? `${value.invitations} invitation${value.invitations === 1 ? "" : "s"}` : undefined,
    value.partial ? value.error !== undefined ? "activity unavailable" : value.checking ? "checking activity" : "some activity unchecked" : undefined];
  return parts.filter(Boolean).join(" · ") || undefined;
}
