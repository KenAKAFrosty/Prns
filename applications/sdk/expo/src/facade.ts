import { HOST_CONTRACT_FINGERPRINT, NATIVE_CONTRACT_FINGERPRINT } from "./contract.generated";
import type {
  AnnounceLxmfOutcome as WireAnnounceLxmfOutcome,
  CancelLxmfMessageInput as WireCancelLxmfMessageInput,
  CancelLxmfMessageOutcome as WireCancelLxmfMessageOutcome,
  Contact as WireContact,
  ContactListOutcome as WireContactListOutcome,
  ContactLookupOutcome as WireContactLookupOutcome,
  ContactMutationOutcome as WireContactMutationOutcome,
  ContactDestinationInput as WireContactDestinationInput,
  CreateManualContactInput as WireCreateManualContactInput,
  DescribeRemoteControlTargetInput as WireDescribeRemoteControlTargetInput,
  DevelopmentNodeSnapshot as WireDevelopmentNodeSnapshot,
  DevelopmentNodeStartInput as WireDevelopmentNodeStartInput,
  DevelopmentNodeStartOutcome as WireDevelopmentNodeStartOutcome,
  DevelopmentNodeStopOutcome as WireDevelopmentNodeStopOutcome,
  IdentityCreationOutcome as WireIdentityCreationOutcome,
  IdentityImportPreviewOutcome as WireIdentityImportPreviewOutcome,
  InitiateRemoteControlPairingInput as WireInitiateRemoteControlPairingInput,
  ListLxmfMessagesInput as WireListLxmfMessagesInput,
  LxmfHealth as WireLxmfHealth,
  LxmfDeliveryFailure as WireLxmfDeliveryFailure,
  LxmfDeliveryState as WireLxmfDeliveryState,
  LxmfDirection as WireLxmfDirection,
  LxmfMessage as WireLxmfMessage,
  LxmfMessageListOutcome as WireLxmfMessageListOutcome,
  LxmfPeerListOutcome as WireLxmfPeerListOutcome,
  LxmfPeerSummary as WireLxmfPeerSummary,
  LxmfText as WireLxmfText,
  LxmfVerification as WireLxmfVerification,
  MeasureLxmfTextInput as WireMeasureLxmfTextInput,
  MeasureLxmfTextOutcome as WireMeasureLxmfTextOutcome,
  RemoteControlDescribeOutcome as WireRemoteControlDescribeOutcome,
  RemoteControlPairingCommandOutcome as WireRemoteControlPairingCommandOutcome,
  RemoteControlPairingDecisionInput as WireRemoteControlPairingDecisionInput,
  RetryLxmfMessageInput as WireRetryLxmfMessageInput,
  RetryLxmfMessageOutcome as WireRetryLxmfMessageOutcome,
  SetContactAliasInput as WireSetContactAliasInput,
  SetContactPinnedInput as WireSetContactPinnedInput,
  SendDirectTextInput as WireSendDirectTextInput,
  SendDirectTextOutcome as WireSendDirectTextOutcome,
  PrimaryIdentityState as WirePrimaryIdentityState,
} from "./contract.generated";
import type { DestinationHash, IdentityHash } from "personal-rns/contract";
import { hydrateGenerated, NativePayloadError, type Hydrated } from "./hydrate";
import type { PrnsAppNativeModule } from "./native";

type BridgeFailure = {
  readonly type: "bridgeFailure";
  readonly kind: "invalidInput" | "panic";
  readonly detail: string;
};

