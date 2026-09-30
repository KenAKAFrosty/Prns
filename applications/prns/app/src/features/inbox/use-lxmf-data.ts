import * as Bindings from "@prns-internal/expo";
import type { LxmfMessage, LxmfMessageListOutcome } from "@prns-internal/expo";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { formatContactHash, parseDestinationHash } from "@/features/contacts/format";
import { useMessagingDirectory } from "@/features/contacts/messaging-directory";
import {
  type RuntimeCommandResult,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
import { messagePeer } from "./format";

// Leave room for one look-ahead row within the native maximum of 100.
const pageSize = 50;
type Failure = string | Bindings.NativeStoragePreparationError;
type Page = {
  readonly owner: object;
  readonly peerKey: string | null;
  readonly messages: readonly LxmfMessage[];
  readonly hasMore: boolean;
};
class MailboxReadError extends Error {}

/** Durable reads have their own revision and request ownership, independent of radio polling. */
export function useLxmfData(peer: Uint8Array | null) {
  const development = useDevelopmentRuntime();
  const directory = useMessagingDirectory();
  const peerKey = peer === null ? null : formatContactHash(peer);
  const available = development.availability.type === "available";
  const { listLxmfMessages, listLxmfConversations } = development;
  const phase = development.phase;
  const generationId = development.snapshot?.generationId;
  // Runtime replacement and recipient changes invalidate every outstanding read.
  const owner = useMemo(
    () => ({ peerKey, available, phase, generationId }),
    [peerKey, available, phase, generationId],
  );
  const [page, setPage] = useState<Page>({ owner, peerKey, messages: [], hasMore: false });
  const pageRef = useRef(page);
  const [pending, setPending] = useState(false);
  const [messagesLoaded, setMessagesLoaded] = useState(false);
  const [failure, setFailure] = useState<{ owner: object; detail: Failure } | null>(null);
  const request = useRef(0);
  const mounted = useRef(false);
  const currentOwner = useRef(owner);
  currentOwner.current = owner;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      request.current += 1;
    };
  }, []);

  const read = useCallback(
    async (before: bigint | undefined, limit: number) => {
      const selectedPeer = peerKey === null ? undefined : parseDestinationHash(peerKey);
      if (selectedPeer === null) {
        throw new MailboxReadError("The selected conversation destination is invalid.");
      }
      const result =
        peerKey === null
          ? await listLxmfConversations({ before, limit })
          : await listLxmfMessages({
              peer: selectedPeer,
              before,
              limit,
            });
      return listedMessages(result);
    },
    [listLxmfConversations, listLxmfMessages, peerKey],
  );

  const load = useCallback(
    async (older: boolean) => {
      // A callback captured by an earlier mutation can be invoked after its
      // screen/lifecycle owner departed. It must not invalidate a newer read.
      if (!mounted.current || currentOwner.current !== owner) return;
      if (!available) {
        setPending(false);
        return;
      }
      if (older && (!pageRef.current.hasMore || pageRef.current.owner !== owner)) return;
      const ticket = ++request.current;
      const current = () =>
        mounted.current && request.current === ticket && currentOwner.current === owner;
      const previous =
        pageRef.current.peerKey === peerKey
          ? pageRef.current
          : { owner, peerKey, messages: [], hasMore: false };
      setPending(true);
      setFailure(null);
      try {
        // Refresh every loaded row, including older delivery states. Each individual
        // read stays bounded; pagination never requires loading the whole mailbox.
        const wanted = older ? pageSize : Math.max(pageSize, previous.messages.length);
        let before = older ? previous.messages.at(-1)?.localRecordId : undefined;
        let rows: readonly LxmfMessage[] = [];
        let hasMore = false;
        while (rows.length < wanted) {
          const remaining = Math.min(pageSize, wanted - rows.length);
          const next = await read(before, remaining + 1);
          if (!current()) return;
          const included = next.slice(0, remaining);
          rows = [...rows, ...included];
          hasMore = next.length > remaining;
          if (!hasMore) break;
          const cursor = included.at(-1)?.localRecordId;
          if (cursor === undefined || (before !== undefined && cursor >= before)) {
            throw new Error("Mailbox cursor did not advance");
          }
          before = cursor;
        }
        const combined = older ? [...previous.messages, ...rows] : rows;
        const seen = new Set<string>();
        const messages = combined.filter((message) => {
          const key =
            peerKey === null
              ? formatContactHash(messagePeer(message))
              : message.localRecordId.toString();
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        });
        const nextPage = { owner, peerKey, messages, hasMore };
        pageRef.current = nextPage;
        setPage(nextPage);
        setMessagesLoaded(true);
      } catch (error) {
        if (current()) {
          setFailure({
            owner,
            detail:
              error instanceof Bindings.NativeStoragePreparationError
                ? error
                : error instanceof MailboxReadError
                  ? error.message
                  : "Messages could not be loaded. Try again.",
          });
        }
      } finally {
        if (current()) setPending(false);
      }
    },
    [available, owner, peerKey, read],
  );

  const refreshMessages = useCallback(() => load(false), [load]);
  const loadOlder = useCallback(() => load(true), [load]);
  const { refreshContacts, refreshDiscovery } = directory;
  const refresh = useCallback(async () => {
    if (!mounted.current || currentOwner.current !== owner) return;
    await Promise.all([refreshMessages(), refreshContacts(), refreshDiscovery()]);
  }, [owner, refreshContacts, refreshDiscovery, refreshMessages]);

  useEffect(() => {
    // Global snapshot revisions change on every radio poll. Reread only for LXMF
    // storage/projection changes (including active sends), or a new lifecycle owner.
    void development.snapshot?.lxmf.mailboxRevision;
    void development.snapshot?.lxmf.projectionRevision;
    void refreshMessages();
    return () => {
      request.current += 1;
    };
  }, [
    development.snapshot?.lxmf.mailboxRevision,
    development.snapshot?.lxmf.projectionRevision,
    refreshMessages,
  ]);

  return {
    peers: directory.peers,
    contacts: directory.contacts,
    messages: available && page.owner === owner ? page.messages : [],
    hasMore: available && page.owner === owner && page.hasMore,
    pending: available && (pending || directory.pending),
    messagesLoaded: available && page.owner === owner && messagesLoaded,
    failure:
      (failure?.owner === owner ? failure.detail : null) ??
      directory.failure ??
      directory.discoveryFailure,
    refresh,
    loadOlder,
  };
}

function listedMessages(
  result: RuntimeCommandResult<LxmfMessageListOutcome>,
): readonly LxmfMessage[] {
  if (result.type === "operationFailure") {
    if (result.storagePreparation !== undefined) {
      throw new Bindings.NativeStoragePreparationError(result.storagePreparation);
    }
    throw new MailboxReadError("Messages could not be loaded. Try again.");
  }
  switch (result.outcome.tag) {
    case Bindings.LxmfMessageListOutcome_Tags.Listed:
      return result.outcome.inner.messages;
    case Bindings.LxmfMessageListOutcome_Tags.InvalidInput:
      throw new MailboxReadError("Messages could not be loaded for this destination.");
    case Bindings.LxmfMessageListOutcome_Tags.DevelopmentUnavailable:
      throw new MailboxReadError("Messages are not available right now.");
    case Bindings.LxmfMessageListOutcome_Tags.DevelopmentResetRequired:
      throw new MailboxReadError("Reset app data to use messaging again.");
  }
}
