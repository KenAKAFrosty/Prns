import { scopedDevelopmentRuntime } from "@prns-internal/expo";
import { runtimeProvider as androidRuntimeProvider } from "./runtime-provider.android";
import { runtimeProvider as iosRuntimeProvider } from "./runtime-provider.ios";
import { runtimeProvider as webRuntimeProvider } from "./runtime-provider.web";

jest.mock("@prns-internal/expo", () => ({
  androidRuntime: { readStatus: jest.fn() },
  accessorySetupRuntime: { readStatus: jest.fn() },
  developmentRuntime: { startDevelopmentNode: jest.fn() },
  scopedDevelopmentRuntime: jest.fn(),
}));

describe("platform development runtime providers", () => {
  test("web reports a typed unavailable capability", () => {
    expect(webRuntimeProvider).toEqual({
      availability: {
        type: "unavailable",
        platform: "web",
        reason: "notImplemented",
      },
    });
  });

  test("Android attaches to the process-owned runtime without Apple accessory setup", () => {
    expect(androidRuntimeProvider.availability).toEqual({ type: "available", platform: "android" });
    expect("accessorySetup" in androidRuntimeProvider).toBe(false);
    expect("androidRuntime" in androidRuntimeProvider).toBe(true);
    if (!("acquire" in androidRuntimeProvider)) throw new Error("Android provider is unavailable");
    const onSnapshot = jest.fn();
    androidRuntimeProvider.acquire({ onSnapshot, onBackgroundFailure: jest.fn() });
    expect(scopedDevelopmentRuntime).toHaveBeenLastCalledWith(
      androidRuntimeProvider.runtime,
      expect.objectContaining({ nativeLifetime: "process", onSnapshot }),
    );
  });

  test("iOS retains its accessory capability and process lifetime", () => {
    expect("accessorySetup" in iosRuntimeProvider).toBe(true);
    expect("androidRuntime" in iosRuntimeProvider).toBe(false);
    if (!("acquire" in iosRuntimeProvider)) throw new Error("iOS provider is unavailable");
    iosRuntimeProvider.acquire({ onSnapshot: jest.fn(), onBackgroundFailure: jest.fn() });
    expect(scopedDevelopmentRuntime).toHaveBeenLastCalledWith(
      iosRuntimeProvider.runtime,
      expect.objectContaining({ nativeLifetime: "process" }),
    );
  });
});
