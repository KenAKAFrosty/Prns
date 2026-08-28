import type {
  HostCommand,
  PrnsLimits,
} from "../contract.js";
import type { PrnsDiagnosticEvent } from "./events.js";
import type { BrowserPersistedState } from "./persistence.js";
import type { RegisterSingleDestinationOptions } from "./runtime_contract.js";
import type { BleIdentity, IdentitySecretKey } from "./values.js";

export type WorkerInitialization = {
  readonly identity: IdentitySecretKey;
  readonly bleIdentity?: BleIdentity;
  readonly persistedState?: BrowserPersistedState;
  readonly persistenceEnabled: boolean;
  readonly limits: PrnsLimits;
  readonly resourceCompressionModuleUrl?: string;
  readonly wasmModuleUrl?: string;
  readonly autoWifiSelectionSeed?: Uint8Array;
};

export type WorkerCall =
  | {
      readonly operation: "registerSingleDestination";
      readonly value: RegisterSingleDestinationOptions;
    }
  | { readonly operation: "registerNodePage"; readonly value: Uint8Array }
  | { readonly operation: "execute"; readonly value: HostCommand }
  | {
      readonly operation: "sendResourceBlob";
      readonly value: {
        readonly linkId: Uint8Array;
        readonly blob: Blob;
        readonly options: unknown;
      };
    }
  | { readonly operation: "snapshot" }
  | { readonly operation: "hostSnapshot" }
  | { readonly operation: "stop" }
  | {
      readonly operation: "webSocketConnect";
      readonly value: { readonly url: string; readonly options: unknown };
    }
  | { readonly operation: "autoWifiStart" }
  | { readonly operation: "autoWifiStatus" }
  | { readonly operation: "autoWifiClose" }
  | { readonly operation: "interfaceSessionClose"; readonly value: number };

export type WorkerControlRequest = {
  readonly type: "call";
  readonly id: number;
  readonly call: WorkerCall;
};

export type WorkerCapabilityCall =
  | { readonly operation: "registerInterface"; readonly value: unknown }
  | { readonly operation: "deactivateInterface"; readonly value: Uint8Array }
  | {
      readonly operation: "ingest";
      readonly value: { readonly interfaceId: Uint8Array; readonly bytes: Uint8Array };
    }
  | {
      readonly operation: "takeOutbound";
      readonly value: { readonly interfaceId: Uint8Array; readonly maximumFrames?: number };
    }
  | { readonly operation: "waitForOutboundActivity"; readonly value: Uint8Array }
  | { readonly operation: "createBluetoothReassembler" }
  | {
      readonly operation: "absorbBluetoothFragment";
      readonly value: { readonly id: number; readonly bytes: Uint8Array };
    }
  | { readonly operation: "releaseBluetoothReassembler"; readonly value: number }
  | { readonly operation: "createUsbAutoDecoder" }
  | {
      readonly operation: "feedUsbAutoDecoder";
      readonly value: { readonly id: number; readonly bytes: Uint8Array };
    }
  | { readonly operation: "releaseUsbAutoDecoder"; readonly value: number };

export type WorkerCapabilityRequest = {
  readonly type: "call";
  readonly id: number;
  readonly call: WorkerCapabilityCall;
};

export type WorkerControlResponse =
  | { readonly type: "started"; readonly outcome: unknown }
  | { readonly type: "settled"; readonly id: number; readonly outcome: unknown }
  | {
      readonly type: "eventBackpressureExceeded";
      readonly rejectedEventBytes: number;
    }
  | {
      readonly type: "protocolFailed";
      readonly id?: number;
      readonly detail: string;
    };

export type WorkerEventMessage =
  | { readonly type: "batch"; readonly id: number; readonly buffer: ArrayBuffer }
  | {
      readonly type: "diagnostic";
      readonly id: number;
      readonly event: PrnsDiagnosticEvent;
    };

export type WorkerEventAcknowledgement = {
  readonly type: "acknowledge";
  readonly id: number;
};

export type WorkerStartMessage = {
  readonly type: "initialize";
  readonly initialization: WorkerInitialization;
  readonly control: MessagePort;
  readonly events: MessagePort;
  readonly capabilities: MessagePort;
};
