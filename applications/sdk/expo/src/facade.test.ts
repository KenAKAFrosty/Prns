import { destinationHash, identityHash } from "personal-rns/contract";
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
  lxmf: { state: "stopped", inboundOverflowCount: "0" },
  controllerIdentityFingerprint: null,
  pairing: { type: "searching" },
  pairingCandidates: [],
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
    saveObservedDestination: jest.fn(async () => JSON.stringify({ type: "notObserved" })),
    createManualContact: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    setContactAlias: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    setContactPinned: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    deleteContact: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    getContact: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    listContacts: jest.fn(async () => JSON.stringify({ type: "listed", contacts: [] })),
    listLxmfPeers: jest.fn(async () => JSON.stringify({ type: "listed", peers: [] })),
    listLxmfMessages: jest.fn(async () => JSON.stringify({ type: "listed", messages: [] })),
    retryLxmfMessage: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    cancelLxmfMessage: jest.fn(async () => JSON.stringify({ type: "notFound" })),
    announceLxmf: jest.fn(async () => JSON.stringify({ type: "announced" })),
    measureLxmfText: jest.fn(async () =>
      JSON.stringify({ type: "measured", wireBytes: 113, remainingBytes: 318 }),
    ),
    sendDirectText: jest.fn(async () => JSON.stringify({ type: "accepted", localRecordId: "1" })),
    stop: jest.fn(async () => JSON.stringify({ type: "alreadyStopped" })),
    reset: jest.fn(async () => JSON.stringify({ type: "alreadyStopped" })),
    ...overrides,
  };
}

