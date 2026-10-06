import { Tag, from, match, match_into } from "../../prns-js/src/browser/index.js";
import type {
  DataFrom,
  Tag as Tagged,
  TagFrom,
} from "../../prns-js/src/browser/index.js";

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

const roundTripped = JSON.parse(JSON.stringify(ready)) as Creation;
const roundTrippedValue = match(roundTripped, {
  Ready: ({ value }) => value,
  Missing: () => 0,
});
assert(roundTrippedValue === 42, "tagged data survives JSON round trips");

const absent: Creation = Tag("Missing");
const absentValue = match(absent as Creation, {
  Ready: () => "ready",
  Missing: () => "missing",
});
assert(absentValue === "missing", "data-less tags dispatch their handler");

const plainState = "Ready" as "Ready" | "Missing";
const plainValue = match(plainState, {
  Ready: () => 42,
  Missing: () => 0,
});
assert(plainValue === 42, "string literals dispatch their named handler");

const untaggedValue = match(42, {
  UNTAGGED: (value) => value + 1,
});
assert(untaggedValue === 43, "untagged values reach the fallback handler");

const { MakeTag } = from<Creation>();
const missing = MakeTag("Missing");
assert(missing.tag === "Missing", "from constructs data-less union members");

const into = match_into<number>().from(ready as Creation, {
  Ready: ({ value }) => value,
  Missing: () => 0,
});
assert(into === 42, "match_into constrains every branch to one return type");

console.log("casework smoke passed");
