import type * as Bindings from "@prns-internal/native-bindings";
import {
  bindingModule,
  HOST_CONTRACT_FINGERPRINT,
  NATIVE_CONTRACT_FINGERPRINT,
} from "@prns-internal/native-bindings";
import { type FfiConverter, UniffiInternalError } from "@ubjs/core";
import type { PrnsAppNativeModule } from "./native";

export type * from "@prns-internal/native-bindings";

export class NativeContractMismatchError extends Error {
  constructor(
    readonly expected: string,
    readonly actual: string,
  ) {
    super(`Native contract mismatch: expected ${expected}, received ${actual}`);
    this.name = "NativeContractMismatchError";
  }
}

/** Preparation errors retain the generated outcome for recovery decisions. */
export class NativeStoragePreparationError extends Error {
  constructor(readonly outcome: Bindings.NativeStoragePreparationOutcome) {
    super(`Native storage preparation: ${outcome.tag}`);
    this.name = "NativeStoragePreparationError";
  }
}

type BindingLoader = () => Promise<typeof Bindings>;
const loadNativeBindings: BindingLoader = () => import("@prns-internal/native-bindings/native");
const codecs = bindingModule.converters;
const asyncOptions = (signal: AbortSignal | undefined) =>
  signal === undefined ? undefined : { signal };

// RN's AbortSignal polyfill has .aborted and listeners, but no throwIfAborted.
function throwIfAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted === true) throw new UniffiInternalError.AbortError();
}

/**
 * Domain names and platform admission around the generated functions. Values
 * pass through unchanged; the native supervisor owns the process and storage.
 */