export type DevelopmentNodeSnapshot = Hydrated<WireDevelopmentNodeSnapshot>;
export type DevelopmentNodeStartInput = Hydrated<WireDevelopmentNodeStartInput>;
export type DevelopmentNodeStartOutcome = Hydrated<WireDevelopmentNodeStartOutcome>;
export type DevelopmentNodeStopOutcome = Hydrated<WireDevelopmentNodeStopOutcome>;
export type RemoteControlPairingCommandOutcome = Hydrated<WireRemoteControlPairingCommandOutcome>;
export type RemoteControlDescribeOutcome = Hydrated<WireRemoteControlDescribeOutcome>;
export type InitiateRemoteControlPairingInput = Hydrated<WireInitiateRemoteControlPairingInput>;
export type RemoteControlPairingDecisionInput = Hydrated<WireRemoteControlPairingDecisionInput>;
export type DescribeRemoteControlTargetInput = Hydrated<WireDescribeRemoteControlTargetInput>;
export type PrimaryIdentityState = Hydrated<WirePrimaryIdentityState>;
export type IdentityImportPreviewOutcome = Hydrated<WireIdentityImportPreviewOutcome>;
export type IdentityCreationOutcome = Hydrated<WireIdentityCreationOutcome>;
export type Contact = Hydrated<WireContact>;
export type ContactMutationOutcome = Hydrated<WireContactMutationOutcome>;
export type ContactLookupOutcome = Hydrated<WireContactLookupOutcome>;
export type ContactListOutcome = Hydrated<WireContactListOutcome>;
export type LxmfHealth = Hydrated<WireLxmfHealth>;
export type LxmfDeliveryFailure = Hydrated<WireLxmfDeliveryFailure>;
export type LxmfDeliveryState = Hydrated<WireLxmfDeliveryState>;
export type LxmfDirection = Hydrated<WireLxmfDirection>;
export type LxmfMessage = Hydrated<WireLxmfMessage>;
export type LxmfPeerSummary = Hydrated<WireLxmfPeerSummary>;
export type LxmfText = Hydrated<WireLxmfText>;
export type LxmfVerification = Hydrated<WireLxmfVerification>;
export type ListLxmfMessagesInput = Hydrated<WireListLxmfMessagesInput>;
export type LxmfMessageListOutcome = Hydrated<WireLxmfMessageListOutcome>;
export type LxmfPeerListOutcome = Hydrated<WireLxmfPeerListOutcome>;
export type AnnounceLxmfOutcome = Hydrated<WireAnnounceLxmfOutcome>;
export type MeasureLxmfTextInput = Hydrated<WireMeasureLxmfTextInput>;
export type MeasureLxmfTextOutcome = Hydrated<WireMeasureLxmfTextOutcome>;
export type SendDirectTextInput = Hydrated<WireSendDirectTextInput>;
export type SendDirectTextOutcome = Hydrated<WireSendDirectTextOutcome>;
export type RetryLxmfMessageOutcome = Hydrated<WireRetryLxmfMessageOutcome>;
export type CancelLxmfMessageOutcome = Hydrated<WireCancelLxmfMessageOutcome>;

export type DevelopmentRuntime = {
  readonly inspectDevelopmentIdentity: () => Promise<PrimaryIdentityState>;
  readonly previewIdentityImport: (identity: Uint8Array) => Promise<IdentityImportPreviewOutcome>;
  readonly createGeneratedIdentity: () => Promise<IdentityCreationOutcome>;
  readonly createImportedIdentity: (identity: Uint8Array) => Promise<IdentityCreationOutcome>;
  readonly startDevelopmentNode: (
    input: DevelopmentNodeStartInput,
  ) => Promise<DevelopmentNodeStartOutcome>;
  readonly readDevelopmentNodeSnapshot: () => Promise<DevelopmentNodeSnapshot>;
  readonly initiateRemoteControlPairing: (
    input: InitiateRemoteControlPairingInput,
  ) => Promise<RemoteControlPairingCommandOutcome>;
  readonly approveRemoteControlPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Promise<RemoteControlPairingCommandOutcome>;
  readonly rejectRemoteControlPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Promise<RemoteControlPairingCommandOutcome>;
  readonly describeRemoteControlTarget: (
    input: DescribeRemoteControlTargetInput,
  ) => Promise<RemoteControlDescribeOutcome>;
  readonly saveObservedDestination: (
    destination: DestinationHash,
  ) => Promise<ContactMutationOutcome>;
  readonly createManualContact: (
    destination: DestinationHash,
    identity: IdentityHash | null,
    alias: string | null,
  ) => Promise<ContactMutationOutcome>;
  readonly setContactAlias: (
    destination: DestinationHash,
    alias: string | null,
  ) => Promise<ContactMutationOutcome>;
  readonly setContactPinned: (
    destination: DestinationHash,
    pinned: boolean,
  ) => Promise<ContactMutationOutcome>;
  readonly deleteContact: (destination: DestinationHash) => Promise<ContactMutationOutcome>;
  readonly getContact: (destination: DestinationHash) => Promise<ContactLookupOutcome>;
  readonly listContacts: () => Promise<ContactListOutcome>;
  readonly listLxmfPeers: () => Promise<LxmfPeerListOutcome>;
  readonly listLxmfMessages: (input: ListLxmfMessagesInput) => Promise<LxmfMessageListOutcome>;
  readonly retryLxmfMessage: (localRecordId: bigint) => Promise<RetryLxmfMessageOutcome>;
  readonly cancelLxmfMessage: (localRecordId: bigint) => Promise<CancelLxmfMessageOutcome>;
  readonly announceLxmf: () => Promise<AnnounceLxmfOutcome>;
  readonly measureLxmfText: (input: MeasureLxmfTextInput) => Promise<MeasureLxmfTextOutcome>;
  readonly sendDirectText: (input: SendDirectTextInput) => Promise<SendDirectTextOutcome>;
  readonly stopDevelopmentNode: () => Promise<DevelopmentNodeStopOutcome>;
  readonly resetDevelopmentData: () => Promise<DevelopmentNodeStopOutcome>;
};

