import { createAndroidRuntime, parseAndroidRuntimeStatus } from "./android-runtime";
import { NativePayloadError } from "./hydrate";
import type { AndroidRuntimeNativeModule } from "./native";

const status = {
  revision: 1,
  bluetoothPermission: "denied",
  backgroundDiscovery: "notGranted",
  bluetoothRadio: "off",
  locationServices: "off",
  service: "running",
  lastError: null,
} as const;

function fixture(overrides: Partial<AndroidRuntimeNativeModule> = {}): AndroidRuntimeNativeModule {
  return {
    androidRuntimeStatus: jest.fn(async () => JSON.stringify(status)),
    requestBluetoothPermissions: jest.fn(async () => JSON.stringify(status)),
    requestBackgroundBluetoothPermission: jest.fn(async () => JSON.stringify(status)),
    addListener: jest.fn(() => ({ remove: jest.fn() })),
    ...overrides,
  };
}

describe("Android runtime capability", () => {
  test("keeps a running node distinct from unavailable Bluetooth and never prompts on a read", async () => {
    const native = fixture();
    const runtime = createAndroidRuntime(native);
    await expect(runtime.readStatus()).resolves.toEqual(status);
    expect(native.requestBluetoothPermissions).not.toHaveBeenCalled();
    expect(native.requestBackgroundBluetoothPermission).not.toHaveBeenCalled();
    await runtime.requestBluetoothPermissions();
    expect(native.requestBluetoothPermissions).toHaveBeenCalledTimes(1);
    expect(native.requestBackgroundBluetoothPermission).not.toHaveBeenCalled();
    await runtime.requestBackgroundBluetoothPermission();
    expect(native.requestBackgroundBluetoothPermission).toHaveBeenCalledTimes(1);
  });

  test("subscribes to Android events only and ignores malformed events", () => {
    let listener: ((event: { status: string }) => void) | undefined;
    const remove = jest.fn();
    const native = fixture({
      addListener: jest.fn((event, callback) => {
        expect(event).toBe("onAndroidRuntimeStatus");
        listener = callback;
        return { remove };
      }),
    });
    const receive = jest.fn();
    const subscription = createAndroidRuntime(native).addStatusListener(receive);
    listener?.({ status: JSON.stringify(status) });
    listener?.({ status: "{}" });
    subscription.remove();
    expect(receive).toHaveBeenCalledTimes(1);
    expect(receive).toHaveBeenCalledWith(status);
    expect(remove).toHaveBeenCalledTimes(1);
  });

  test.each([
    { revision: -1 },
    { revision: Number.MAX_SAFE_INTEGER + 1 },
    { bluetoothPermission: "authorized" },
    { backgroundDiscovery: "unknown" },
    { bluetoothRadio: "ready" },
    { locationServices: "unknown" },
    { service: "ready" },
    { lastError: 0 },
  ])("rejects malformed status %j", (invalid) => {
    expect(() => parseAndroidRuntimeStatus(JSON.stringify({ ...status, ...invalid }))).toThrow(
      NativePayloadError,
    );
  });

  test.each(["null", "[]", "not JSON", "{}"])("rejects missing status %s", (value) => {
    expect(() => parseAndroidRuntimeStatus(value)).toThrow(NativePayloadError);
  });
});
