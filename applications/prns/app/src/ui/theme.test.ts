import { layoutModeForWidth } from "./theme";

describe("responsive layout", () => {
  it.each([
    [320, "compact"],
    [767, "compact"],
    [768, "medium"],
    [1199, "medium"],
    [1200, "wide"],
    [1920, "wide"],
  ] as const)("classifies %d logical pixels as %s", (width, expected) => {
    expect(layoutModeForWidth(width)).toBe(expected);
  });
});
