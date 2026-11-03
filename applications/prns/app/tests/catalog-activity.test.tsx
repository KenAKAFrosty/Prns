import { completedPrefix } from "../src/CatalogActivity";
import type { prns_identities as identities } from "@prns/app-bindings";

jest.mock("@prns/app-bindings", () => ({
  prns_identities_core: { IdentityOperationState_Tags: { Completed: "Completed" } },
  prns_identities: {},
}));
function operation(sequence: bigint, state: string, unpublishedKey?: object): identities.IdentityOperationRecord {
  return { token: { sequence }, state: { tag: state }, unpublishedKey } as identities.IdentityOperationRecord;
}
test("acknowledgement stops at an earlier unresolved operation even when later results completed", () => {
  expect(completedPrefix([operation(5n, "Completed"), operation(2n, "Completed"), operation(3n, "Transitioning")])).toBe(2n);
  expect(completedPrefix([operation(3n, "Reserved"), operation(5n, "Completed")])).toBeUndefined();
});
test("a completed operation retaining a key blocks pruning through it", () => {
  expect(completedPrefix([operation(1n, "Completed"), operation(2n, "Completed", {}), operation(3n, "Completed")])).toBe(1n);
});
test("large sequence numbers remain exact and an empty catalog has no acknowledgement target", () => {
  expect(completedPrefix([])).toBeUndefined();
  expect(completedPrefix([operation(9007199254740993n, "Completed"), operation(9007199254740992n, "Completed")])).toBe(9007199254740993n);
});