export class NativeContractMismatchError extends Error {
  readonly expected: string;
  readonly actual: string;

  constructor(expected: string, actual: string) {
    super(`Native contract mismatch: expected ${expected}, received ${actual}`);
    this.name = "NativeContractMismatchError";
    this.expected = expected;
    this.actual = actual;
  }
}

export class NativeBridgeError extends Error {
  readonly kind: BridgeFailure["kind"];

  constructor(failure: BridgeFailure) {
    super(`Native bridge ${failure.kind}: ${failure.detail}`);
    this.name = "NativeBridgeError";
    this.kind = failure.kind;
  }
}

export function createDevelopmentRuntime(nativeModule: PrnsAppNativeModule): DevelopmentRuntime {
  let handshake: Promise<void> | undefined;

  const verifyContract = (): Promise<void> => {
    if (handshake === undefined) {
      handshake = Promise.all([
        nativeModule.contractFingerprint(),
        nativeModule.hostContractFingerprint(),
      ])
        .then(([appActual, hostActual]) => {
          if (appActual !== NATIVE_CONTRACT_FINGERPRINT) {
            throw new NativeContractMismatchError(NATIVE_CONTRACT_FINGERPRINT, appActual);
          }
          if (hostActual !== HOST_CONTRACT_FINGERPRINT) {
            throw new NativeContractMismatchError(HOST_CONTRACT_FINGERPRINT, hostActual);
          }
        })
        .catch((error: unknown) => {
          handshake = undefined;
          throw error;
        });
    }
    return handshake;
  };

  const read = async <Wire>(operation: () => Promise<string>): Promise<Hydrated<Wire>> => {
    await verifyContract();
    const result = parseBridgeJson<Wire>(await operation());
    verifyEmbeddedContractFingerprints(result);
    return hydrateGenerated(result);
  };

  return {
    inspectDevelopmentIdentity: () => read(() => nativeModule.inspectIdentity()),
    previewIdentityImport: (identity) =>
      read(() => nativeModule.previewIdentityImport(Array.from(identity))),
    createGeneratedIdentity: () => read(() => nativeModule.createGeneratedIdentity()),
    createImportedIdentity: (identity) =>
      read(() => nativeModule.createImportedIdentity(Array.from(identity))),
    startDevelopmentNode: (input) =>
      read(() =>
        nativeModule.start(
          JSON.stringify({
            developmentTcpTarget: input.developmentTcpTarget,
          } satisfies WireDevelopmentNodeStartInput),
        ),
      ),
    readDevelopmentNodeSnapshot: () => read(() => nativeModule.snapshot()),
    initiateRemoteControlPairing: (input) =>
      read(() => nativeModule.initiatePairing(JSON.stringify(input))),
    approveRemoteControlPairing: (input) =>
      read(() => nativeModule.approvePairing(JSON.stringify(input))),
    rejectRemoteControlPairing: (input) =>
      read(() => nativeModule.rejectPairing(JSON.stringify(input))),
    describeRemoteControlTarget: (input) =>
      read(() =>
        nativeModule.describeTarget(
          JSON.stringify({
            targetIdentityFingerprint: Array.from(input.targetIdentityFingerprint),
          } satisfies WireDescribeRemoteControlTargetInput),
        ),
      ),
    saveObservedDestination: (destination) =>
      read(() =>
        nativeModule.saveObservedDestination(
          JSON.stringify({
            destination: Array.from(destination),
          } satisfies WireContactDestinationInput),
        ),
      ),
    createManualContact: (destination, identity, alias) =>
      read(() =>
        nativeModule.createManualContact(
          JSON.stringify({
            destination: Array.from(destination),
            identity: identity === null ? null : Array.from(identity),
            alias,
          } satisfies WireCreateManualContactInput),
        ),
      ),
    setContactAlias: (destination, alias) =>
      read(() =>
        nativeModule.setContactAlias(
          JSON.stringify({
            destination: Array.from(destination),
            alias,
          } satisfies WireSetContactAliasInput),
        ),
      ),
    setContactPinned: (destination, pinned) =>
      read(() =>
        nativeModule.setContactPinned(
          JSON.stringify({
            destination: Array.from(destination),
            pinned,
          } satisfies WireSetContactPinnedInput),
        ),
      ),
    deleteContact: (destination) =>
      read(() =>
        nativeModule.deleteContact(
          JSON.stringify({
            destination: Array.from(destination),
          } satisfies WireContactDestinationInput),
        ),
      ),
    getContact: (destination) =>
      read(() =>
        nativeModule.getContact(
          JSON.stringify({
            destination: Array.from(destination),
          } satisfies WireContactDestinationInput),
        ),
      ),
    listContacts: () => read(() => nativeModule.listContacts()),
    listLxmfPeers: () => read(() => nativeModule.listLxmfPeers()),
    listLxmfMessages: (input) =>
      read(() =>
        nativeModule.listLxmfMessages(
          JSON.stringify({
            peer: input.peer === null ? null : Array.from(input.peer),
            before: input.before === null ? null : input.before.toString(),
            limit: input.limit,
          } satisfies WireListLxmfMessagesInput),
        ),
      ),
    retryLxmfMessage: (localRecordId) =>
      read(() =>
        nativeModule.retryLxmfMessage(
          JSON.stringify({
            localRecordId: localRecordId.toString(),
          } satisfies WireRetryLxmfMessageInput),
        ),
      ),
    cancelLxmfMessage: (localRecordId) =>
      read(() =>
        nativeModule.cancelLxmfMessage(
          JSON.stringify({
            localRecordId: localRecordId.toString(),
          } satisfies WireCancelLxmfMessageInput),
        ),
      ),
    announceLxmf: () => read(() => nativeModule.announceLxmf()),
    measureLxmfText: (input) =>
      read(() =>
        nativeModule.measureLxmfText(
          JSON.stringify({
            title: input.title,
            content: input.content,
          } satisfies WireMeasureLxmfTextInput),
        ),
      ),
    sendDirectText: (input) =>
      read(() =>
        nativeModule.sendDirectText(
          JSON.stringify({
            destination: Array.from(input.destination),
            title: input.title,
            content: input.content,
          } satisfies WireSendDirectTextInput),
        ),
      ),
    stopDevelopmentNode: () => read(() => nativeModule.stop()),
    resetDevelopmentData: () => read(() => nativeModule.reset()),
  };
}

