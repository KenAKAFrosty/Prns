import { prns_groups as groups } from "@prns/app-bindings";
import { scanActivity } from "../src/runtime/identity-activity";
import type { IdentityActivity } from "../src/runtime/identity-activity";
import { viewedDefault } from "../src/messaging/viewed";

jest.mock("@react-native-async-storage/async-storage", () => ({ __esModule: true, default: {} }));
jest.mock("@prns/app-bindings", () => ({ prns_groups: { GroupInvitationState: { Pending: 0 } },
  prns_groups_core: { GroupClockReading: { Trusted: class {} } },
}));
const bytes = (value: number, size: number) => new Uint8Array(size).fill(value).buffer;
const first = bytes(1, 32), second = bytes(2, 32), incarnation = bytes(3, 24);
const hex = (value: ArrayBuffer) => Array.from(new Uint8Array(value), (item) => item.toString(16).padStart(2, "0")).join("");

test("activity includes later conversation and filtered invitation pages and reuses unchanged history", async () => {
  const conversations = jest.fn(async (after: ArrayBuffer | undefined) => after === undefined
    ? { items: [{ group: first }], next: first } : { items: [{ group: second }], next: undefined });
  const changes = jest.fn(async (group: ArrayBuffer) => ({ head: { group, incarnation, position: 1n } }));
  const messages = jest.fn(async (group: ArrayBuffer) => ({ currentIncarnation: incarnation, nextBefore: undefined,
    messages: [{ outgoing: false, position: { group, incarnation, position: 1n } }] }));
  const invitations = jest.fn(async (_now: bigint, after: ArrayBuffer | undefined) => after === undefined
    ? { items: [], next: first } : { items: [{ outgoing: false, state: groups.GroupInvitationState.Pending }], next: undefined });
  const client = { conversations, changes, messages, invitations } as unknown as groups.ClientLike;
  const cache = new Map();
  const publish = jest.fn<void, [IdentityActivity]>();
  const viewed = { version: 1 as const, heads: { [`${hex(first)}:${hex(incarnation)}`]: "1" } };
  const signal = new AbortController().signal;
  await scanActivity(client, viewed, signal, cache, publish);
  expect(publish).toHaveBeenLastCalledWith({ unread: 1, invitations: 1, partial: false });
  expect(conversations).toHaveBeenCalledTimes(2);
  expect(invitations).toHaveBeenCalledTimes(2);
  expect(messages).toHaveBeenCalledTimes(2);
  await scanActivity(client, viewed, signal, cache, publish);
  expect(messages).toHaveBeenCalledTimes(2);
  await scanActivity(client, { ...viewed, heads: { ...viewed.heads, [`${hex(second)}:${hex(incarnation)}`]: "1" } }, signal, cache, publish);
  expect(publish).toHaveBeenLastCalledWith({ unread: 0, invitations: 1, partial: false });
});

test("failed history reads leave activity explicitly incomplete", async () => {
  const client = { conversations: async () => ({ items: [{ group: first }], next: undefined }),
    changes: async () => { throw new Error("Storage unavailable"); },
    invitations: async () => ({ items: [], next: undefined }),
  } as unknown as groups.ClientLike;
  const publish = jest.fn<void, [IdentityActivity]>();
  await scanActivity(client, viewedDefault, new AbortController().signal, new Map(), publish);
  expect(publish).toHaveBeenLastCalledWith({ unread: 0, invitations: 0, partial: true });
});


test("background activity scan keeps completed invitation totals until all reads finish", async () => {
  type InvitationPage = { items: { outgoing: boolean; state: number }[]; next: undefined };
  let finishInvitations!: (page: InvitationPage) => void;
  let reachedInvitations!: () => void;
  const heldInvitations = new Promise<InvitationPage>((resolve) => { finishInvitations = resolve; });
  const readingInvitations = new Promise<void>((resolve) => { reachedInvitations = resolve; });
  const invitations = jest.fn(async (): Promise<InvitationPage> => ({
    items: [{ outgoing: false, state: groups.GroupInvitationState.Pending }], next: undefined,
  }));
  const client = { conversations: async () => ({ items: [], next: undefined }), invitations } as unknown as groups.ClientLike;
  let visible: IdentityActivity | undefined;
  const publish = jest.fn((value: IdentityActivity) => { visible = value; });
  const signal = new AbortController().signal;
  const cache = new Map();
  await scanActivity(client, viewedDefault, signal, cache, publish);
  expect(visible).toEqual({ unread: 0, invitations: 1, partial: false });
  publish.mockClear();
  invitations.mockImplementationOnce(() => { reachedInvitations(); return heldInvitations; });
  const refresh = scanActivity(client, viewedDefault, signal, cache, publish);
  await readingInvitations;
  // The conversation page is complete, but its zero invitation subtotal must
  // not hide a known banner while the invitation read is still in flight.
  expect(publish).not.toHaveBeenCalled();
  expect(visible).toEqual({ unread: 0, invitations: 1, partial: false });
  finishInvitations({ items: [], next: undefined });
  await refresh;
  expect(publish).toHaveBeenCalledTimes(1);
  expect(visible).toEqual({ unread: 0, invitations: 0, partial: false });
});
