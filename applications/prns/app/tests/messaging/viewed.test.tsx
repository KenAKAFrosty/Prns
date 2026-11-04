import "./native-mock";
import type { prns_groups as groups } from "@prns/app-bindings";
import { hex } from "../../src/bytes";
import { decodeViewed, markViewed, unreadStatus, viewedDefault, viewedByteBudget } from "../../src/messaging/viewed";

const bytes = (size: number, fill = 1) => new Uint8Array(size).fill(fill).buffer;
const group = bytes(32);
const incarnation = bytes(24);
const position = (value: bigint, source = incarnation): groups.GroupChangeToken => ({ group, incarnation: source, position: value });
const head = (value: bigint, source = incarnation) => markViewed(viewedDefault, [position(value, source)]);
function message(value: bigint, outgoing = false, source = incarnation): groups.GroupTextMessage {
  return { position: position(value, source), outgoing, id: bytes(32, Number(value)), author: bytes(16), text: "Text",
    membershipChanged: false, receivedCount: 0n, recipientCount: 1n, sentAtMillis: 0n };
}
function fixture(...pages: groups.GroupTextPage[]) {
  const messages = jest.fn<Promise<groups.GroupTextPage>, unknown[]>();
  for (const page of pages) messages.mockResolvedValueOnce(page);
  return { messages, client: { messages } as unknown as groups.ClientLike };
}
function page(messages: groups.GroupTextMessage[], more?: groups.GroupRecordCursor): groups.GroupTextPage {
  return { messages, visitedHeads: [position(100n)], currentIncarnation: incarnation, nextBefore: more };
}
const signal = () => new AbortController().signal;

test("outgoing messages and later receipt revisions do not create unread incoming messages", async () => {
  const { client } = fixture(page([message(11n, true), message(8n)]));
  expect(await unreadStatus(client, group, head(10n), signal())).toBe("viewed");
});
test("new incoming records beyond the local viewed position are unread", async () => {
  const { client } = fixture(page([message(11n), message(8n)]));
  expect(await unreadStatus(client, group, head(10n), signal())).toBe("unread");
});
test("a large viewed position never marks a new membership incarnation viewed", async () => {
  const { client } = fixture(page([message(2n, false, bytes(24, 2))]));
  expect(await unreadStatus(client, group, head(100n), signal())).toBe("unread");
});
test("empty text pages continue into earlier membership history", async () => {
  const cursor = { group, incarnation: bytes(24, 2), before: 5n };
  const { client, messages } = fixture(page([], cursor), page([message(3n, false, cursor.incarnation)]));
  expect(await unreadStatus(client, group, viewedDefault, signal())).toBe("unread");
  expect(messages.mock.calls[1]?.[1]).toEqual(cursor);
});
test("bounded paging and concurrent readmission report unchecked rather than a false viewed result", async () => {
  const cursor = { group, incarnation, before: 5n };
  const { client } = fixture(...Array.from({ length: 8 }, () => page([], cursor)));
  expect(await unreadStatus(client, group, viewedDefault, signal())).toBe("unchecked");
  const changed = fixture(page([], cursor), { ...page([]), currentIncarnation: bytes(24, 2) });
  expect(await unreadStatus(changed.client, group, viewedDefault, signal())).toBe("unchecked");
});
test("viewed positions retain exact bigint precision and never move backwards", () => {
  const initial = head(9007199254740993n);
  expect(markViewed(initial, [position(7n)])).toBe(initial);
  expect(decodeViewed(JSON.parse(JSON.stringify(initial)))).toEqual(initial);
  expect(initial.heads[`${hex(group)}:${hex(incarnation)}`]).toBe("9007199254740993");
  expect(() => decodeViewed({ version: 1, heads: { [`${hex(group)}:${hex(incarnation)}`]: "18446744073709551616" } })).toThrow();
});

test("viewed markers use a byte budget rather than the former 4096-conversation ceiling", () => {
  const heads = Object.fromEntries(Array.from({ length: 5000 }, (_, i) => [`${i.toString(16).padStart(64, "0")}:${"a".repeat(48)}`, "1"]));
  expect(Object.keys(decodeViewed({ version: 1, heads }).heads)).toHaveLength(5000);
  const tooMany = Object.fromEntries(Array.from({ length: Math.ceil(viewedByteBudget / 100) }, (_, i) => [`${i.toString(16).padStart(64, "0")}:${"a".repeat(48)}`, "1"]));
  const before = JSON.stringify(tooMany);
  expect(() => decodeViewed({ version: 1, heads: tooMany })).toThrow(/byte budget/);
  expect(JSON.stringify(tooMany)).toBe(before);
});