describe("development runtime facade", () => {
  test("checks the native contract once and hydrates every snapshot result", async () => {
    const native = fakeNative();
    const runtime = createDevelopmentRuntime(native);

    const started = await runtime.startDevelopmentNode({ developmentTcpTarget: null });
    const current = await runtime.readDevelopmentNodeSnapshot();

    expect(started.type).toBe("started");
    if (started.type === "started") {
      expect(started.snapshot.revision).toBe(0n);
    }
    expect(current.revision).toBe(0n);
    expect(native.contractFingerprint).toHaveBeenCalledTimes(1);
    expect(native.hostContractFingerprint).toHaveBeenCalledTimes(1);
    expect(native.start).toHaveBeenCalledWith(JSON.stringify({ developmentTcpTarget: null }));
  });

  test("hydrates the generated multi-candidate catalog through the runtime facade", async () => {
    const native = fakeNative({
      snapshot: jest.fn(async () =>
        JSON.stringify({
          ...snapshot,
          pairingCandidates: NATIVE_CONTRACT_FIXTURES.pairingCandidates,
        }),
      ),
    });

    const current = await createDevelopmentRuntime(native).readDevelopmentNodeSnapshot();

    expect(current.pairingCandidates).toEqual([
      {
        candidateId: "candidate-fixture",
        displayName: "Fixture node",
        observedAtMillis: 9_007_199_254_740_991n,
        expiresAtMillis: 9_007_199_254_740_992n,
        expiresInMillis: 1n,
      },
      {
        candidateId: "unnamed-candidate-fixture",
        displayName: null,
        observedAtMillis: 17n,
        expiresAtMillis: 99n,
        expiresInMillis: 82n,
      },
    ]);
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

    await expect(
      runtime.startDevelopmentNode({ developmentTcpTarget: null }),
    ).rejects.toBeInstanceOf(NativeContractMismatchError);
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

  test("serializes every contact input explicitly and hydrates contact identities", async () => {
    const destination = destinationHash(Uint8Array.from({ length: 16 }, (_, index) => index));
    const identity = identityHash(Uint8Array.from({ length: 16 }, (_, index) => index + 16));
    const createManualContact = jest.fn(async () =>
      JSON.stringify({
        type: "saved",
        contact: {
          destination: Array.from(destination),
          identity: Array.from(identity),
          alias: null,
          pinned: false,
        },
      }),
    );
    const setContactAlias = jest.fn(async () => JSON.stringify({ type: "notFound" }));
    const runtime = createDevelopmentRuntime(fakeNative({ createManualContact, setContactAlias }));

    const created = await runtime.createManualContact(destination, identity, null);
    await runtime.setContactAlias(destination, null);

    expect(createManualContact).toHaveBeenCalledWith(
      JSON.stringify({
        destination: Array.from(destination),
        identity: Array.from(identity),
        alias: null,
      }),
    );
    expect(setContactAlias).toHaveBeenCalledWith(
      JSON.stringify({ destination: Array.from(destination), alias: null }),
    );
    expect(created.type === "saved" ? created.contact.destination : null).toBeInstanceOf(
      Uint8Array,
    );
    expect(created.type === "saved" ? created.contact.identity : null).toBeInstanceOf(Uint8Array);
  });

  test("serializes LXMF inputs and hydrates message bytes and exact u64 values", async () => {
    const peer = destinationHash(Uint8Array.from({ length: 16 }, (_, index) => index));
    const listLxmfMessages = jest.fn(async () =>
      JSON.stringify({
        type: "listed",
        messages: [
          {
            localRecordId: "18446744073709551615",
            messageId: Array(32).fill(0x44),
            source: Array.from(peer),
            destination: Array.from(peer),
            timestamp: "1700000000000",
            title: { type: "utf8", value: "Hello" },
            content: { type: "invalidUtf8", bytes: [0xff] },
            direction: "inbound",
            verification: "sourceUnknown",
            deliveryState: {
              type: "failed",
              failedAttempts: "9007199254740992",
              lastFailure: "deliveryTimedOut",
            },
          },
        ],
      }),
    );
    const sendDirectText = jest.fn(async () =>
      JSON.stringify({ type: "accepted", localRecordId: "9" }),
    );
    const retryLxmfMessage = jest.fn(async () =>
      JSON.stringify({ type: "accepted", localRecordId: "10" }),
    );
    const cancelLxmfMessage = jest.fn(async () =>
      JSON.stringify({
        type: "notCancellable",
        current: {
          type: "delivered",
          deliveredAt: "18446744073709551615",
          rtt: "23",
        },
      }),
    );
    const runtime = createDevelopmentRuntime(
      fakeNative({
        listLxmfMessages,
        retryLxmfMessage,
        cancelLxmfMessage,
        sendDirectText,
      }),
    );

    const listed = await runtime.listLxmfMessages({ peer, before: 9n, limit: 25 });
    const retried = await runtime.retryLxmfMessage(10n);
    const cancelled = await runtime.cancelLxmfMessage(11n);
    const sent = await runtime.sendDirectText({
      destination: peer,
      title: "Hello",
      content: "World",
    });

    expect(listLxmfMessages).toHaveBeenCalledWith(
      JSON.stringify({ peer: Array.from(peer), before: "9", limit: 25 }),
    );
    expect(retryLxmfMessage).toHaveBeenCalledWith(JSON.stringify({ localRecordId: "10" }));
    expect(cancelLxmfMessage).toHaveBeenCalledWith(JSON.stringify({ localRecordId: "11" }));
    expect(sendDirectText).toHaveBeenCalledWith(
      JSON.stringify({
        destination: Array.from(peer),
        title: "Hello",
        content: "World",
      }),
    );
    expect(sent).toEqual({ type: "accepted", localRecordId: 9n });
    expect(retried).toEqual({ type: "accepted", localRecordId: 10n });
    expect(cancelled).toEqual({
      type: "notCancellable",
      current: {
        type: "delivered",
        deliveredAt: 18_446_744_073_709_551_615n,
        rtt: 23n,
      },
    });
    expect(listed.type).toBe("listed");
    if (listed.type !== "listed") {
      throw new Error("LXMF fixture was not listed");
    }
    expect(listed.messages[0]?.localRecordId).toBe(18_446_744_073_709_551_615n);
    expect(listed.messages[0]?.messageId).toEqual(Uint8Array.from({ length: 32 }, () => 0x44));
    expect(listed.messages[0]?.source).toEqual(peer);
    expect(listed.messages[0]?.content).toEqual({
      type: "invalidUtf8",
      bytes: Uint8Array.of(0xff),
    });
    expect(listed.messages[0]?.deliveryState).toEqual({
      type: "failed",
      failedAttempts: 9_007_199_254_740_992n,
      lastFailure: "deliveryTimedOut",
    });
  });
});