export function createDevelopmentRuntime(
  getNativeModule: () => PrnsAppNativeModule,
  loadBindings: BindingLoader = loadNativeBindings,
) {
  let verifiedBindings: Promise<typeof Bindings> | undefined;
  let storagePreparation: Promise<void> | undefined;

  const bindings = async (signal?: AbortSignal) => {
    throwIfAborted(signal);
    // Require the platform capability before installing the native player.
    getNativeModule();
    if (verifiedBindings === undefined) {
      verifiedBindings = loadBindings()
        .then((api) => {
          const contract = api.bindingContract();
          for (const [expected, actual] of [
            [NATIVE_CONTRACT_FINGERPRINT, contract.app],
            [HOST_CONTRACT_FINGERPRINT, contract.host],
          ] as const) {
            if (expected !== actual) throw new NativeContractMismatchError(expected, actual);
          }
          return api;
        })
        .catch((cause: unknown) => {
          verifiedBindings = undefined;
          throw cause;
        });
    }
    const api = await verifiedBindings;
    throwIfAborted(signal);
    return api;
  };
  const decode = async <T>(
    operation: (native: PrnsAppNativeModule) => Promise<number[]>,
    codec: FfiConverter<Uint8Array, T>,
  ): Promise<T> => {
    await bindings();
    return codec.lift(Uint8Array.from(await operation(getNativeModule())));
  };
  const prepareStorage = async () => {
    if (storagePreparation === undefined) {
      storagePreparation = decode(
        (native) => native.prepareStorage(),
        codecs.FfiConverterTypeNativeStoragePreparationOutcome,
      )
        .then((outcome) => {
          if (outcome.tag !== "Prepared") throw new NativeStoragePreparationError(outcome);
        })
        .catch((cause: unknown) => {
          storagePreparation = undefined;
          throw cause;
        });
    }
    await storagePreparation;
  };
  const call = async <T>(
    operation: (api: typeof Bindings) => Promise<T> | T,
    signal?: AbortSignal,
    preparation?: "storage" | "outbound",
  ): Promise<T> => {
    const api = await bindings(signal);
    if (preparation === "storage") await prepareStorage();
    if (preparation === "outbound") await getNativeModule().prepareOutbound();
    throwIfAborted(signal);
    return operation(api);
  };

  return {
    inspectDevelopmentIdentity: () =>
      decode((native) => native.inspectIdentity(), codecs.FfiConverterTypePrimaryIdentityState),
    previewIdentityImport: (identity: Uint8Array) =>
      call((api) => api.previewIdentityImport(identity)),
    createGeneratedIdentity: () =>
      decode(
        (native) => native.createGeneratedIdentity(),
        codecs.FfiConverterTypeIdentityCreationOutcome,
      ),
    createImportedIdentity: (identity: Uint8Array) =>
      decode(
        (native) => native.createImportedIdentity(Array.from(identity)),
        codecs.FfiConverterTypeIdentityCreationOutcome,
      ),
    startDevelopmentNode: async (input: Bindings.DevelopmentNodeStartInput) => {
      await bindings();
      const bytes = codecs.FfiConverterTypeDevelopmentNodeStartInput.lower(
        input,
        (size) => new Uint8Array(size),
      );
      return decode(
        (native) => native.start(Array.from(bytes)),
        codecs.FfiConverterTypeDevelopmentNodeStartOutcome,
      );
    },
    readDevelopmentNodeSnapshot: (signal?: AbortSignal) =>
      call((api) => api.readSnapshot(asyncOptions(signal)), signal),
    initiateRemoteControlPairing: (
      input: Bindings.InitiateRemoteControlPairingInput,
      signal?: AbortSignal,
    ) => call((api) => api.initiatePairing(input, asyncOptions(signal)), signal, "outbound"),
    approveRemoteControlPairing: (
      input: Bindings.RemoteControlPairingDecisionInput,
      signal?: AbortSignal,
    ) => call((api) => api.approvePairing(input, asyncOptions(signal)), signal),
    rejectRemoteControlPairing: (
      input: Bindings.RemoteControlPairingDecisionInput,
      signal?: AbortSignal,
    ) => call((api) => api.rejectPairing(input, asyncOptions(signal)), signal),
    describeRemoteControlTarget: (
      input: Bindings.DescribeRemoteControlTargetInput,
      signal?: AbortSignal,
    ) => call((api) => api.describeTarget(input, asyncOptions(signal)), signal, "outbound"),
    announceRemoteControlTarget: (
      input: Bindings.AnnounceRemoteControlTargetInput,
      signal?: AbortSignal,
    ) => call((api) => api.announceTarget(input, asyncOptions(signal)), signal, "outbound"),
    saveObservedDestination: (destination: Uint8Array, signal?: AbortSignal) =>
      call(
        (api) => api.saveObservedDestination({ destination }, asyncOptions(signal)),
        signal,
        "storage",
      ),
    createManualContact: (
      destination: Uint8Array,
      identity?: Uint8Array,
      alias?: string,
      signal?: AbortSignal,
    ) =>
      call(
        (api) => api.createManualContact({ destination, identity, alias }, asyncOptions(signal)),
        signal,
        "storage",
      ),
    setContactAlias: (destination: Uint8Array, alias?: string, signal?: AbortSignal) =>
      call(
        (api) => api.setContactAlias({ destination, alias }, asyncOptions(signal)),
        signal,
        "storage",
      ),
    setContactPinned: (destination: Uint8Array, pinned: boolean, signal?: AbortSignal) =>
      call(
        (api) => api.setContactPinned({ destination, pinned }, asyncOptions(signal)),
        signal,
        "storage",
      ),
    deleteContact: (destination: Uint8Array, signal?: AbortSignal) =>
      call((api) => api.deleteContact({ destination }, asyncOptions(signal)), signal, "storage"),
    getContact: (destination: Uint8Array, signal?: AbortSignal) =>
      call((api) => api.getContact({ destination }, asyncOptions(signal)), signal, "storage"),
    listContacts: (signal?: AbortSignal) =>
      call((api) => api.listContacts(asyncOptions(signal)), signal, "storage"),
    listLxmfPeers: (signal?: AbortSignal) =>
      call((api) => api.listLxmfPeers(asyncOptions(signal)), signal),
    listLxmfMessages: (input: Bindings.ListLxmfMessagesInput, signal?: AbortSignal) =>
      call((api) => api.listLxmfMessages(input, asyncOptions(signal)), signal, "storage"),
    retryLxmfMessage: (localRecordId: bigint, signal?: AbortSignal) =>
      call(
        (api) => api.retryLxmfMessage({ localRecordId }, asyncOptions(signal)),
        signal,
        "outbound",
      ),
    cancelLxmfMessage: (localRecordId: bigint, signal?: AbortSignal) =>
      call(
        (api) => api.cancelLxmfMessage({ localRecordId }, asyncOptions(signal)),
        signal,
        "storage",
      ),
    announceLxmf: (signal?: AbortSignal) =>
      call((api) => api.announceLxmf(asyncOptions(signal)), signal, "outbound"),
    measureLxmfText: (input: Bindings.MeasureLxmfTextInput, signal?: AbortSignal) =>
      call((api) => api.measureLxmfText(input, asyncOptions(signal)), signal),
    sendDirectText: (input: Bindings.SendDirectTextInput, signal?: AbortSignal) =>
      call((api) => api.sendDirectText(input, asyncOptions(signal)), signal, "outbound"),
    stopDevelopmentNode: () =>
      decode((native) => native.stop(), codecs.FfiConverterTypeDevelopmentNodeStopOutcome),
    resetDevelopmentData: async () => {
      const outcome = await decode(
        (native) => native.reset(),
        codecs.FfiConverterTypeDevelopmentNodeStopOutcome,
      );
      storagePreparation = undefined;
      return outcome;
    },
  };
}

export type DevelopmentRuntime = ReturnType<typeof createDevelopmentRuntime>;
