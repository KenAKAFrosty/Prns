import { Tag, from, match, match_into } from "../ts/index.js";
import type { DataFrom, Tag as Tagged, TagFrom } from "../ts/index.js";

type Creation =
  | Tagged<"Ready", { readonly value: number }>
  | Tagged<"Missing">;

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? true
    : false;
type Expect<Value extends true> = Value;
type CreationTags = Expect<Equal<TagFrom<Creation>, "Ready" | "Missing">>;
type CreationData = Expect<
  Equal<DataFrom<Creation>, { readonly value: number } | undefined>
>;

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

const ready: Creation = Tag("Ready", { value: 42 });
const readyValue = match(ready as Creation, {
  Ready: ({ value }) => value,
  Missing: () => 0,
});
assert(readyValue === 42, "match dispatches tagged data");

const structural = {
  ...ready,
  observedAt: 123,
};
const structuralValue = match(structural as Creation, {
  Ready: ({ value }) => value,
  Missing: () => 0,
});
assert(structuralValue === 42, "match accepts structural supersets");

const roundTripped = JSON.parse(JSON.stringify(Tag("Missing"))) as Creation;
const roundTrippedValue = match(roundTripped, {
  Ready: () => "ready",
  Missing: () => "missing",
});
assert(roundTrippedValue === "missing", "data-less tags survive JSON round trips");

const futureState: string = "FutureState";
const futureValue = match(futureState, {
  UNTAGGED: (value) => `untagged:${value}`,
});
assert(futureValue === "untagged:FutureState", "wide strings use UNTAGGED");

const reservedState: string = "UNTAGGED";
const reservedValue = match(reservedState, {
  UNTAGGED: (value) => `untagged:${value}`,
});
assert(reservedValue === "untagged:UNTAGGED", "UNTAGGED remains a value");

type CollidingName = "Ready" | Tagged<"Ready", { readonly value: number }>;
function collidingValue(value: CollidingName): number {
  return match(value, {
    Ready: (data) => data?.value ?? 0,
  });
}
assert(collidingValue("Ready") === 0, "plain and tagged names may overlap");
assert(collidingValue(ready) === 42, "overlapping tagged data remains typed");

const { MakeTag } = from<Creation>();
const missing = MakeTag("Missing");
assert(missing.tag === "Missing", "from constructs data-less union members");

const into = match_into<number>().from<Creation>(ready, {
  Ready: ({ value }) => value,
  Missing: () => 0,
});
assert(into === 42, "match_into constrains every branch to one return type");

let rejectedPrototypeHandler = false;
try {
  match(Tag("toString") as Tagged<"toString">, {} as never);
} catch (error) {
  rejectedPrototypeHandler = error instanceof TypeError;
}
assert(rejectedPrototypeHandler, "prototype properties are not handlers");

console.log("casework smoke passed");
