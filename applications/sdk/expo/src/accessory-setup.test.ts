import { NativePayloadError } from "./native-payload";
import { createAccessorySetupRuntime, parseAccessorySetupStatus } from "./accessory-setup";
import type { AppleAccessorySetupNativeModule } from "./native";

const readyStatus = JSON.stringify({
  phase: "ready",
  picker: "idle",
  authorizedAccessoryCount: 2,
  nativeStart: "running",
  restorationLaunchRequested: true,
  revision: 4,
  lastError: null,
});

function nativeSetupFixture(
  overrides: Partial<AppleAccessorySetupNativeModule> = {},
): AppleAccessorySetupNativeModule {
  return {
    accessorySetupStatus: jest.fn(async () => readyStatus),
    showAccessorySetupPicker: jest.fn(async () => JSON.stringify({ type: "completed" })),
    addListener: jest.fn(() => ({ remove: jest.fn() })),
    ...overrides,
  };
}

describe("app-owned AccessorySetupKit bridge", () => {
  test("parses the privacy-safe native status without accessory identifiers", async () => {
    const status = await createAccessorySetupRuntime(nativeSetupFixture()).readStatus();

    expect(status).toEqual({
      phase: "ready",
      picker: "idle",
      authorizedAccessoryCount: 2,
      nativeStart: "running",
      restorationLaunchRequested: true,
      revision: 4,
      lastError: null,
    });
    expect(JSON.stringify(status)).not.toMatch(/[0-9a-f]{8}-[0-9a-f-]{27}/iu);
  });

  test("delivers status changes and returns a removable subscription", () => {
    let nativeListener: ((event: { readonly status: string }) => void) | undefined;
    const remove = jest.fn();
    const runtime = createAccessorySetupRuntime(
      nativeSetupFixture({
        addListener: jest.fn((_name, listener) => {
          nativeListener = listener;
          return { remove };
        }),
      }),
    );
    const listener = jest.fn();

    const subscription = runtime.addStatusListener(listener);
    nativeListener?.({
      status: JSON.stringify({
        phase: "setupRequired",
        picker: "presented",
        authorizedAccessoryCount: 0,
        nativeStart: "notRequested",
        restorationLaunchRequested: false,
        revision: 5,
        lastError: null,
      }),
    });
    subscription.remove();

    expect(listener).toHaveBeenCalledWith(
      expect.objectContaining({ phase: "setupRequired", picker: "presented" }),
    );
    expect(remove).toHaveBeenCalledTimes(1);
  });

  test.each(["cancelled", "timedOut", "restricted", "alreadyActive"] as const)(
    "preserves the typed %s picker outcome",
    async (type) => {
      const runtime = createAccessorySetupRuntime(
        nativeSetupFixture({
          showAccessorySetupPicker: jest.fn(async () => JSON.stringify({ type })),
        }),
      );

      await expect(runtime.showPicker()).resolves.toEqual({ type });
    },
  );

  test("rejects malformed native status instead of guessing authorization", () => {
    expect(() =>
      parseAccessorySetupStatus(
        JSON.stringify({
          phase: "ready",
          picker: "idle",
          authorizedAccessoryCount: -1,
          nativeStart: "running",
          restorationLaunchRequested: false,
          revision: 0,
          lastError: null,
        }),
      ),
    ).toThrow(NativePayloadError);
  });
});
