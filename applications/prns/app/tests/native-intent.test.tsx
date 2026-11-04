import { redirectSystemPath } from "../app/+native-intent";

test("incoming contact links open explicit review while ordinary routes preserve their scope", () => {
  expect(redirectSystemPath({ path: "rns-contact://v1/public-card", initial: true })).toBe("/contacts");
  const scoped = "/conversations/group/abc?owner=one%3A1%3Atwo";
  expect(redirectSystemPath({ path: scoped, initial: false })).toBe(scoped);
});

test("malformed or oversized external paths never enter the router decoder", () => {
  for (const path of ["/contacts?label=%E0%A4", "/contacts?label=%GG", `/contacts?x=${"%".repeat(20_000)}`]) {
    expect(redirectSystemPath({ path, initial: false })).toBe("/invalid-link");
  }
});
