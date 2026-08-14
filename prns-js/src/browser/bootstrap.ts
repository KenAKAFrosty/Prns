import { Tag } from "../casework.js";
import type {
  BackendInfo,
  CapabilityName,
  InterfaceKind,
  PrnsLimits,
} from "../contract.js";
import { record } from "./decoding.js";
import { describeHostError } from "./host_errors.js";
import { hostGlobal } from "./host_apis.js";
import { describeStableIdentityStoreFailure } from "./persistence.js";
import type {
  StableIdentityLoadOutcome,
  StableIdentitySaveOutcome,
  StableIdentityStore,
} from "./persistence.js";
import type {
  BleIdentityAvailability,
  EntropyOutcome,
  PrnsWasmModule,
} from "./runtime_contract.js";
import {
  BLE_IDENTITY_LENGTH,
  MIN_ENTROPY_BYTES,
  PrnsValidationError,
  bleIdentity,
  identitySecretKey,
  positiveInteger,
} from "./values.js";
import type {
  EntropyBytes,
  IdentitySecretKey,
} from "./values.js";

type IdentityGenerationOutcome =
  | Tag<"Generated", IdentitySecretKey>
  | Tag<"HostApiUnavailable", { readonly api: "Crypto" }>
  | Tag<"EntropySourceFailed", { readonly detail: string }>;

export function webCryptoEntropy(length: number): EntropyOutcome {
  try {
    if (!hostGlobal().crypto) {
      return Tag("HostApiUnavailable", { api: "Crypto" });
    }
    const bytes = webCryptoBytes(length);
    if (bytes.length < MIN_ENTROPY_BYTES) {
      return Tag("InsufficientEntropy", {
        minimum: MIN_ENTROPY_BYTES,
        actual: bytes.length,
      });
    }
    return Tag("Filled", bytes as EntropyBytes);
  } catch (error) {
    return Tag("EntropySourceFailed", { detail: describeHostError(error) });
  }
}

function webCryptoBytes(length: number): Uint8Array {
  if (!Number.isSafeInteger(length) || length <= 0) {
    throw new PrnsValidationError(
      "invalid-number",
      "random byte length must be a positive safe integer",
    );
  }
  const out = new Uint8Array(length);
  const crypto = hostGlobal().crypto;
  if (!crypto) {
    throw new PrnsValidationError(
      "missing-host-api",
      "Prns entropy requires globalThis.crypto.getRandomValues",
    );
  }
  crypto.getRandomValues(out);
  return out;
}

export function webCryptoIdentity(length: number): IdentityGenerationOutcome {
  try {
    if (!hostGlobal().crypto) {
      return Tag("HostApiUnavailable", { api: "Crypto" });
    }
    return Tag(
      "Generated",
      identitySecretKey(webCryptoBytes(length), length),
    );
  } catch (error) {
    return Tag("EntropySourceFailed", { detail: describeHostError(error) });
  }
}

export async function loadOrCreateBleIdentity(
  store: StableIdentityStore,
): Promise<BleIdentityAvailability> {
  let loaded: StableIdentityLoadOutcome;
  try {
    loaded = await store.load(BLE_IDENTITY_LENGTH);
  } catch (error) {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `load Bluetooth LE identity: ${describeHostError(error)}`,
    });
  }
  if (loaded.tag === "Loaded") {
    const validated = bleIdentity(loaded.data);
    return validated.tag === "ValidBleIdentity"
      ? Tag("Available", validated.data)
      : Tag("StableIdentityUnavailable", {
          interface: "bluetooth",
          detail: `stored Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
        });
  }
  if (loaded.tag !== "Missing") {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: describeStableIdentityStoreFailure(loaded),
    });
  }
  let generatedBytes: Uint8Array;
  try {
    generatedBytes = webCryptoBytes(BLE_IDENTITY_LENGTH);
  } catch (error) {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `generate Bluetooth LE identity: ${describeHostError(error)}`,
    });
  }
  const validated = bleIdentity(generatedBytes);
  if (validated.tag !== "ValidBleIdentity") {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `generated Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
    });
  }
  const generated = validated.data;
  let saved: StableIdentitySaveOutcome;
  try {
    saved = await store.save(generated);
  } catch (error) {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `save Bluetooth LE identity: ${describeHostError(error)}`,
    });
  }
  if (saved.tag !== "Saved") {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: describeStableIdentityStoreFailure(saved),
    });
  }
  return Tag("Available", generated);
}

export function cooperativeBackendInfo(): BackendInfo {
  const webSocketAvailable = typeof globalThis.WebSocket === "function";
  const capabilities: CapabilityName[] = webSocketAvailable
    ? ["WebSocket", "BrowserRendezvous"]
    : [];
  const interfaceKinds: InterfaceKind[] = webSocketAvailable
    ? ["WebSocketClient", "BrowserRendezvous"]
    : [];
  return Object.freeze({
    backend: "Cooperative",
    capabilities: Object.freeze(capabilities),
    interfaceKinds: Object.freeze(interfaceKinds),
  });
}

export async function loadBundledWasm(): Promise<
  | Tag<"Loaded", PrnsWasmModule>
  | Tag<"WasmLoadFailed", { readonly detail: string }>
> {
  const moduleUrl = bundledWasmModuleUrl();
  try {
    const imported: unknown = await import(moduleUrl.href);
    const module = record(imported, "bundled WebAssembly module");
    const initialize = module.default;
    if (typeof initialize !== "function") {
      return Tag("WasmLoadFailed", {
        detail: "bundled WebAssembly module has no initializer",
      });
    }
    await initialize();
    return Tag("Loaded", imported as PrnsWasmModule);
  } catch (error) {
    return Tag("WasmLoadFailed", { detail: describeHostError(error) });
  }
}

export function bundledWasmModuleUrl(): URL {
  return new URL("../../wasm/prns_wasm.js", import.meta.url);
}

export function browserLimits(limits: PrnsLimits): PrnsLimits {
  return {
    pendingCommands: positiveInteger(
      limits.pendingCommands,
      "pending command limit",
    ),
    applicationEvents: positiveInteger(
      limits.applicationEvents,
      "application event limit",
    ),
    retainedEventBytes: positiveInteger(
      limits.retainedEventBytes,
      "retained event byte limit",
    ),
    diagnostics: positiveInteger(limits.diagnostics, "diagnostic limit"),
  };
}
