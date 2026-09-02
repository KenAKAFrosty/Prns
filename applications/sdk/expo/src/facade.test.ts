import { identityHash } from "personal-rns/contract";
import { NATIVE_CONTRACT_FINGERPRINT } from "./contract.generated";
import { createDevelopmentRuntime, NativeBridgeError, NativeContractMismatchError } from "./facade";
import type { PrnsAppNativeModule } from "./native";

const snapshot = {
  contractFingerprint: NATIVE_CONTRACT_FINGERPRINT,
  revision: "0",
  runtime: "stopped",
  bluetooth: { type: "notCompiled" },
  controllerIdentityFingerprint: null,
  pairing: { type: "searching" },
  pairedTargets: [],
  activeOperation: null,
  failure: null,
};

function fakeNative(overrides: Partial<PrnsAppNativeModule> = {}): PrnsAppNativeModule {
  return {
    contractFingerprint: jest.fn(async () => NATIVE_CONTRACT_FINGERPRINT),
    start: jest.fn(async () => JSON.stringify({ type: "started", snapshot })),
    snapshot: jest.fn(async () => JSON.stringify(snapshot)),
    initiatePairing: jest.fn(async () => JSON.stringify({ type: "busy" })),
    approvePairing: jest.fn(async () => JSON.stringify({ type: "busy" })),
    rejectPairing: jest.fn(async () => JSON.stringify({ type: "busy" })),
    describeTarget: jest.fn(async () => JSON.stringify({ type: "busy" })),
    stop: jest.fn(async () => JSON.stringify({ type: "alreadyStopped" })),
    reset: jest.fn(async () => JSON.stringify({ type: "alreadyStopped" })),
    ...overrides,
  };
}

describe("development runtime facade", () => {
  test("checks the native contract once and hydrates every snapshot result", async () => {
    const native = fakeNative();
    const runtime = createDevelopmentRuntime(native);

    const started = await runtime.startDevelopmentNode();
    const current = await runtime.readDevelopmentNodeSnapshot();

    expect(started.type).toBe("started");
    if (started.type === "started") {
      expect(started.snapshot.revision).toBe(0n);
    }
    expect(current.revision).toBe(0n);
    expect(native.contractFingerprint).toHaveBeenCalledTimes(1);
  });

  test("refuses a stale native library before invoking an operation", async () => {
    const native = fakeNative({ contractFingerprint: jest.fn(async () => "stale-contract") });
    const runtime = createDevelopmentRuntime(native);

    await expect(runtime.startDevelopmentNode()).rejects.toBeInstanceOf(
      NativeContractMismatchError,
    );
    expect(native.start).not.toHaveBeenCalled();
  });

  test("also rejects a stale fingerprint embedded in a returned snapshot", async () => {
    const native = fakeNative({
      snapshot: jest.fn(async () =>
        JSON.stringify({ ...snapshot, contractFingerprint: "stale-snapshot" }),
      ),
    });

    await expect(
      createDevelopmentRuntime(native).readDevelopmentNodeSnapshot(),
    ).rejects.toBeInstanceOf(NativeContractMismatchError);
  });

  test("turns private C ABI failure envelopes into bridge errors", async () => {
    const native = fakeNative({
      snapshot: jest.fn(async () =>
        JSON.stringify({
          type: "bridgeFailure",
          kind: "invalidInput",
          detail: "contract input is too large",
        }),
      ),
    });

    await expect(
      createDevelopmentRuntime(native).readDevelopmentNodeSnapshot(),
    ).rejects.toBeInstanceOf(NativeBridgeError);
  });

  test("serializes branded target identities back to honest JSON arrays", async () => {
    const describeTarget = jest.fn(async () => JSON.stringify({ type: "busy" }));
    const runtime = createDevelopmentRuntime(fakeNative({ describeTarget }));
    const targetIdentityFingerprint = identityHash(Uint8Array.from({ length: 16 }, (_, i) => i));

    await runtime.describeRemoteControlTarget({ targetIdentityFingerprint });

    expect(describeTarget).toHaveBeenCalledWith(
      JSON.stringify({ targetIdentityFingerprint: Array.from(targetIdentityFingerprint) }),
    );
  });
});
