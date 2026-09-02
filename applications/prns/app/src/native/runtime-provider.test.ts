import { runtimeProvider as androidRuntimeProvider } from "./runtime-provider.android";
import { runtimeProvider as webRuntimeProvider } from "./runtime-provider.web";

describe("unsupported development runtime providers", () => {
  test("web reports a typed unavailable capability", () => {
    expect(webRuntimeProvider).toEqual({
      availability: {
        type: "unavailable",
        platform: "web",
        reason: "notImplemented",
      },
    });
  });

  test("Android reports a typed unavailable capability", () => {
    expect(androidRuntimeProvider).toEqual({
      availability: {
        type: "unavailable",
        platform: "android",
        reason: "notImplemented",
      },
    });
  });
});
