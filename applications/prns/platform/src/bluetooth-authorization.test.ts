import { NativePayloadError } from "./native-payload";
import {
  createBluetoothAuthorizationRuntime,
  parseBluetoothAuthorizationStatus,
} from "./bluetooth-authorization";
import type { AppleBluetoothNativeModule } from "./native";

const readyStatus = {
  authorization: "allowedAlways",
  nativeStart: "running",
  restorationAttemptRequested: true,
  revision: 4,
};

function nativeFixture(
  overrides: Partial<AppleBluetoothNativeModule> = {},
): AppleBluetoothNativeModule {
  return {
    bluetoothAuthorizationStatus: jest.fn(async () => JSON.stringify(readyStatus)),
    addListener: jest.fn(() => ({ remove: jest.fn() })),
    ...overrides,
  };
}

describe("ordinary Bluetooth authorization bridge", () => {
  test.each(["notDetermined", "allowedAlways", "denied", "restricted"] as const)(
    "preserves the %s app permission without implying any connected peer",
    async (authorization) => {
      const runtime = createBluetoothAuthorizationRuntime(
        nativeFixture({
          bluetoothAuthorizationStatus: async () =>
            JSON.stringify({ ...readyStatus, authorization }),
        }),
      );
      await expect(runtime.readStatus()).resolves.toEqual({ ...readyStatus, authorization });
    },
  );

  test("delivers valid status changes, ignores malformed events, and releases its subscription", () => {
    let nativeListener: ((event: { readonly status: string }) => void) | undefined;
    const remove = jest.fn();
    const addListener = jest.fn((_name, listener) => {
      nativeListener = listener;
      return { remove };
    });
    const runtime = createBluetoothAuthorizationRuntime(nativeFixture({ addListener }));
    const listener = jest.fn();
    const subscription = runtime.addStatusListener(listener);
    nativeListener?.({
      status: JSON.stringify({ ...readyStatus, authorization: "denied", revision: 5 }),
    });
    nativeListener?.({ status: "not JSON" });
    nativeListener?.({ status: JSON.stringify({ ...readyStatus, revision: -1 }) });
    subscription.remove();
    expect(addListener).toHaveBeenCalledWith(
      "onBluetoothAuthorizationStatus",
      expect.any(Function),
    );
    expect(listener).toHaveBeenCalledTimes(1);
    expect(listener).toHaveBeenCalledWith({ ...readyStatus, authorization: "denied", revision: 5 });
    expect(remove).toHaveBeenCalledTimes(1);
  });

  test.each([
    { authorization: "ready" },
    { nativeStart: "unknown" },
    { restorationAttemptRequested: "true" },
    { revision: -1 },
    { revision: 1.5 },
    { revision: Number.MAX_SAFE_INTEGER + 1 },
  ])("rejects malformed native status %j", (invalid) => {
    expect(() =>
      parseBluetoothAuthorizationStatus(JSON.stringify({ ...readyStatus, ...invalid })),
    ).toThrow(NativePayloadError);
  });
  test.each(["null", "[]", "true", "invalid"])("rejects non-object payload %s", (json) => {
    expect(() => parseBluetoothAuthorizationStatus(json)).toThrow(NativePayloadError);
  });
});
