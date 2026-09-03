import { NATIVE_CONTRACT_FINGERPRINT } from "./contract.generated";
import { hydrateGenerated, NativePayloadError } from "./hydrate";

describe("generated native payload hydration", () => {
  test("hydrates exact integers and Prns byte brands without sharing wire arrays", () => {
    const targetIdentityFingerprint = Array.from({ length: 16 }, (_, index) => index);
    const destination = Array.from({ length: 16 }, (_, index) => index + 16);
    const controllerIdentityFingerprint = Array.from({ length: 16 }, (_, index) => index + 32);
    const wire = {
      contractFingerprint: NATIVE_CONTRACT_FINGERPRINT,
      revision: "18446744073709551615",
      runtime: "running" as const,
      primaryIdentity: { type: "present" as const, identityHash: targetIdentityFingerprint },
      localHost: {
        type: "running" as const,
        host: {
          revision: "7",
          backend: {
            backend: "Native",
            capabilities: ["Bluetooth"],
            interfaceKinds: ["AutomaticBluetoothLe"],
          },
          interfaces: [
            {
              interfaceId: Array(8).fill(0x44),
              kind: "AutomaticBluetoothLe",
              health: "Connected",
              rxBytes: "18446744073709551615",
              txBytes: "9007199254740992",
              routeCount: 1,
              linkCount: 0,
              transportedLinkCount: 0,
            },
          ],
          routes: [
            {
              destination,
              hops: 1,
              viaIdentity: targetIdentityFingerprint,
              interfaceId: Array(8).fill(0x44),
              learnedAtMillis: 11,
              lastRouteActivityAtMillis: 12,
              expiresAtMillis: 13,
            },
          ],
          activeLinkCount: 0,
          destinationIdentities: [{ destination, identity: targetIdentityFingerprint }],
          runtime: {
            running: true,
            uptimeMillis: 14,
            interfaceCount: 1,
            onlineInterfaceCount: 1,
            routeCount: 1,
            linkCount: 0,
            transportedLinkCount: 0,
            rxBytes: "18446744073709551615",
            txBytes: "9007199254740992",
            rxBps: 0,
            txBps: 0,
          },
          persistence: { persistent: true, restored: true, lastFlushCause: "Startup" },
        },
      },
      controllerIdentityFingerprint,
      pairing: {
        type: "candidateObserved" as const,
        candidate: {
          candidateId: "candidate",
          endpoint: destination,
          observedAtMillis: "9007199254740991",
          expiresAtMillis: "9007199254740992",
          publicAppData: [0, 127, 255],
        },
      },
      pairedTargets: [
        {
          targetIdentityFingerprint,
          destination,
          controllerIdentityFingerprint,
          permittedRequests: ["describe" as const],
        },
      ],
      activeOperation: { kind: "pairing" as const, startedAtMillis: "0" },
      failure: null,
    };

    const hydrated = hydrateGenerated(wire);

    expect(hydrated.revision).toBe(18_446_744_073_709_551_615n);
    expect(hydrated.pairing.candidate.observedAtMillis).toBe(9_007_199_254_740_991n);
    expect(hydrated.pairing.candidate.expiresAtMillis).toBe(9_007_199_254_740_992n);
    expect(hydrated.activeOperation.startedAtMillis).toBe(0n);
    expect(hydrated.primaryIdentity.identityHash).toBeInstanceOf(Uint8Array);
    expect(hydrated.localHost.host.revision).toBe(7n);
    expect(hydrated.localHost.host.interfaces[0]?.interfaceId).toBeInstanceOf(Uint8Array);
    expect(hydrated.localHost.host.interfaces[0]?.rxBytes).toBe(18_446_744_073_709_551_615n);
    expect(hydrated.localHost.host.routes[0]?.expiresAtMillis).toBe(13);
    expect(hydrated.localHost.host.routes[0]?.viaIdentity).toBeInstanceOf(Uint8Array);
    expect(hydrated.localHost.host.destinationIdentities[0]?.identity).toBeInstanceOf(Uint8Array);
    expect(hydrated.pairing.candidate.endpoint).toBeInstanceOf(Uint8Array);
    expect(hydrated.pairing.candidate.publicAppData).toEqual(Uint8Array.of(0, 127, 255));
    expect(hydrated.pairedTargets[0]?.targetIdentityFingerprint).toBeInstanceOf(Uint8Array);

    targetIdentityFingerprint[0] = 255;
    expect(hydrated.pairedTargets[0]?.targetIdentityFingerprint[0]).toBe(0);
  });

  test.each(["", "00", "01", "-1", "+1", "1.0", " 1", "18446744073709551616"])(
    "rejects non-canonical or out-of-range u64 value %p",
    (revision) => {
      expect(() => hydrateGenerated({ revision })).toThrow(NativePayloadError);
    },
  );

  test("rejects non-byte values before Uint8Array can coerce them", () => {
    expect(() => hydrateGenerated({ publicAppData: [0, 256] })).toThrow(NativePayloadError);
    expect(() => hydrateGenerated({ publicAppData: [0, 1.5] })).toThrow(NativePayloadError);
    expect(() => hydrateGenerated({ publicAppData: [0, "1"] })).toThrow(NativePayloadError);
  });

  test.each([31, 33])("rejects a %i-byte LXMF message ID", (length) => {
    expect(() => hydrateGenerated({ messageId: Array(length).fill(0) })).toThrow(
      NativePayloadError,
    );
  });

  test("keeps invalid UTF-8 diagnostic bytes variable-length", () => {
    expect(hydrateGenerated({ bytes: [0xff] }).bytes).toEqual(Uint8Array.of(0xff));
    expect(hydrateGenerated({ bytes: Array(64).fill(0x80) }).bytes).toHaveLength(64);
  });

  test("delegates fixed hash lengths to the provider-neutral Prns contract", () => {
    expect(() => hydrateGenerated({ targetIdentityFingerprint: Array(15).fill(0) })).toThrow(
      NativePayloadError,
    );
    expect(() => hydrateGenerated({ destination: Array(17).fill(0) })).toThrow(NativePayloadError);
    expect(() => hydrateGenerated({ interfaceId: Array(7).fill(0) })).toThrow(NativePayloadError);
  });

  test("hydrates contact and identity-conflict fields with their canonical brands", () => {
    const hydrated = hydrateGenerated({
      contact: {
        destination: Array(16).fill(1),
        identity: Array(16).fill(2),
        alias: null,
        pinned: true,
      },
      existing: Array(16).fill(3),
      attempted: Array(16).fill(4),
    });

    expect(hydrated.contact.destination).toBeInstanceOf(Uint8Array);
    expect(hydrated.contact.identity).toBeInstanceOf(Uint8Array);
    expect(hydrated.existing).toBeInstanceOf(Uint8Array);
    expect(hydrated.attempted).toBeInstanceOf(Uint8Array);
  });

  test("hydrates every durable LXMF delivery timestamp and attempt count as bigint", () => {
    const hydrated = hydrateGenerated({
      queued: { type: "queued", failedAttempts: "18446744073709551615" },
      sending: { type: "sending", failedAttempts: "9007199254740992" },
      delivered: {
        type: "delivered",
        deliveredAt: "1700000000123",
        rtt: "23",
      },
      deliveredWithoutRtt: {
        type: "delivered",
        deliveredAt: "1700000000124",
        rtt: null,
      },
      cancelled: { type: "cancelled", cancelledAt: "1700000000456" },
    });

    expect(hydrated.queued.failedAttempts).toBe(18_446_744_073_709_551_615n);
    expect(hydrated.sending.failedAttempts).toBe(9_007_199_254_740_992n);
    expect(hydrated.delivered.deliveredAt).toBe(1_700_000_000_123n);
    expect(hydrated.delivered.rtt).toBe(23n);
    expect(hydrated.deliveredWithoutRtt.rtt).toBeNull();
    expect(hydrated.cancelled.cancelledAt).toBe(1_700_000_000_456n);
  });

  test.each(["failedAttempts", "deliveredAt", "rtt", "cancelledAt"])(
    "rejects a non-canonical durable LXMF u64 in %s",
    (key) => {
      expect(() => hydrateGenerated({ [key]: "01" })).toThrow(NativePayloadError);
    },
  );

  test("keeps Host route deadlines as safe numbers while pairing deadlines are bigint", () => {
    expect(hydrateGenerated({ expiresAtMillis: "13" }).expiresAtMillis).toBe(13n);
    expect(hydrateGenerated({ routes: [{ expiresAtMillis: 13 }] }).routes[0]?.expiresAtMillis).toBe(
      13,
    );
    expect(() =>
      hydrateGenerated({ routes: [{ expiresAtMillis: Number.MAX_SAFE_INTEGER + 1 }] }),
    ).toThrow(NativePayloadError);
  });
});
