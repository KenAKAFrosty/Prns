import { NATIVE_CONTRACT_FIXTURES } from "./contract.generated";

describe("Rust-generated contract fixtures", () => {
  test("every fixture survives the exact JSON bridge projection", () => {
    for (const fixtures of Object.values(NATIVE_CONTRACT_FIXTURES)) {
      for (const expected of fixtures) {
        const parsed: unknown = JSON.parse(JSON.stringify(expected));
        expect(parsed).toEqual(expected);
      }
    }
  });

  test("covers every consumed tagged variant through local-node L1", () => {
    expect(NATIVE_CONTRACT_FIXTURES.primaryIdentityStates.map(({ type }) => type)).toEqual([
      "missing",
      "present",
      "unavailable",
      "developmentResetRequired",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.localHostStates.map(({ type }) => type)).toEqual([
      "stopped",
      "stopped",
      "running",
      "unavailable",
      "developmentResetRequired",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.identityImportPreviewOutcomes.map(({ type }) => type)).toEqual([
      "valid",
      "invalidLength",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.identityCreationOutcomes.map(({ type }) => type)).toEqual([
      "created",
      "alreadyExists",
      "invalidLength",
      "unavailable",
      "developmentResetRequired",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.contactMutationOutcomes.map(({ type }) => type)).toEqual([
      "saved",
      "updated",
      "deleted",
      "existing",
      "alreadyExists",
      "notFound",
      "localNodeStopped",
      "notObserved",
      "identityConflict",
      "missingIdentity",
      "developmentUnavailable",
      "developmentResetRequired",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.contactLookupOutcomes.map(({ type }) => type)).toEqual([
      "found",
      "notFound",
      "developmentUnavailable",
      "developmentResetRequired",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.contactListOutcomes.map(({ type }) => type)).toEqual([
      "listed",
      "developmentUnavailable",
      "developmentResetRequired",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.pairingStates.map(({ type }) => type)).toEqual([
      "bluetoothUnavailable",
      "searching",
      "candidateObserved",
      "invitationSubmitted",
      "confirmationRequired",
      "awaitingTargetApproval",
      "persisting",
      "paired",
      "rejected",
      "expired",
      "cancelled",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.startOutcomes.map(({ type }) => type)).toEqual([
      "started",
      "alreadyRunning",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.stopOutcomes.map(({ type }) => type)).toEqual([
      "stopped",
      "alreadyStopped",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.pairingOutcomes.map(({ type }) => type)).toEqual([
      "accepted",
      "busy",
      "failed",
    ]);
    expect(NATIVE_CONTRACT_FIXTURES.describeOutcomes.map(({ type }) => type)).toEqual([
      "described",
      "busy",
      "failed",
    ]);
  });
});
