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
      bluetooth: { type: "ready" as const },
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

  test("delegates fixed hash lengths to the provider-neutral Prns contract", () => {
    expect(() => hydrateGenerated({ targetIdentityFingerprint: Array(15).fill(0) })).toThrow(
      NativePayloadError,
    );
    expect(() => hydrateGenerated({ destination: Array(17).fill(0) })).toThrow(NativePayloadError);
  });
});
