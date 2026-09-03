import { identityHash } from "personal-rns/contract";
import {
  HOST_CONTRACT_FINGERPRINT,
  NATIVE_CONTRACT_FINGERPRINT,
  NATIVE_CONTRACT_FIXTURES,
} from "./contract.generated";
import { createDevelopmentRuntime, NativeBridgeError, NativeContractMismatchError } from "./facade";
import type { PrnsAppNativeModule } from "./native";

const snapshot = {
  contractFingerprint: NATIVE_CONTRACT_FINGERPRINT,
  revision: "0",
  runtime: "stopped",
  primaryIdentity: { type: "missing" },
  localHost: { type: "stopped", lastStartFailure: null },
  controllerIdentityFingerprint: null,
  pairing: { type: "searching" },
  pairedTargets: [],
  activeOperation: null,
  failure: null,
};

function fakeNative(overrides: Partial<PrnsAppNativeModule> = {}): PrnsAppNativeModule {
  return {
    contractFingerprint: jest.fn(async () => NATIVE_CONTRACT_FINGERPRINT),
    hostContractFingerprint: jest.fn(async () => HOST_CONTRACT_FINGERPRINT),
    inspectIdentity: jest.fn(async () => JSON.stringify({ type: "missing" })),
    previewIdentityImport: jest.fn(async () => JSON.stringify({ type: "invalidLength" })),
    createGeneratedIdentity: jest.fn(async () => JSON.stringify({ type: "alreadyExists" })),
    createImportedIdentity: jest.fn(async () => JSON.stringify({ type: "alreadyExists" })),
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
    expect(native.hostContractFingerprint).toHaveBeenCalledTimes(1);
  });

  test("hydrates every canonical Host scalar from the Rust-generated running fixture", async () => {
    const runningHost = NATIVE_CONTRACT_FIXTURES.localHostStates.find(
      (state) => state.type === "running",
    );
    if (runningHost?.type !== "running") {
      throw new Error("Rust contract fixtures did not include a running Host");
    }
    const native = fakeNative({
      snapshot: jest.fn(async () =>
        JSON.stringify({
          ...snapshot,
          revision: "18446744073709551615",
          runtime: "running",
          localHost: runningHost,
        }),
      ),
    });

    const current = await createDevelopmentRuntime(native).readDevelopmentNodeSnapshot();

    expect(current.revision).toBe(18_446_744_073_709_551_615n);
    expect(current.localHost.type).toBe("running");
    if (current.localHost.type !== "running") {
      throw new Error("hydrated fixture did not retain its running Host variant");
    }
    const host = current.localHost.host;
    expect(host.revision).toBe(18_446_744_073_709_551_615n);
    expect(host.backend).toEqual({
      backend: "Native",
      capabilities: ["Bluetooth"],
      interfaceKinds: ["AutomaticBluetoothLe"],
    });
    expect(host.interfaces).toHaveLength(1);
    expect(host.interfaces[0]).toMatchObject({
      name: "Bluetooth Auto",
      kind: "AutomaticBluetoothLe",
      health: "Connected",
      rxBytes: 18_446_744_073_709_551_615n,
      txBytes: 9_007_199_254_740_992n,
      rxBps: 7,
      txBps: 8,
      routeCount: 1,
      linkCount: 2,
      transportedLinkCount: 3,
    });
    expect(host.interfaces[0]?.interfaceId).toEqual(Uint8Array.from({ length: 8 }, () => 0x44));
    expect(host.routes).toHaveLength(1);
    expect(host.routes[0]).toMatchObject({
      hops: 1,
      learnedAtMillis: 11,
      lastRouteActivityAtMillis: 12,
      expiresAtMillis: 13,
    });
    expect(host.routes[0]?.destination).toEqual(Uint8Array.from({ length: 16 }, () => 0x55));
    expect(host.routes[0]?.viaIdentity).toEqual(Uint8Array.from({ length: 16 }, () => 0x66));
    expect(host.routes[0]?.interfaceId).toEqual(Uint8Array.from({ length: 8 }, () => 0x44));
    expect(host.activeLinkCount).toBe(2);
    expect(host.destinationIdentities).toHaveLength(1);
    expect(host.destinationIdentities[0]?.destination).toEqual(
      Uint8Array.from({ length: 16 }, () => 0x55),
    );
    expect(host.destinationIdentities[0]?.identity).toEqual(
      Uint8Array.from({ length: 16 }, () => 0x77),
    );
    expect(host.runtime).toEqual({
      running: true,
      uptimeMillis: 14,
      interfaceCount: 1,
      onlineInterfaceCount: 1,
      routeCount: 1,
      linkCount: 2,
      transportedLinkCount: 3,
      rxBytes: 18_446_744_073_709_551_615n,
      txBytes: 9_007_199_254_740_992n,
      rxBps: 7,
      txBps: 8,
    });
    expect(host.persistence).toEqual({
      persistent: true,
      restored: true,
      lastFlushCause: "Startup",
    });
  });

  test("refuses a stale canonical Host contract before invoking an operation", async () => {
    const native = fakeNative({
      hostContractFingerprint: jest.fn(async () => "stale-host-contract"),
    });
    const runtime = createDevelopmentRuntime(native);

    await expect(runtime.readDevelopmentNodeSnapshot()).rejects.toBeInstanceOf(
      NativeContractMismatchError,
    );
    expect(native.snapshot).not.toHaveBeenCalled();
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

  test("passes identity credentials as byte arrays and hydrates the derived hash", async () => {
    const identity = Uint8Array.from({ length: 64 }, (_, index) => index);
    const derived = Array(16).fill(0x42);
    const previewIdentityImport = jest.fn(async () =>
      JSON.stringify({ type: "valid", identityHash: derived }),
    );
    const createImportedIdentity = jest.fn(async () =>
      JSON.stringify({ type: "created", identityHash: derived }),
    );
    const runtime = createDevelopmentRuntime(
      fakeNative({ previewIdentityImport, createImportedIdentity }),
    );

    const preview = await runtime.previewIdentityImport(identity);
    const created = await runtime.createImportedIdentity(identity);

    expect(previewIdentityImport).toHaveBeenCalledWith(Array.from(identity));
    expect(createImportedIdentity).toHaveBeenCalledWith(Array.from(identity));
    expect(preview.type === "valid" ? preview.identityHash : null).toBeInstanceOf(Uint8Array);
    expect(created.type === "created" ? created.identityHash : null).toBeInstanceOf(Uint8Array);
  });
});