function parseBridgeJson<Wire>(json: string): Wire {
  let parsed: Wire | BridgeFailure;
  try {
    parsed = JSON.parse(json) as Wire | BridgeFailure;
  } catch (cause) {
    throw new NativePayloadError("$", "native result is not valid JSON", { cause });
  }
  if (isBridgeFailure(parsed)) {
    throw new NativeBridgeError(parsed);
  }
  return parsed;
}

function isBridgeFailure(value: unknown): value is BridgeFailure {
  return (
    value !== null &&
    typeof value === "object" &&
    "type" in value &&
    value.type === "bridgeFailure" &&
    "kind" in value &&
    (value.kind === "invalidInput" || value.kind === "panic") &&
    "detail" in value &&
    typeof value.detail === "string"
  );
}

function verifyEmbeddedContractFingerprints(value: unknown): void {
  if (Array.isArray(value)) {
    for (const item of value) {
      verifyEmbeddedContractFingerprints(item);
    }
    return;
  }
  if (value === null || typeof value !== "object") {
    return;
  }
  for (const [key, nested] of Object.entries(value)) {
    if (key === "contractFingerprint") {
      if (nested !== NATIVE_CONTRACT_FINGERPRINT) {
        throw new NativeContractMismatchError(
          NATIVE_CONTRACT_FINGERPRINT,
          typeof nested === "string" ? nested : String(nested),
        );
      }
    } else {
      verifyEmbeddedContractFingerprints(nested);
    }
  }
}
