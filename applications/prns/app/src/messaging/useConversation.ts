import { useCallback } from "react";
import type { prns_groups as groups } from "@prns/app-bindings";
import { useDetail } from "./useDetail";
export const CATALOG_PAGE = { maxRows: 32, maxBytes: 64n * 1024n };
export const MEMBER_PAGE = { maxRows: 32, maxBytes: 8n * 1024n };
// Must fit one supported retained input/result; native rejects an insufficient
// byte budget instead of silently returning an empty successful page.
export const OPERATION_PAGE = { maxRows: 16, maxBytes: 1024n * 1024n };
function selectedId(value: string, kind: string): ArrayBuffer {
  if (!/^[0-9a-f]{64}$/.test(value)) throw new Error(`Invalid selected ${kind}.`);
  return Uint8Array.from(value.match(/../g)!, (part) => Number.parseInt(part, 16)).buffer;
}
export function groupId(value: string): ArrayBuffer { return selectedId(value, "conversation"); }
export function useConversation(client: groups.ClientLike, selected: string | undefined, active: boolean, revision: number) {
  const read = useCallback((signal: AbortSignal) => client.conversation(groupId(selected!), CATALOG_PAGE, { signal }), [client, selected]);
  return useDetail(client, selected, active, revision, read);
}
export function useInvitation(client: groups.ClientLike, selected: string | undefined, active: boolean, revision: number) {
  const read = useCallback((signal: AbortSignal) => client.invitation(selectedId(selected!, "invitation"), BigInt(Math.floor(Date.now() / 1000)), { signal }), [client, selected]);
  return useDetail(client, selected, active, revision, read);
}
