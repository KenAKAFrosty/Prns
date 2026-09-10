import { routeParamsAreValid, screenById } from "./catalog";

describe("route parameter validation", () => {
  it("rejects unknown parameters on routes without parameters", () => {
    const entry = screenById("nodes.index");

    expect(routeParamsAreValid(entry, {})).toBe(true);
    expect(routeParamsAreValid(entry, { unexpected: "value" })).toBe(false);
  });

  it("requires one non-empty scalar and rejects repeated values", () => {
    const entry = screenById("inbox.conversation");

    expect(routeParamsAreValid(entry, { destination: "00".repeat(16) })).toBe(true);
    expect(routeParamsAreValid(entry, {})).toBe(false);
    expect(routeParamsAreValid(entry, { destination: "not-a-destination" })).toBe(false);
    expect(routeParamsAreValid(entry, { destination: "" })).toBe(false);
    expect(routeParamsAreValid(entry, { destination: "   " })).toBe(false);
    expect(routeParamsAreValid(entry, { destination: ["00".repeat(16)] })).toBe(false);
  });

  it("accepts only declared onboarding steps as a single optional value", () => {
    const entry = screenById("installation.onboarding");

    expect(routeParamsAreValid(entry, {})).toBe(true);
    for (const step of [
      "welcome",
      "create",
      "import",
      "retention",
      "provision",
      "interfaces",
      "pairing",
      "complete",
    ]) {
      expect(routeParamsAreValid(entry, { step })).toBe(true);
    }
    expect(routeParamsAreValid(entry, { step: "future-step" })).toBe(false);
    expect(routeParamsAreValid(entry, { step: ["welcome", "complete"] })).toBe(false);
  });

  it("keeps manual contact creation parameter-free", () => {
    const entry = screenById("contacts.add");

    expect(routeParamsAreValid(entry, {})).toBe(true);
    expect(routeParamsAreValid(entry, { source: "manual" })).toBe(false);
  });

  it("addresses contact details by one destination parameter", () => {
    const entry = screenById("contacts.entry");

    expect(routeParamsAreValid(entry, { destination: "00".repeat(16) })).toBe(true);
    expect(routeParamsAreValid(entry, {})).toBe(false);
    expect(routeParamsAreValid(entry, { destination: ["00".repeat(16)] })).toBe(false);
  });

  it("accepts only exact optional compose destinations and declared union values", () => {
    const compose = screenById("inbox.compose");
    const location = screenById("location.map");

    expect(routeParamsAreValid(compose, {})).toBe(true);
    expect(routeParamsAreValid(compose, { destination: "00".repeat(16) })).toBe(true);
    expect(routeParamsAreValid(compose, { destination: "destination:abc" })).toBe(false);
    expect(routeParamsAreValid(compose, { destination: "00".repeat(15) })).toBe(false);

    expect(routeParamsAreValid(location, { focus: "local" })).toBe(true);
    expect(routeParamsAreValid(location, { focus: "directory:abc" })).toBe(true);
    expect(routeParamsAreValid(location, { focus: "message:abc" })).toBe(true);
    expect(routeParamsAreValid(location, { focus: "local:abc" })).toBe(false);
  });
});
